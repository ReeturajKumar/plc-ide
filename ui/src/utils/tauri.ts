import { invoke } from "@tauri-apps/api/core";
import type { CompileReport, FbSummary, IoMapping, ProgramSource } from "../types/runtime";
import type { FileNode } from "../types/project";

/** The compiler queries, answered by the desktop app (nothing runs). */
export const compilerTransport = {
    /** Compile without running: every program's errors and the I/O mapping errors. */
    compile: (programs: ProgramSource[], mappings: IoMapping[]) =>
        invoke<CompileReport>("compile_programs", { programs, mappings }),
    /** The standard function blocks (TON, CTU, …) with their inputs and outputs. */
    standardFunctionBlocks: () => invoke<FbSummary[]>("standard_function_blocks"),
};

/** Typed wrappers over the Rust filesystem commands. Errors are already friendly strings. */
export const fsApi = {
    createProject: (rootPath: string, projectJson: string, mainContent: string) =>
        invoke<void>("create_project", { rootPath, projectJson, mainContent }),

    readProject: (rootPath: string) => invoke<string>("read_project", { rootPath }),

    writeProjectJson: (rootPath: string, projectJson: string) =>
        invoke<void>("write_project_json", { rootPath, projectJson }),

    readFile: (rootPath: string, relPath: string) =>
        invoke<string>("read_file", { rootPath, relPath }),

    writeFile: (rootPath: string, relPath: string, content: string) =>
        invoke<void>("write_file", { rootPath, relPath, content }),

    /** Create a file; fails rather than overwrite. */
    createFile: (rootPath: string, relPath: string, content: string) =>
        invoke<void>("create_file", { rootPath, relPath, content }),

    createDir: (rootPath: string, relPath: string) => invoke<void>("create_dir", { rootPath, relPath }),

    /** The whole project folder, folders first. */
    readTree: (rootPath: string) => invoke<FileNode[]>("read_tree", { rootPath }),

    /** Rename a file or folder (folders move with their contents). */
    renameFile: (rootPath: string, oldRelPath: string, newRelPath: string) =>
        invoke<void>("rename_file", { rootPath, oldRelPath, newRelPath }),

    /** Delete a file, or a folder with everything in it. */
    deleteEntry: (rootPath: string, relPath: string) => invoke<void>("delete_entry", { rootPath, relPath }),
};
