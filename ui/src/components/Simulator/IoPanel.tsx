import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { useSimulatorStore } from "../../store/simulatorStore";
import { useProjectStore } from "../../store/projectStore";
import { checkIoMappings, IO_MAPPING_PATH, loadIoState, setIoInput, type DeclaredVariable } from "../../store/runActions";
import { useConsoleStore } from "../../store/consoleStore";
import { clearIoMappings, setIoMapping } from "../../store/projectActions";
import type { FbSummary, IoKind, IoPoint } from "../../types/runtime";
import { compilerApi } from "../../services/compilerApi";
import { formatValue, valueClass } from "./format";

const GROUPS: { kind: IoKind; title: string }[] = [
    { kind: "DigitalInput", title: "DIGITAL INPUTS" },
    { kind: "DigitalOutput", title: "DIGITAL OUTPUTS" },
    { kind: "AnalogInput", title: "ANALOG INPUTS" },
    { kind: "AnalogOutput", title: "ANALOG OUTPUTS" },
];

const isDigital = (kind: IoKind) => kind === "DigitalInput" || kind === "DigitalOutput";
const isInput = (kind: IoKind) => kind === "DigitalInput" || kind === "AnalogInput";

const inputClass =
    "w-full min-w-0 px-1.5 py-0.5 rounded border border-[var(--theme-border)] bg-[var(--theme-bg-input)] text-[var(--theme-text-primary)] focus:outline-none focus:border-[var(--theme-border-focus)]";

const commitOnEnter = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") e.currentTarget.blur();
    if (e.key === "Escape") {
        e.currentTarget.dataset.cancel = "1";
        e.currentTarget.blur();
    }
};

/**
 * The variable wired to an address, with a compact themed suggestion list (the native
 * datalist popup can't be styled). Saved in project.json and applied right away.
 */
const MappingField = ({ address, mapped, options, error }: { address: string; mapped: string; options: string[]; error?: string }) => {
    const [draft, setDraft] = useState(mapped);
    const [open, setOpen] = useState(false);
    // Only typing filters the list; opening it (focus, click) shows every suggestion.
    const [filtering, setFiltering] = useState(false);
    const [active, setActive] = useState(0);
    const [rect, setRect] = useState<DOMRect | null>(null);
    const ref = useRef<HTMLInputElement>(null);
    useEffect(() => setDraft(mapped), [mapped]);

    const typed = draft.trim().toLowerCase();
    const matches = filtering ? options.filter((o) => o.toLowerCase().includes(typed)) : options;
    const shown = open && matches.length > 0;

    const show = (filter: boolean) => {
        setRect(ref.current?.getBoundingClientRect() ?? null);
        setFiltering(filter);
        // Opening highlights the current mapping, so it's clear what is selected.
        setActive(filter ? 0 : Math.max(0, options.indexOf(draft)));
        setOpen(true);
    };
    const commit = (value: string) => {
        setOpen(false);
        if (value.trim() !== mapped) void setIoMapping(address, value);
    };

    return (
        <>
            <input
                ref={ref}
                value={draft}
                placeholder="not mapped"
                aria-label={`Variable mapped to ${address}`}
                role="combobox"
                aria-expanded={shown}
                aria-autocomplete="list"
                spellCheck={false}
                onFocus={() => show(false)}
                // Still focused after a pick: a click reopens the list.
                onClick={() => !open && show(false)}
                onChange={(e) => {
                    setDraft(e.target.value);
                    show(true);
                }}
                onKeyDown={(e) => {
                    if (shown && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
                        e.preventDefault();
                        const step = e.key === "ArrowDown" ? 1 : -1;
                        setActive((i) => (i + step + matches.length) % matches.length);
                    } else if (shown && e.key === "Enter") {
                        setDraft(matches[active]);
                        commit(matches[active]);
                        e.currentTarget.blur();
                    } else commitOnEnter(e);
                }}
                onBlur={(e) => {
                    setOpen(false);
                    if (e.currentTarget.dataset.cancel) {
                        delete e.currentTarget.dataset.cancel;
                        setDraft(mapped);
                    } else commit(draft);
                }}
                title={error}
                aria-invalid={!!error}
                className={`${inputClass} ${error ? "border-red-500" : ""}`}
            />
            {shown && rect && (
                <ul
                    role="listbox"
                    // Opens upward when there's no room below (max-h-40 = 160px).
                    style={
                        rect.bottom + 164 > window.innerHeight
                            ? { left: rect.left, bottom: window.innerHeight - rect.top + 2, width: rect.width }
                            : { left: rect.left, top: rect.bottom + 2, width: rect.width }
                    }
                    className="fixed z-50 max-h-40 overflow-y-auto theme-panel border border-[var(--theme-border)] rounded shadow-xl py-0.5 font-mono text-[11px]"
                >
                    {matches.map((name, i) => (
                        <li
                            key={name}
                            role="option"
                            aria-selected={i === active}
                            // mousedown, not click: picking must happen before the input's blur.
                            onMouseDown={(e) => {
                                e.preventDefault();
                                setDraft(name);
                                commit(name);
                                ref.current?.blur();
                            }}
                            onMouseEnter={() => setActive(i)}
                            className={`px-2 py-0.5 cursor-pointer truncate ${
                                i === active
                                    ? "bg-[var(--theme-bg-hover)] text-[var(--theme-text-primary)]"
                                    : "text-[var(--theme-text-secondary)]"
                            }`}
                        >
                            {name}
                        </li>
                    ))}
                </ul>
            )}
        </>
    );
};

