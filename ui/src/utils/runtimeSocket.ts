import type { Request, Response, RuntimeMessage, RuntimeState } from "../types/protocol";

/**
 * The connection to the PLC backend (`myplc-runtime --listen`): one WebSocket at
 * `VITE_RUNTIME_URL` (from `.env`), carrying protocol messages as JSON. Requests get the
 * response with the same id; `STATE_UPDATE`s arrive on their own. It connects on the first
 * request and again after the connection is lost. Only `services/runtimeApi` uses it.
 */

const RUNTIME_URL: string = import.meta.env.VITE_RUNTIME_URL || "ws://127.0.0.1:5020";
const RUNTIME_TOKEN: string | undefined = import.meta.env.VITE_RUNTIME_TOKEN || undefined;
/** The longest a request may take (LOAD_PROJECT compiles the whole project). */
const REQUEST_TIMEOUT_MS = 30_000;

type Pending = { resolve: (response: Response) => void; reject: (error: unknown) => void };

let socket: WebSocket | null = null;
let connecting: Promise<WebSocket> | null = null;
const pending = new Map<string, Pending>();
const stateHandlers = new Set<(state: RuntimeState) => void>();
const unavailableHandlers = new Set<(error: unknown) => void>();

const unavailable = (message: string) => ({ code: "UNAVAILABLE", message });

function connect(): Promise<WebSocket> {
    if (socket?.readyState === WebSocket.OPEN) return Promise.resolve(socket);
    if (connecting) return connecting;
    connecting = new Promise((resolve, reject) => {
        const url = RUNTIME_TOKEN ? `${RUNTIME_URL}?token=${encodeURIComponent(RUNTIME_TOKEN)}` : RUNTIME_URL;
        const ws = new WebSocket(url);
        ws.onopen = () => {
            socket = ws;
            connecting = null;
            resolve(ws);
        };
        ws.onmessage = (event) => {
            const message = JSON.parse(event.data as string) as RuntimeMessage;
            if (message.type === "STATE_UPDATE") {
                stateHandlers.forEach((handler) => handler(message.state));
            } else if (message.id !== null) {
                pending.get(message.id)?.resolve(message);
                pending.delete(message.id);
            }
        };
        ws.onclose = () => {
            const wasOpen = socket === ws;
            socket = null;
            connecting = null;
            if (!wasOpen) {
                reject(unavailable(`The PLC runtime can't be reached at ${RUNTIME_URL}. Check that it is running and that VITE_RUNTIME_URL and VITE_RUNTIME_TOKEN are right.`));
                return;
            }
            const lost = unavailable("The connection to the PLC runtime was lost.");
            pending.forEach(({ reject: fail }) => fail(lost));
            pending.clear();
            unavailableHandlers.forEach((handler) => handler(lost));
        };
    });
    return connecting;
}

/** Protocol requests to the backend. */
export const runtimeTransport = {
    /** One protocol request; resolves with its protocol response. */
    request: async (request: Request): Promise<Response> => {
        const ws = await connect();
        return new Promise<Response>((resolve, reject) => {
            const timer = setTimeout(() => {
                pending.delete(request.id);
                reject(unavailable("The PLC runtime is not responding."));
            }, REQUEST_TIMEOUT_MS);
            pending.set(request.id, {
                resolve: (response) => (clearTimeout(timer), resolve(response)),
                reject: (error) => (clearTimeout(timer), reject(error)),
            });
            ws.send(JSON.stringify(request));
        });
    },
};

/**
 * What the backend pushes without being asked: every PLC state change, and the loss of the
 * connection (`{ code: "UNAVAILABLE", message }`). Each returns a function that unsubscribes.
 */
export const runtimeEvents = {
    onState: async (handler: (state: RuntimeState) => void): Promise<() => void> => {
        stateHandlers.add(handler);
        return () => void stateHandlers.delete(handler);
    },
    onUnavailable: async (handler: (error: unknown) => void): Promise<() => void> => {
        unavailableHandlers.add(handler);
        return () => void unavailableHandlers.delete(handler);
    },
};
