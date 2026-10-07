import type { FileNode, PLCProject, ProjectFile } from "../types/project";
import type { PLCProgram } from "../types/program";

export const PROJECT_VERSION = "0.1.0";

/** Default source for a project's main program. Not executed in Phase 1 — source only. */
export const DEFAULT_MAIN_ST = `PROGRAM main

VAR
    Start : BOOL := FALSE;
    Motor : BOOL := FALSE;
END_VAR

IF Start THEN
    Motor := TRUE;
ELSE
    Motor := FALSE;
END_IF;

END_PROGRAM
`;

/** Boilerplate for a new file in a project's function-blocks folder. */
export function defaultFunctionBlockSource(name: string): string {
    return `FUNCTION_BLOCK ${name}\n\nVAR_INPUT\nEND_VAR\n\nVAR_OUTPUT\nEND_VAR\n\nVAR\nEND_VAR\n\nEND_FUNCTION_BLOCK\n`;
}

/** Boilerplate for a newly created program. */
export function defaultProgramSource(name: string): string {
    return `PROGRAM ${name}\n\nVAR\nEND_VAR\n\nEND_PROGRAM\n`;
}

/** Joins a project root with a relative path using the OS separator. */
export function joinPath(root: string, rel: string): string {
    const sep = root.includes("\\") ? "\\" : "/";
    return `${root.replace(/[\\/]+$/, "")}${sep}${rel.replace(/[\\/]+/g, sep)}`;
}

/** Builds a fresh project model with a single main program. */
export function createProjectModel(name: string, rootPath: string): PLCProject {
    const now = new Date().toISOString();
    const main: PLCProgram = {
        id: crypto.randomUUID(),
        name: "main",
        fileName: "main.st",
        language: "ST",
        path: "programs/main.st",
    };
    return {
        id: crypto.randomUUID(),
        name: name.trim(),
        version: PROJECT_VERSION,
        createdAt: now,
        updatedAt: now,
        rootPath,
        programs: [main],
    };
}

// --- project-relative paths (always `/`-separated) ---

export function baseName(path: string): string {
    return path.slice(path.lastIndexOf("/") + 1);
}

/** Parent folder of a path; "" for the project root. */
export function parentPath(path: string): string {
    const slash = path.lastIndexOf("/");
    return slash < 0 ? "" : path.slice(0, slash);
}

export function childPath(parent: string, name: string): string {
    return parent ? `${parent}/${name}` : name;
}

/** Whether `path` is `folder` itself or inside it. */
export function isWithin(path: string, folder: string): boolean {
    return path === folder || path.startsWith(`${folder}/`);
}

/** Rewrite `path` for a rename of `from` to `to` (a file, or a folder and its contents). */
export function remapPath(path: string, from: string, to: string): string {
    return isWithin(path, from) ? to + path.slice(from.length) : path;
}

/** Tabs showing a standard block's definition: not project files, never saved or edited. */
export const BUILTIN_PREFIX = "builtin:";

export function isBuiltinPath(path: string): boolean {
    return path.startsWith(BUILTIN_PREFIX);
}

export function isStFile(path: string): boolean {
    return /\.st$/i.test(path);
}

/** Every file path in a tree. */
export function filePaths(nodes: FileNode[]): string[] {
    return nodes.flatMap((n) => (n.isDir ? filePaths(n.children) : [n.path]));
}

// --- the program registry in project.json ---

/** Program entry for a `.st` file; its name is the file name without `.st`. */
export function programForPath(path: string, id: string = crypto.randomUUID()): PLCProgram {
    const fileName = baseName(path);
    return { id, name: fileName.replace(/\.st$/i, ""), fileName, language: "ST", path };
}

/**
 * Make the registry match the `.st` files on disk: keep entries (their ids and their
 * order, which is the execution order) whose file still exists, append new files, drop
 * deleted ones. Returns null if nothing changed.
 */
export function reconcilePrograms(programs: PLCProgram[], stPaths: string[]): PLCProgram[] | null {
    const onDisk = new Set(stPaths);
    const known = new Set(programs.map((p) => p.path));
    const next = [
        ...programs.filter((p) => onDisk.has(p.path)),
        ...stPaths.filter((path) => !known.has(path)).map((path) => programForPath(path)),
    ];
    const unchanged = next.length === programs.length && next.every((p, i) => p === programs[i]);
    return unchanged ? null : next;
}

/** Move the program at `index` by `delta` places (execution order); null if it can't move. */
export function movePrograms(programs: PLCProgram[], index: number, delta: number): PLCProgram[] | null {
    const to = index + delta;
    if (index < 0 || to < 0 || to >= programs.length) return null;
    const next = [...programs];
    const [moved] = next.splice(index, 1);
    next.splice(to, 0, moved);
    return next;
}

/** The source a program was last compiled from, and how many errors it had. */
export interface CompileRecord {
    source: string;
    errorCount: number;
}

export type CompileStatus = "Not Compiled" | "Compiling" | "Compiled" | "Error" | "Modified";

/**
 * A program's compilation state. `source` is its current text if it is open in the editor
 * (undefined otherwise: then it can't have changed in the IDE). Edited after compiling
 * means Modified: that result is stale and is never run.
 */
export function compileStatus(record: CompileRecord | undefined, source: string | undefined, compiling: boolean): CompileStatus {
    if (compiling) return "Compiling";
    if (!record) return "Not Compiled";
    if (source !== undefined && source !== record.source) return "Modified";
    return record.errorCount > 0 ? "Error" : "Compiled";
}

/** Apply a rename of `from` → `to` to the registry, keeping program ids. */
export function remapPrograms(programs: PLCProgram[], from: string, to: string): PLCProgram[] {
    return programs.map((p) => (isWithin(p.path, from) ? programForPath(remapPath(p.path, from, to), p.id) : p));
}

/** Serializes a project to its on-disk project.json form (drops runtime-only rootPath). */
export function toProjectFile(project: PLCProject): string {
    const { rootPath: _rootPath, ...file } = project;
    return JSON.stringify(file, null, 2);
}

/** Parses project.json content, attaching the known rootPath. */
export function fromProjectFile(json: string, rootPath: string): PLCProject {
    const file = JSON.parse(json) as ProjectFile;
    return { ...file, rootPath };
}

/** Editor language for a file: Monaco id plus the name shown in the status bar. */
export function languageFor(path: string): { id: string; label: string } {
    const ext = path.slice(path.lastIndexOf(".") + 1).toLowerCase();
    if (isStFile(path)) return { id: "st", label: "Structured Text" };
    if (ext === "json") return { id: "json", label: "JSON" };
    if (ext === "md") return { id: "markdown", label: "Markdown" };
    return { id: "plaintext", label: "Plain Text" };
}

export function findNode(nodes: FileNode[], path: string): FileNode | null {
    for (const node of nodes) {
        if (node.path === path) return node;
        if (node.isDir && isWithin(path, node.path)) return findNode(node.children, path);
    }
    return null;
}

/** Folder that new files and folders go into: the selected folder, or the selected file's folder. */
export function targetFolder(tree: FileNode[], selection: string | null): string {
    const node = selection === null ? null : findNode(tree, selection);
    if (!node) return "";
    return node.isDir ? node.path : parentPath(node.path);
}
