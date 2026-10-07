import { runtimeTransport as transport } from "../utils/tauri";
import { call } from "./runtimeApi";
import type { FbSummary, IoMapping, ProgramSource } from "../types/runtime";

/** Compiler queries: nothing runs, no PLC state changes. */
export const compilerApi = {
    /** Every program's errors, breakable lines, declarations, and the I/O mapping errors. */
    compile: (programs: ProgramSource[], mappings: IoMapping[]) => call(() => transport.compile(programs, mappings)),
    /** The standard function blocks (TON, CTU, …) with their interfaces. */
    standardFunctionBlocks: (): Promise<FbSummary[]> => call(transport.standardFunctionBlocks),
};
