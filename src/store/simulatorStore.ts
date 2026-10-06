import { create } from "zustand";
import type { RuntimeState } from "../types/runtime";
import type { Problem } from "./consoleStore";

export const STOPPED_STATE: RuntimeState = {
    status: "STOPPED",
    programs: [],
    cycleCount: 0,
    scanTimeMs: 100,
    variables: [],
    functionBlocks: [],
    error: null,
    errorProgram: null,
    io: [],
    location: null,
};

interface SimulatorState {
    runtime: RuntimeState;
    /** Errors that stopped (or prevented) the run: every compile error, or one scan error. */
    errors: Problem[];

    setRuntime: (runtime: RuntimeState) => void;
    setErrors: (errors: Problem[]) => void;
}

export const useSimulatorStore = create<SimulatorState>((set) => ({
    runtime: STOPPED_STATE,
    errors: [],

    setRuntime: (runtime) => set({ runtime }),
    setErrors: (errors) => set({ errors }),
}));
