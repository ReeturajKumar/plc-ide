import type { PLCProgram } from "./program";
import type { IoMapping } from "./runtime";

/** A MyPLC project. Persisted (minus rootPath) as project.json at the project root. */
export interface PLCProject {
    id: string;
    name: string;
    version: string;
    createdAt: string;
    updatedAt: string;
    /** Absolute path to the project folder. Runtime-only; not written to project.json. */
    rootPath: string;
    programs: PLCProgram[];
    /** Variables wired to simulated I/O. Missing in projects made before I/O existed. */
    io?: { mappings: IoMapping[] };
    /** Debugger breakpoints: program file path → lines. */
    breakpoints?: Record<string, number[]>;
}

/** Shape of project.json on disk (rootPath is derived from the folder, not stored). */
export type ProjectFile = Omit<PLCProject, "rootPath">;

/** A file or folder in the project, as listed from disk. */
export interface FileNode {
    name: string;
    /** Relative to the project root, with `/` separators. */
    path: string;
    isDir: boolean;
    /** Folders first, then files, sorted by name. Empty for files. */
    children: FileNode[];
}