/** An analog input's value: typed, then sent on Enter or when the field loses focus. */
const AnalogField = ({ point }: { point: IoPoint }) => {
    const shown = formatValue("REAL", point.value);
    const [draft, setDraft] = useState(shown);
    useEffect(() => setDraft(shown), [shown]);
    const value = Number(draft);
    const valid = draft.trim() !== "" && Number.isFinite(value);
    return (
        <input
            value={draft}
            inputMode="decimal"
            aria-label={`${point.address} value`}
            aria-invalid={!valid}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={commitOnEnter}
            onBlur={(e) => {
                if (e.currentTarget.dataset.cancel || !valid) {
                    delete e.currentTarget.dataset.cancel;
                    setDraft(shown);
                } else if (value !== point.value) void setIoInput(point.address, value);
            }}
            className={`${inputClass} w-24 tabular-nums ${valid ? "" : "border-red-500"}`}
        />
    );
};

const PointValue = ({ point }: { point: IoPoint }) => {
    if (point.kind === "AnalogInput") return <AnalogField point={point} />;
    if (point.kind === "DigitalInput") {
        const on = point.value === true;
        return (
            <button
                onClick={() => void setIoInput(point.address, !on)}
                aria-pressed={on}
                aria-label={`Toggle ${point.address}`}
                title={`Set ${point.address} ${on ? "OFF" : "ON"}`}
                className={`w-14 py-0.5 rounded border text-[11px] font-semibold transition-colors cursor-pointer focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)] ${
                    on
                        ? "bg-emerald-500/20 border-emerald-500/60 text-emerald-300"
                        : "bg-[var(--theme-bg-input)] border-[var(--theme-border)] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
                }`}
            >
                {on ? "ON" : "OFF"}
            </button>
        );
    }
    // Outputs are written by the PLC only: displayed, never edited.
    return (
        <span className={`font-semibold tabular-nums ${valueClass(point.value)}`}>
            {formatValue(isDigital(point.kind) ? "BOOL" : "REAL", point.value)}
        </span>
    );
};

/**
 * Simulated PLC I/O: set inputs, watch outputs, and map variables to addresses. Values
 * come from the Rust runtime; inputs are read at the start of each scan and outputs are
 * written at its end.
 */
