import { compilerTransport as transport } from "../utils/tauri";
import { call } from "./runtimeApi";
import { tagList } from "../utils/tags";
import type { FbSummary, IoMapping, ProgramSource } from "../types/runtime";

/** Compiler queries: nothing runs, no PLC state changes. */
export const compilerApi = {
    /** Every program's errors, breakable lines, declarations, and the I/O mapping errors. */
    compile: async (programs: ProgramSource[], mappings: IoMapping[]) => {
        // SCADA tags are valid names in programs, as on the backend.
        const tags = (await tagList()).map((t): [string, string] => [t.name, t.stType]);
        return call(() => transport.compile(programs, mappings, tags));
    },
    /** The standard function blocks (TON, CTU, …) with their interfaces. */
    standardFunctionBlocks: (): Promise<FbSummary[]> => call(transport.standardFunctionBlocks),
};
