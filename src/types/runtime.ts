/** Mirrors the Rust `simulator::state` and `st::error` types. */

export type RuntimeStatus = "STOPPED" | "RUNNING" | "PAUSED";

/** Mirrors Rust `st::error::ErrorKind`. */
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

export interface VariableSnapshot {
    /** The program that declares it. */
    program: string;
    name: string;
    dataType: DataType;
    /** BOOL → boolean; INT, DINT, REAL → number; TIME → milliseconds. */
    value: boolean | number;
    /** Never assigned by the program, so the simulator lets the user drive it. */
    isInput: boolean;
    /** The I/O address it is mapped to (it then follows that input / drives that output). */
    io: string | null;
}

export interface MemberSnapshot {
    name: string;
    dataType: DataType;
    value: boolean | number;
    /** Outputs (Q, ET, CV) are read-only; inputs (IN, PT, …) are set in calls. */
    isOutput: boolean;
    /** A user block's internal variable, shown read-only for debugging. */
    isInternal: boolean;
}

/** A function block instance, e.g. `RunTimer : TON`. */
export interface FunctionBlockSnapshot {
    program: string;
    name: string;
    blockType: string;
    members: MemberSnapshot[];
}

export interface RuntimeState {
    status: RuntimeStatus;
    /** The loaded programs, in execution order. */
    programs: string[];
    cycleCount: number;
    scanTimeMs: number;
    /** Plain variables. */
    variables: VariableSnapshot[];
    functionBlocks: FunctionBlockSnapshot[];
    /** Set when a scan failed and stopped the runtime. */
    error: StError | null;
    /** The program `error` happened in. */
    errorProgram: string | null;
    /** Simulated I/O: inputs as the user set them, outputs as the last scan wrote them. */
    io: IoPoint[];
    /** Debugger: the statement the PLC is paused before (null when paused between scans). */
    location: DebugLocation | null;
}

export interface DebugLocation {
    /** The program whose scan is running (the caller when inside a function block). */
    program: string;
    /** Source file of the statement (registry name): the program's or the block's. */
    file: string;
    line: number;
    column: number;
    /** Set when paused inside a user function block. */
    functionBlock: string | null;
    /** The instance running it, e.g. "Counter1". */
    instance: string | null;
}

/** A breakpoint on a program's source line. */
export interface Breakpoint {
    program: string;
    line: number;
}

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
