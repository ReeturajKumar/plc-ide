/**
 * The MyPLC UI ↔ runtime protocol, transport independent. Mirrors the Rust module
 * `myplc_core::protocol` (server/core/src/protocol/mod.rs), which is the source of
 * truth: change that first, then this file to match.
 *
 * This is how the IDE talks to the PLC backend: `runtimeApi` sends `Request`s over the
 * WebSocket (`utils/runtimeSocket.ts`) and receives `RuntimeState` in responses and events.
 */
import type { CompileReport, DataType, IoMapping, IoPoint, ProgramSource, StError } from "./runtime";

export type RequestId = string;

// ---------------------------------------------------------------- UI → runtime

/** A command with the id its response will carry. */
export type Request = { id: RequestId } & Command;

export type Command =
    | { command: "LOAD_PROJECT"; payload: { programs: ProgramSource[]; mappings?: IoMapping[]; scanTimeMs?: number } }
    | { command: "RUN"; payload?: { paused?: boolean } }
    | { command: "STOP" }
    | { command: "PAUSE" }
    | { command: "RESUME" }
    | { command: "STEP_SCAN" }
    | { command: "STEP_STATEMENT" }
    | { command: "SET_INPUT"; payload: { address: string; value: boolean | number } }
    | { command: "SET_VARIABLE"; payload: { program: string; name: string; value: boolean } }
    | { command: "SET_BREAKPOINTS"; payload: { breakpoints: Breakpoint[] } }
    | { command: "APPLY_IO_MAPPINGS"; payload: { mappings: IoMapping[] } }
    | { command: "GET_STATE" };

/** `file` is the source file's name in the project, as `Location.file` reports it. */
export interface Breakpoint {
    file: string;
    line: number;
}

// ---------------------------------------------------------------- runtime → UI

/** Anything the runtime sends: only a RESPONSE answers a request. */
export type RuntimeMessage = ({ type: "RESPONSE" } & Response) | { type: "STATE_UPDATE"; state: RuntimeState };

export type Response =
    | { id: RequestId | null; success: true; data: { state: RuntimeState } }
    | { id: RequestId | null; success: false; error: ProtocolError };

export type ProtocolError = { message: string } & (
    | { code: "COMPILATION_ERROR"; details: CompileReport }
    | { code: "RUNTIME_ERROR"; details: ScanError }
    | { code: "IO_MAPPING_ERROR"; details: StError[] }
    | { code: "REJECTED" }
    | { code: "UNAVAILABLE" }
);

// ---------------------------------------------------------------- state

export type RuntimeStatus = "STOPPED" | "RUNNING" | "PAUSED";

export interface RuntimeState {
    status: RuntimeStatus;
    programs: string[];
    cycleCount: number;
    scanTimeMs: number;
    variables: Variable[];
    functionBlocks: FunctionBlock[];
    io: IoPoint[];
    debugger: DebuggerState;
    error: ScanError | null;
}

export interface Variable {
    program: string;
    name: string;
    dataType: DataType;
    value: boolean | number;
    /** Some program assigns it; otherwise its value only changes from outside. */
    writtenByProgram: boolean;
    ioAddress: string | null;
}

export interface FunctionBlock {
    program: string;
    instance: string;
    typeName: string;
    standard: boolean;
    inputs: Member[];
    outputs: Member[];
    internals: Member[];
}

export interface Member {
    name: string;
    dataType: DataType;
    value: boolean | number;
}

export interface DebuggerState {
    location: Location | null;
    breakpoints: Breakpoint[];
}

export interface Location {
    program: string;
    file: string;
    line: number;
    column: number;
    functionBlock: string | null;
    instance: string | null;
}

export interface ScanError {
    program: string;
    error: StError;
}
