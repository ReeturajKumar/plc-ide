import { create } from "zustand";
import type { FileNode, PLCProject } from "../types/project";
import type { PLCProgram } from "../types/program";
import type { FbSummary, IoMapping } from "../types/runtime";
import type { CompileRecord } from "../utils/project";

interface ProjectState {
    project: PLCProject | null;
    /** The project folder as last read from disk. */
    tree: FileNode[];
    /** Program registry changes not yet written to project.json. */
    dirty: boolean;
    /** Last compilation of each program, by path. Session-only. */
    compiled: Record<string, CompileRecord>;
    /** Paths of the programs being compiled right now. */
    compiling: string[];
    /** Program path → lines where a statement starts, from the last compile/check. */
    breakableLines: Record<string, number[]>;
    /** What each .st file contains, from the last compile/check. */
    fileKinds: Record<string, FileKind>;

    setProject: (project: PLCProject) => void;
    closeProject: () => void;
    markSaved: () => void;
    setTree: (tree: FileNode[]) => void;
    /** Replace the program registry (kept in sync with the .st files on disk). Its order is the execution order. */
    setPrograms: (programs: PLCProgram[]) => void;
    /** Replace the I/O mappings (saved in project.json). */
    setIoMappings: (mappings: IoMapping[]) => void;
    /** Take I/O mappings and breakpoints from project.json edited outside the IDE (already saved). */
    adoptExternal: (changes: Pick<PLCProject, "io" | "breakpoints">) => void;
    setCompiled: (path: string, record: CompileRecord) => void;
    setCompiling: (paths: string[]) => void;
    /** Replace one file's breakpoints (saved in project.json). */
    setBreakpoints: (path: string, lines: number[]) => void;
    setBreakableLines: (path: string, lines: number[]) => void;
    setFileKind: (path: string, kind: FileKind) => void;
}

/** A .st file defines FUNCTION_BLOCKs and/or a PROGRAM (only programs run in the scan). */
export interface FileKind {
    hasProgram: boolean;
    functionBlocks: FbSummary[];
}

const fresh = { tree: [], dirty: false, compiled: {}, compiling: [], breakableLines: {}, fileKinds: {} };

export const useProjectStore = create<ProjectState>((set) => ({
    project: null,
    ...fresh,

    setProject: (project) => set({ project, ...fresh }),
    closeProject: () => set({ project: null, ...fresh }),
    markSaved: () =>
        set((s) =>
            s.project ? { project: { ...s.project, updatedAt: new Date().toISOString() }, dirty: false } : s
        ),
    setTree: (tree) => set({ tree }),
    setPrograms: (programs) => set((s) => (s.project ? { project: { ...s.project, programs }, dirty: true } : s)),
    setIoMappings: (mappings) => set((s) => (s.project ? { project: { ...s.project, io: { mappings } }, dirty: true } : s)),
    adoptExternal: (changes) => set((s) => (s.project ? { project: { ...s.project, ...changes } } : s)),
    setCompiled: (path, record) => set((s) => ({ compiled: { ...s.compiled, [path]: record } })),
    setCompiling: (compiling) => set({ compiling }),
    setBreakpoints: (path, lines) =>
        set((s) =>
            s.project ? { project: { ...s.project, breakpoints: { ...s.project.breakpoints, [path]: lines } }, dirty: true } : s
        ),
    setBreakableLines: (path, lines) => set((s) => ({ breakableLines: { ...s.breakableLines, [path]: lines } })),
    setFileKind: (path, kind) => set((s) => ({ fileKinds: { ...s.fileKinds, [path]: kind } })),
}));
