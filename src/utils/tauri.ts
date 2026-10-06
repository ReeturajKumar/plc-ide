import { invoke } from "@tauri-apps/api/core";
import type { Breakpoint, CompileReport, FbSummary, IoMapping, ProgramSource, RuntimeState, StError } from "../types/runtime";
import type { FileNode } from "../types/project";

/** Typed wrappers over the Rust PLC simulator commands. Errors reject with an `StError`. */
export const runtimeApi = {
    /**
     * Compile every program, check the I/O mappings and run all programs each scan, in
     * order (or load them paused, for STEP). Rejects with a `CompileReport` on any error.
     */
    start: (programs: ProgramSource[], mappings: IoMapping[], paused = false, scanTimeMs?: number) =>
        invoke<RuntimeState>("start_program", { programs, mappings, scanTimeMs, paused }),
    /** Compile without running: every program's errors and the I/O mapping errors. */
    compile: (programs: ProgramSource[], mappings: IoMapping[]) =>
        invoke<CompileReport>("compile_programs", { programs, mappings }),
    /** Apply new I/O mappings to the loaded programs right away; returns the mapping errors. */
    applyIoMappings: (mappings: IoMapping[]) => invoke<StError[]>("apply_io_mappings", { mappings }),
    /** Set a simulated input: DI true/false, AI a number. Works whether or not the PLC runs. */
    setIoInput: (address: string, value: boolean | number) => invoke<RuntimeState>("set_io_input", { address, value }),
    /** The standard function blocks (TON, CTU, …) with their inputs and outputs. */
    standardFunctionBlocks: () => invoke<FbSummary[]>("standard_function_blocks"),
    /** Execute exactly one scan of every program; only while paused. */
    step: () => invoke<RuntimeState>("step_program"),
    /** Debugger: run the next statement while paused, then stop again. */
    stepStatement: () => invoke<RuntimeState>("step_statement"),
    /** Debugger: replace every breakpoint. */
    setBreakpoints: (breakpoints: Breakpoint[]) => invoke<RuntimeState>("set_breakpoints", { breakpoints }),
    stop: () => invoke<RuntimeState>("stop_program"),
    pause: () => invoke<RuntimeState>("pause_program"),
    resume: () => invoke<RuntimeState>("resume_program"),
    getState: () => invoke<RuntimeState>("get_runtime_state"),
    /** Set a BOOL variable declared by `program`. */
    setInput: (program: string, name: string, value: boolean) =>
        invoke<RuntimeState>("set_input", { program, name, value }),
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
