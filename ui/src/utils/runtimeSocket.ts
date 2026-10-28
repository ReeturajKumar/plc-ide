import type { Request, Response, RuntimeMessage, RuntimeState } from "../types/protocol";
import { getConnection } from "./connection";

/**
 * The connection to the PLC backend (`myplc-runtime --listen`): one WebSocket to the
 * address and token from `connection` (saved on this machine, set in Settings), carrying
 * protocol messages as JSON. Requests get the response with the same id; `STATE_UPDATE`s
 * arrive on their own. It connects on the first request and again after the connection is
 * lost. Only `services/runtimeApi` uses it.
 */

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
    const { url: base, token } = getConnection();
    connecting = new Promise((resolve, reject) => {
        const url = token ? `${base}?token=${encodeURIComponent(token)}` : base;
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
                reject(unavailable(`The PLC runtime can't be reached at ${base}. Check that it is running and that the backend address and token in Settings are right.`));
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

/** Drop the connection so the next request reconnects — e.g. after the backend settings change. */
export function reconnect(): void {
    const old = socket;
    socket = null;
    connecting = null;
    old?.close();
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
