import type { UnlistenFn } from "@tauri-apps/api/event";
import { runtimeEvents, runtimeTransport as transport } from "../utils/tauri";
import type { CompileReport, FbSummary, IoMapping, ProgramSource, StError } from "../types/runtime";
import type { Breakpoint, Command, ProtocolError, RuntimeState, ScanError } from "../types/protocol";

/**
 * The UI's only way to the PLC: user intents in, PLC state (or a `RuntimeApiError`) out.
 * Each intent is one or more protocol requests (`types/protocol.ts`); components and
 * stores never see the protocol or the transport (today one Tauri command and events).
 */

/** Why a request failed, whatever the transport: the protocol's error codes. */
export type RuntimeErrorCode = ProtocolError["code"];

/** What each code carries in `details` (see `types/protocol.ts`). */
type ErrorDetails = CompileReport | ScanError | StError[];

export class RuntimeApiError extends Error {
    constructor(
        readonly code: RuntimeErrorCode,
        message: string,
        readonly details?: ErrorDetails,
    ) {
        super(message);
        this.name = "RuntimeApiError";
    }

    /** The error as one StError, when it is one (a scan failure). */
    get stError(): StError | undefined {
        return this.code === "RUNTIME_ERROR" ? (this.details as ScanError).error : undefined;
    }
}

const CODES: readonly RuntimeErrorCode[] = ["COMPILATION_ERROR", "RUNTIME_ERROR", "IO_MAPPING_ERROR", "REJECTED", "UNAVAILABLE"];

function isProtocolError(e: unknown): e is ProtocolError {
    return (
        typeof e === "object" &&
        e !== null &&
        "code" in e &&
        CODES.includes(e.code as RuntimeErrorCode) &&
        "message" in e &&
        typeof e.message === "string"
    );
}

/** A protocol error, or whatever the transport failed with, as a `RuntimeApiError`. */
export function toRuntimeError(e: unknown): RuntimeApiError {
    if (e instanceof RuntimeApiError) return e;
    if (isProtocolError(e)) return new RuntimeApiError(e.code, e.message, "details" in e ? e.details : undefined);
    console.error("[runtime]", e);
    return new RuntimeApiError("UNAVAILABLE", "The PLC runtime is not available.");
}

async function call<T>(request: () => Promise<T>): Promise<T> {
    try {
        return await request();
    } catch (e) {
        throw toRuntimeError(e);
    }
}

let lastId = 0;

/** One protocol request; resolves with the state it left the PLC in. */
async function send(command: Command): Promise<RuntimeState> {
    const response = await call(() => transport.request({ id: `ui-${++lastId}`, ...command }));
    if (!response.success) throw toRuntimeError(response.error);
    return response.data.state;
}

/** RUN as the IDE knows it: compile and load the project, then start it. */
async function loadAndRun(programs: ProgramSource[], mappings: IoMapping[], paused: boolean): Promise<RuntimeState> {
    await send({ command: "LOAD_PROJECT", payload: { programs, mappings } });
    return send({ command: "RUN", payload: { paused } });
}

