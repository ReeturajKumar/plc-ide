import { fsApi, runtimeApi } from "../utils/tauri";
import { baseName } from "../utils/project";
import type { CompileReport, CompileResult, RuntimeState, StError } from "../types/runtime";
import type { PLCProgram } from "../types/program";
import { useEditorStore } from "./editorStore";
import { useProjectStore } from "./projectStore";
import { useUIStore } from "./uiStore";
import { useConsoleStore, type Problem } from "./consoleStore";
import { useSimulatorStore } from "./simulatorStore";
import { openFile, persistProjectJson } from "./projectActions";
import { revealPosition } from "../utils/editorRef";

// The Rust scan loop runs on its own clock; the UI mirrors it by polling at the
// default scan rate. Events would be the upgrade if scan times drop well below this.
const POLL_MS = 100;
let pollTimer: ReturnType<typeof setInterval> | null = null;

const stamp = () => new Date().toLocaleTimeString();
const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? "" : "s"}`;

function isStError(e: unknown): e is StError {
    return typeof e === "object" && e !== null && "message" in e && "line" in e && "column" in e;
}

function isCompileReport(e: unknown): e is CompileReport {
    return typeof e === "object" && e !== null && "programs" in e && "mapping" in e && Array.isArray(e.mapping);
}

/** Problems with the I/O mappings aren't in a file; they're listed under this pseudo-path. */
export const IO_MAPPING_PATH = "#io-mapping";

const ioMappings = () => useProjectStore.getState().project?.io?.mappings ?? [];

/** The error a control command was refused with. */
function toStError(e: unknown): StError {
    if (isStError(e)) return e;
    console.error("[runtime]", e);
    return { kind: "General", line: 0, column: 0, message: "The PLC runtime is not available." };
}

export function formatStError(e: StError): string {
    return e.line > 0 ? `Line ${e.line}, Column ${e.column}: ${e.message}` : e.message;
}

/** Diagnostics for one file, in the shape the Problems panel and editor use. */
export function toProblems(path: string, errors: StError[]): Problem[] {
    return errors.map((e) => ({ path, fileName: baseName(path), kind: e.kind, line: e.line, column: e.column, message: e.message }));
}

interface ProgramText {
    program: PLCProgram;
    source: string;
}

/**
 * Every program of the project in execution order, with its current source: the open
 * tab's (unsaved edits included) or else the file on disk. Null if one can't be read.
 */
export async function projectSources(): Promise<ProgramText[] | null> {
    const project = useProjectStore.getState().project;
    if (!project) return null;
    const open = useEditorStore.getState().openFiles;
    try {
        return await Promise.all(
            project.programs.map(async (program) => ({
                program,
                source: open.find((f) => f.path === program.path)?.content ?? (await fsApi.readFile(project.rootPath, program.path)),
            }))
        );
    } catch (e) {
        useUIStore.getState().notify("error", typeof e === "string" ? e : "Couldn't read the project's programs.");
        return null;
    }
}

const toSources = (items: ProgramText[]) => items.map(({ program, source }) => ({ name: program.name, source }));

/**
 * Record compile results for `items` (results[i] belongs to items[i]): each program's
 * status, its Problems and editor squiggles, and a summary in Output. Returns every
 * error as a Problem.
 */
function reportCompile(items: ProgramText[], results: CompileResult[], mapping: StError[], title: string): Problem[] {
    const ps = useProjectStore.getState();
    const cs = useConsoleStore.getState();
    const lines = [`[${stamp()}] ${title}`];
    const problems: Problem[] = [];
    const failed: string[] = [];
    items.forEach(({ program, source }, i) => {
        const errors = results[i]?.errors ?? [];
        ps.setBreakableLines(program.path, results[i]?.lines ?? []);
        if (results[i]) ps.setFileKind(program.path, { hasProgram: results[i].hasProgram, functionBlocks: results[i].functionBlocks });
        const mine = toProblems(program.path, errors);
        ps.setCompiled(program.path, { source, errorCount: errors.length });
        cs.setFileProblems(program.path, mine);
        problems.push(...mine);
        if (errors.length === 0) {
            lines.push(`  ✔ ${program.name}`);
        } else {
            lines.push(`  ✖ ${program.name}  ${plural(errors.length, "error")}`);
            lines.push(...errors.map((e) => `      ${program.fileName} ${formatStError(e)}`));
            failed.push(`${plural(errors.length, "error")} found in ${program.name}.`);
        }
    });
    const mappingProblems = mapping.map((e) => ({ ...toProblems(IO_MAPPING_PATH, [e])[0], fileName: "I/O Mapping" }));
    cs.setFileProblems(IO_MAPPING_PATH, mappingProblems);
    problems.push(...mappingProblems);
    if (mapping.length > 0) {
        lines.push(`  ✖ I/O mapping  ${plural(mapping.length, "error")}`, ...mapping.map((e) => `      ${e.message}`));
        failed.push(`${plural(mapping.length, "error")} found in the I/O mapping.`);
    }
    if (failed.length === 0) {
        lines.push(items.length === 1 ? `  ${items[0].program.name} compiled.` : `  Compilation succeeded: ${plural(items.length, "program")} compiled.`);
    } else {
        lines.push("  Compilation failed.", ...failed.map((f) => `  ${f}`));
        useUIStore.getState().setPanelTab("problems");
    }
    cs.appendOutput([...lines, ""]);
    return problems;
}

/** Compile `items` (the whole project, so shared variables resolve); report `only` of them. */
async function compile(items: ProgramText[], only: ProgramText[], title: string): Promise<boolean> {
    const ps = useProjectStore.getState();
    ps.setCompiling(only.map((i) => i.program.path));
    let report: CompileReport;
    try {
        report = await runtimeApi.compile(toSources(items), ioMappings());
    } catch (e) {
        useUIStore.getState().notify("error", toStError(e).message);
        return false;
    } finally {
        ps.setCompiling([]);
    }
    const picked = only.map((item) => report.programs[items.indexOf(item)]);
    return reportCompile(only, picked, report.mapping, title).length === 0;
}

/**
 * Errors of the program at `path` for the editor's live diagnostics, analyzed together
 * with the rest of the project (so shared variables resolve). Records nothing; null if
 * `path` isn't one of the project's programs.
 */
export async function diagnose(path: string): Promise<StError[] | null> {
    const items = await projectSources();
    const index = items?.findIndex((i) => i.program.path === path) ?? -1;
    if (!items || index < 0) return null;
    const report = await runtimeApi.compile(toSources(items), ioMappings());
    useProjectStore.getState().setBreakableLines(path, report.programs[index]?.lines ?? []);
    const result = report.programs[index];
    if (result) useProjectStore.getState().setFileKind(path, { hasProgram: result.hasProgram, functionBlocks: result.functionBlocks });
    return report.programs[index]?.errors ?? [];
}

/** Compile every program of the project; reports every program's errors. */
export async function compileAll(): Promise<boolean> {
    const items = await projectSources();
    if (!items) return false;
    if (items.length === 0) {
        useUIStore.getState().notify("error", "This project has no programs to compile.");
        return false;
    }
    return compile(items, items, "Compile All");
}

/**
 * Compile the program open in the editor. Only its result is reported; the other
 * programs are read for the variables it may share with them.
 */
export async function compileCurrent(): Promise<boolean> {
    const activePath = useEditorStore.getState().activePath;
    const items = await projectSources();
    if (!items) return false;
    const current = items.find((i) => i.program.path === activePath);
    if (!current) {
        useUIStore.getState().notify("error", "Open a program (.st file) to compile it.");
        return false;
    }
    return compile(items, [current], `Compile ${current.program.name}`);
}

function startPolling(): void {
    if (!pollTimer) pollTimer = setInterval(() => void refresh(), POLL_MS);
}

function stopPolling(): void {
    if (pollTimer) {
        clearInterval(pollTimer);
        pollTimer = null;
    }
}

/** A scan failed: show it in the simulator panel, Problems (on its program) and Output. */
function reportScanError(error: StError, programName: string | null): void {
    const program = useProjectStore.getState().project?.programs.find((p) => p.name === programName);
    const path = program?.path ?? programName ?? "";
    const problem = toProblems(path, [error])[0];
    useSimulatorStore.getState().setErrors([problem]);
    if (program) useConsoleStore.getState().setFileProblems(program.path, [problem]);
    useConsoleStore.getState().appendOutput([`  ✖ ${programName ?? "PLC"}: ${formatStError(error)}`, ""]);
}

// The debug location last shown, so a new stop opens its source only once: switching
// tabs while paused is left alone.
let shownLocation: string | null = null;

/** Path of the file a debug location is in. */
export function locationPath(file: string): string | undefined {
    return useProjectStore.getState().project?.programs.find((p) => p.name === file)?.path;
}

/** The debugger stopped somewhere new: open that source (program or block) at the line. */
function followLocation(state: RuntimeState): void {
    const loc = state.location;
    const key = loc ? `${loc.file}:${loc.line}:${loc.instance ?? ""}:${state.cycleCount}` : null;
    if (key === shownLocation) return;
    shownLocation = key;
    if (!loc) return;
    const path = locationPath(loc.file);
    if (path) void openFile(path).then(() => revealPosition(loc.line, loc.column));
    useUIStore.getState().setPanelTab("simulator");
}

function applyState(state: RuntimeState): void {
    const sim = useSimulatorStore.getState();
    sim.setRuntime(state);
    followLocation(state);
    // A failing scan stops the runtime and leaves its error in every later snapshot; report it once.
    const shown = sim.errors.length === 1 ? formatStError(sim.errors[0]) : null;
    if (state.error && formatStError(state.error) !== shown) reportScanError(state.error, state.errorProgram);
    if (state.status === "STOPPED") stopPolling();
    else startPolling();
}

async function refresh(): Promise<void> {
    try {
        applyState(await runtimeApi.getState());
    } catch (e) {
        stopPolling();
        reportScanError(toStError(e), null);
    }
}

/** Run a control command; show a toast instead of throwing when it's refused. */
async function control(action: () => Promise<RuntimeState>): Promise<boolean> {
    try {
        applyState(await action());
        return true;
    } catch (e) {
        useUIStore.getState().notify("error", formatStError(toStError(e)));
        return false;
    }
}

/**
 * Compile every program and start the PLC: each scan runs all of them, in project order,
 * whichever one is open in the editor. If any program has an error nothing runs. With
 * `paused` the programs are loaded but no scan runs (for STEP).
 */
export async function runProgram(paused = false): Promise<boolean> {
    const ui = useUIStore.getState();
    const items = await projectSources();
    if (!items) return false;
    if (items.length === 0) {
        ui.notify("error", "This project has no programs to run. Create a .st program first.");
        return false;
    }

    const sim = useSimulatorStore.getState();
    const cs = useConsoleStore.getState();
    sim.setErrors([]);
    ui.setPanelTab("simulator");
    cs.appendOutput([`[${stamp()}] ▶ Starting ${items.map((i) => i.program.name).join(" → ")} …`]);

    const ps = useProjectStore.getState();
    ps.setCompiling(items.map((i) => i.program.path));
    await syncBreakpoints(); // the runtime may have restarted since they were set
    try {
        const state = await runtimeApi.start(toSources(items), ioMappings(), paused);
        ps.setCompiling([]);
        // Everything compiled from exactly these sources.
        for (const { program, source } of items) {
            ps.setCompiled(program.path, { source, errorCount: 0 });
            cs.setFileProblems(program.path, []);
        }
        cs.setFileProblems(IO_MAPPING_PATH, []);
        applyState(state);
        cs.appendOutput([
            paused
                ? `  ✔ Loaded ${plural(items.length, "program")}, paused — STEP runs one scan`
                : `  ✔ Running ${plural(items.length, "program")} — scan every ${state.scanTimeMs} ms`,
        ]);
        return true;
    } catch (e) {
        ps.setCompiling([]);
        // Nothing runs.
        stopPolling();
        sim.setRuntime({ ...sim.runtime, status: "STOPPED", cycleCount: 0, programs: [], variables: [], functionBlocks: [] });
        if (isCompileReport(e)) {
            sim.setErrors(reportCompile(items, e.programs, e.mapping, "Compile All"));
            cs.appendOutput(["  Runtime not started.", ""]);
        } else {
            reportScanError(toStError(e), null);
        }
        return false;
    }
}

export async function stopProgram(): Promise<void> {
    if (await control(runtimeApi.stop)) {
        useSimulatorStore.getState().setErrors([]);
        useConsoleStore.getState().appendOutput([`[${stamp()}] ■ Stopped`, ""]);
    }
}

export async function pauseProgram(): Promise<void> {
    if (await control(runtimeApi.pause)) {
        useConsoleStore.getState().appendOutput([`[${stamp()}] ❚❚ Paused`]);
    }
}

export async function resumeProgram(): Promise<void> {
    if (await control(runtimeApi.resume)) {
        useConsoleStore.getState().appendOutput([`[${stamp()}] ▶ Resumed`]);
    }
}

/** STEP SCAN (while paused): finish the current scan, or run one complete scan. */
export async function stepProgram(): Promise<void> {
    if (useSimulatorStore.getState().runtime.status !== "PAUSED") return;
    if (await control(runtimeApi.step)) {
        const { cycleCount } = useSimulatorStore.getState().runtime;
        useConsoleStore.getState().appendOutput([`[${stamp()}] ⏭ Step — scan ${cycleCount} complete`]);
    }
}

/** STEP STATEMENT (while paused): run the next statement and stop before the one after. */
export async function stepStatement(): Promise<void> {
    if (useSimulatorStore.getState().runtime.status !== "PAUSED") return;
    await control(runtimeApi.stepStatement);
}

/** Send every breakpoint (file name + line) to the runtime, which does the stopping. */
export async function syncBreakpoints(): Promise<void> {
    const project = useProjectStore.getState().project;
    const list = (project?.programs ?? []).flatMap((p) => (project?.breakpoints?.[p.path] ?? []).map((line) => ({ program: p.name, line })));
    await control(() => runtimeApi.setBreakpoints(list));
}

/**
 * Add or remove a breakpoint (saved in project.json). Only lines where a statement starts
 * can stop execution; removing one always works.
 */
export async function toggleBreakpoint(path: string, line: number): Promise<void> {
    const ps = useProjectStore.getState();
    const lines = ps.project?.breakpoints?.[path] ?? [];
    if (lines.includes(line)) {
        ps.setBreakpoints(path, lines.filter((l) => l !== line));
    } else if (!(ps.breakableLines[path] ?? []).includes(line)) {
        useUIStore.getState().notify("info", `Line ${line} has no executable statement; breakpoints go on statements.`);
        return;
    } else {
        ps.setBreakpoints(path, [...lines, line].sort((a, b) => a - b));
    }
    await persistProjectJson();
    await syncBreakpoints();
}

/** F5: run when stopped, resume when paused. */
export function runOrResume(): void {
    const status = useSimulatorStore.getState().runtime.status;
    if (status === "STOPPED") void runProgram();
    else if (status === "PAUSED") void resumeProgram();
}

/** Set a simulated input (DI: true/false, AI: a number); the next scan reads it. */
export async function setIoInput(address: string, value: boolean | number): Promise<void> {
    await control(() => runtimeApi.setIoInput(address, value));
}

/** A variable a program declares, as the I/O mapping fields suggest it. */
export interface DeclaredVariable {
    program: string;
    name: string;
    dataType: string;
}

/**
 * Check the I/O mappings now (and apply them to a loaded PLC, so a change works without
 * a restart). Their errors go to Problems and the I/O tab. Returns every program's
 * declared variables, for the mapping suggestions; null if the programs can't be read.
 */
export async function checkIoMappings(): Promise<DeclaredVariable[] | null> {
    const items = await projectSources();
    if (!items) return null;
    const mappings = ioMappings();
    try {
        const report = await runtimeApi.compile(toSources(items), mappings);
        let errors = report.mapping;
        // A mapping the compiler accepts is applied to the running/paused PLC at once.
        if (errors.length === 0 && useSimulatorStore.getState().runtime.status !== "STOPPED") {
            errors = await runtimeApi.applyIoMappings(mappings);
            await loadIoState();
        }
        const problems = errors.map((e) => ({ ...toProblems(IO_MAPPING_PATH, [e])[0], fileName: "I/O Mapping" }));
        useConsoleStore.getState().setFileProblems(IO_MAPPING_PATH, problems);
        return report.programs.flatMap((r) => r.variables.map((v) => ({ program: r.name, ...v })));
    } catch (e) {
        useUIStore.getState().notify("error", toStError(e).message);
        return null;
    }
}

/** Compile quietly to learn what every file defines (for the function block library). */
export async function refreshFileKinds(): Promise<void> {
    const items = await projectSources();
    if (!items) return;
    try {
        const report = await runtimeApi.compile(toSources(items), ioMappings());
        const ps = useProjectStore.getState();
        items.forEach(({ program }, i) => {
            const r = report.programs[i];
            if (r) ps.setFileKind(program.path, { hasProgram: r.hasProgram, functionBlocks: r.functionBlocks });
        });
    } catch (e) {
        console.error("[library]", e);
    }
}

/** Show the simulated I/O as it is now (it exists before any RUN). */
export async function loadIoState(): Promise<void> {
    try {
        applyState(await runtimeApi.getState());
    } catch (e) {
        console.error("[io]", e);
    }
}

export async function toggleInput(program: string, name: string, current: boolean): Promise<void> {
    await control(() => runtimeApi.setInput(program, name, !current));
}

/** Stop the simulator if it's active — used when the project it came from goes away. */
export async function stopIfActive(): Promise<void> {
    if (useSimulatorStore.getState().runtime.status !== "STOPPED") await stopProgram();
}