const IoPanel = () => {
    const io = useSimulatorStore((s) => s.runtime.io);
    const variables = useSimulatorStore((s) => s.runtime.variables);
    const mappings = useProjectStore((s) => s.project?.io?.mappings);
    const problems = useConsoleStore((s) => s.problems);
    const mappingErrors = problems.filter((p) => p.path === IO_MAPPING_PATH);
    const [declared, setDeclared] = useState<DeclaredVariable[]>([]);
    // Interfaces of every block type (standard + the project's), for member suggestions.
    const fileKinds = useProjectStore((s) => s.fileKinds);
    const [standard, setStandard] = useState<FbSummary[]>([]);
    useEffect(() => {
        compilerApi.standardFunctionBlocks().then(setStandard).catch((e) => console.error("[io]", e));
    }, []);
    const blockTypes = [...standard, ...Object.values(fileKinds).flatMap((k) => k.functionBlocks)];

    // The image exists before any RUN (inputs can be set while stopped).
    useEffect(() => {
        if (io.length === 0) void loadIoState();
    }, [io.length]);

    // Check the saved mappings and learn the declared variables, so the fields can
    // suggest them before any RUN (and again when the programs are reloaded).
    useEffect(() => {
        void checkIoMappings().then((vars) => vars && setDeclared(vars));
    }, [variables.length]);

    // Suggestions of the right type. A name several programs declare is offered as
    // Program.Variable, since a bare name would be ambiguous. Function block instances offer
    // Instance.Member: their inputs (fed by DI/AI) and, for DO/AO, their outputs too.
    const names = (dataType: string, forInput: boolean) => {
        const count = (name: string) => declared.filter((v) => v.name.toLowerCase() === name.toLowerCase()).length;
        const qualified = (v: DeclaredVariable) => (count(v.name) > 1 ? `${v.program}.${v.name}` : v.name);
        const variables = declared.filter((v) => v.dataType === dataType).map(qualified);
        const members = declared.flatMap((v) => {
            const fb = blockTypes.find((b) => b.name.toLowerCase() === v.dataType.toLowerCase());
            if (!fb) return [];
            const usable = forInput ? fb.inputs : [...fb.inputs, ...fb.outputs];
            return usable.filter((m) => m.dataType === dataType).map((m) => `${qualified(v)}.${m.name}`);
        });
        return [...new Set([...variables, ...members])];
    };
    // An input variable follows one input only, so an input's suggestions leave out the
    // variables other inputs already use.
    const optionsFor = (point: IoPoint) => {
        const all = names(isDigital(point.kind) ? "BOOL" : "REAL", isInput(point.kind));
        if (!isInput(point.kind)) return all;
        const taken = new Set(
            (mappings ?? [])
                .filter((m) => m.address !== point.address && io.some((p) => p.address === m.address && isInput(p.kind)))
                .map((m) => m.variable.toLowerCase())
        );
        return all.filter((name) => !taken.has(name.toLowerCase()));
    };
    const errorFor = (address: string) => mappingErrors.find((p) => new RegExp(`\\b${address}\\b`).test(p.message))?.message;

    return (
        <div className="flex flex-col gap-3 font-sans text-xs whitespace-normal text-[var(--theme-text-secondary)]">
            {(mappings?.length ?? 0) > 0 && (
                <div>
                    <button
                        onClick={() => void clearIoMappings()}
                        className="px-2 py-0.5 rounded border border-[var(--theme-border)] text-[11px] text-[var(--theme-text-secondary)] hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)] cursor-pointer"
                    >
                        Clear All Mappings
                    </button>
                </div>
            )}
            {mappingErrors.length > 0 && (
                <ul role="alert" className="max-w-5xl rounded border border-red-500/40 bg-red-500/10 px-3 py-2 font-mono text-[11px] text-red-300">
                    {mappingErrors.map((p, i) => (
                        <li key={i}>✖ {p.message}</li>
                    ))}
                </ul>
            )}
            <div className="grid gap-x-8 gap-y-3 grid-cols-[repeat(auto-fill,minmax(300px,1fr))] max-w-5xl">
                {GROUPS.map(({ kind, title }) => (
                    <table key={kind} className="w-full text-left font-mono">
                        <caption className="text-left text-[11px] font-semibold tracking-wider text-[var(--theme-text-muted)] pb-1">
                            {title}
                        </caption>
                        <thead>
                            <tr className="text-[11px] text-[var(--theme-text-muted)]">
                                <th className="font-normal py-1 pr-3 w-12">Address</th>
                                <th className="font-normal py-1 pr-3">Variable</th>
                                <th className="font-normal py-1 w-24">{isInput(kind) ? "Input" : "Output"}</th>
                            </tr>
                        </thead>
                        <tbody>
                            {io
                                .filter((p) => p.kind === kind)
                                .map((point) => (
                                    <tr key={point.address} className="border-t border-[var(--theme-border)]">
                                        <td className="py-1 pr-3 text-[var(--theme-text-primary)]">{point.address}</td>
                                        <td className="py-1 pr-3">
                                            <MappingField
                                                address={point.address}
                                                mapped={mappings?.find((m) => m.address === point.address)?.variable ?? ""}
                                                options={optionsFor(point)}
                                                error={errorFor(point.address)}
                                            />
                                        </td>
                                        <td className="py-1">
                                            <PointValue point={point} />
                                        </td>
                                    </tr>
                                ))}
                        </tbody>
                    </table>
                ))}
            </div>
        </div>
    );
};

export default IoPanel;