export const runtimeApi = {
    /** Compile the project and start scanning, all programs in order each scan. */
    run: (programs: ProgramSource[], mappings: IoMapping[]) => loadAndRun(programs, mappings, false),
    /** Compile the project and load it paused: no scan runs until STEP or RESUME. */
    load: (programs: ProgramSource[], mappings: IoMapping[]) => loadAndRun(programs, mappings, true),
    stop: () => send({ command: "STOP" }),
    pause: () => send({ command: "PAUSE" }),
    resume: () => send({ command: "RESUME" }),
    /** While paused: finish the stopped scan, or run one complete scan. */
    stepScan: () => send({ command: "STEP_SCAN" }),
    /** While paused: run the next statement and stop before the one after. */
    stepStatement: () => send({ command: "STEP_STATEMENT" }),
    /** Simulated inputs, by address ("DI0", "AI1"); they work whether or not the PLC runs. */
    setDigitalInput: (address: string, value: boolean) => send({ command: "SET_INPUT", payload: { address, value } }),
    setAnalogInput: (address: string, value: number) => send({ command: "SET_INPUT", payload: { address, value } }),
    /** Set a BOOL variable a program declares. */
    setVariable: (program: string, name: string, value: boolean) =>
        send({ command: "SET_VARIABLE", payload: { program, name, value } }),
    /** Replace every breakpoint: the UI keeps the list, the runtime does the stopping. */
    setBreakpoints: (breakpoints: Breakpoint[]) => send({ command: "SET_BREAKPOINTS", payload: { breakpoints } }),
    /** Re-map the I/O of the loaded PLC from its next scan; resolves with the mapping errors. */
    applyIoMappings: async (mappings: IoMapping[]): Promise<StError[]> => {
        try {
            await send({ command: "APPLY_IO_MAPPINGS", payload: { mappings } });
            return [];
        } catch (e) {
            const error = toRuntimeError(e);
            if (error.code === "IO_MAPPING_ERROR") return error.details as StError[];
            throw error;
        }
    },
    getState: () => send({ command: "GET_STATE" }),
};

/** Compiler queries: nothing runs, no PLC state changes. */
export const compilerApi = {
    /** Every program's errors, breakable lines, declarations, and the I/O mapping errors. */
    compile: (programs: ProgramSource[], mappings: IoMapping[]) => call(() => transport.compile(programs, mappings)),
    /** The standard function blocks (TON, CTU, …) with their interfaces. */
    standardFunctionBlocks: (): Promise<FbSummary[]> => call(transport.standardFunctionBlocks),
};

type OnState = (state: RuntimeState) => void;
type OnError = (error: RuntimeApiError) => void;

/** Everyone watching, by their state callback (so watching twice is one watch). */
const watchers = new Map<OnState, OnError>();
/** The one shared subscription to the runtime's events, while anyone watches. */
let listening: Promise<UnlistenFn[]> | null = null;
/** Counts state events, so a slower initial GET_STATE never overwrites a newer event. */
let received = 0;

function publish(state: RuntimeState): void {
    received++;
    for (const onState of [...watchers.keys()]) onState(state);
}

/** The runtime is gone: every watch ends, and every watcher hears why. */
function lose(error: RuntimeApiError): void {
    const onErrors = [...watchers.values()];
    runtimeState.unwatch();
    for (const onError of onErrors) onError(error);
}

/**
 * Live PLC state, pushed by the runtime as it changes (no polling), independent of how it
 * arrives. Any number of watchers share one subscription.
 */
export const runtimeState = {
    /**
     * Deliver the state to `onState`: the current one first, then every change, until
     * `unwatch`. `onError` hears when the runtime is lost, which ends every watch.
     */
    watch(onState: OnState, onError: OnError): void {
        if (watchers.has(onState)) return;
        watchers.set(onState, onError);
        if (!listening) {
            const subscribing = Promise.all([
                runtimeEvents.onState(publish),
                runtimeEvents.onUnavailable((error) => lose(toRuntimeError(error))),
            ]);
            listening = subscribing;
            subscribing.catch((e) => {
                if (listening === subscribing) lose(toRuntimeError(e));
            });
        }
        // Events report changes: the current state comes from one GET_STATE, which the
        // IDE answers from the latest state it has.
        const before = received;
        void listening
            .then(() => runtimeApi.getState())
            .then(
                (state) => {
                    if (watchers.get(onState) === onError && received === before) onState(state);
                },
                (e) => {
                    if (watchers.get(onState) === onError) lose(toRuntimeError(e));
                },
            );
    },
    /** Stop delivering to `onState`, or to everyone. The subscription ends with the last watcher. */
    unwatch(onState?: OnState): void {
        if (onState) watchers.delete(onState);
        else watchers.clear();
        if (watchers.size > 0 || !listening) return;
        const subscription = listening;
        listening = null;
        subscription.then((unlisten) => unlisten.forEach((stop) => stop())).catch(() => {});
    },
};
