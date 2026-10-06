/**
 * Mirrors the Rust compiler and I/O types (`compiler::error`, `compiler::project`, `io`).
 * The runtime's own messages and state are in `protocol.ts`.
 */

/** Mirrors Rust `compiler::error::ErrorKind`. */
export type ErrorKind =
    | "Syntax"
    | "UndeclaredVariable"
    | "DuplicateDeclaration"
    | "TypeMismatch"
    | "InvalidAssignment"
    | "InvalidOperator"
    | "InvalidCondition"
    | "InvalidFunction"
    | "InvalidFunctionArguments"
    | "InvalidFunctionBlock"
    | "InvalidFunctionBlockInput"
    | "InvalidFunctionBlockOutput"
    | "InvalidTimeValue"
    | "InvalidCase"
    | "InvalidLoop"
    | "InvalidIoMapping"
    | "Runtime"
    | "General";

export interface StError {
    kind: ErrorKind;
    /** 1-based; 0 when the error has no source position. */
    line: number;
    column: number;
    message: string;
}

export type DataType = "BOOL" | "INT" | "DINT" | "REAL" | "TIME";

export type IoKind = "DigitalInput" | "DigitalOutput" | "AnalogInput" | "AnalogOutput";

/** One I/O point: DI/DO hold a boolean, AI/AO a number (REAL). */
export interface IoPoint {
    address: string;
    kind: IoKind;
    value: boolean | number;
}

/** `variable` (declared by one program) is wired to `address`, e.g. StartButton → DI0. */
export interface IoMapping {
    address: string;
    variable: string;
}

/** One program as sent to the compiler: its name and current source. */
export interface ProgramSource {
    name: string;
    source: string;
}

/** A function block's interface (standard or from the project), for the library panel. */
export interface FbSummary {
    name: string;
    inputs: { name: string; dataType: string }[];
    outputs: { name: string; dataType: string }[];
    /** Line of `FUNCTION_BLOCK` in its file. */
    line: number;
    /** A standard block's read-only ST definition; null for the project's own blocks. */
    source: string | null;
}

/** Compilation result of one program: compiled if `errors` is empty. */
export interface CompileResult {
    name: string;
    errors: StError[];
    /** Lines where a statement starts: where a breakpoint can stop. */
    lines: number[];
    /** Declared variables (empty after a syntax error). */
    variables: { name: string; dataType: string }[];
    /** FUNCTION_BLOCKs the file defines. */
    functionBlocks: FbSummary[];
    /** The file has a PROGRAM; without one it only defines function block types. */
    hasProgram: boolean;
}

/** Compiling a project: one result per program, in order, plus I/O mapping errors. */
export interface CompileReport {
    programs: CompileResult[];
    mapping: StError[];
}
