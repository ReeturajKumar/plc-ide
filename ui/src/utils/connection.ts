/**
 * Where the IDE connects to the PLC backend, and the token it uses. Kept on this machine
 * (localStorage) and editable in Settings, so the token is NOT baked into the installer —
 * a shared build carries no secret. The build may still set VITE_RUNTIME_URL (the backend
 * address is not a secret); VITE_RUNTIME_TOKEN, if set, is only a development fallback.
 */
const URL_KEY = "myplc.backendUrl";
const TOKEN_KEY = "myplc.backendToken";

const ENV_URL = (import.meta.env.VITE_RUNTIME_URL as string | undefined) || "";
const ENV_TOKEN = (import.meta.env.VITE_RUNTIME_TOKEN as string | undefined) || "";
const DEFAULT_URL = ENV_URL || "ws://127.0.0.1:5020";

export type Connection = { url: string; token: string };

/** The backend address and token to use now: saved values win, else the build-time defaults. */
export function getConnection(): Connection {
    let url = DEFAULT_URL;
    let token = ENV_TOKEN;
    try {
        url = localStorage.getItem(URL_KEY) || url;
        const saved = localStorage.getItem(TOKEN_KEY);
        if (saved !== null) token = saved;
    } catch {
        /* private mode / storage blocked: fall back to the build-time defaults */
    }
    return { url, token };
}

/** Save the backend address and token on this computer (never in the shared app). */
export function setConnection({ url, token }: Connection): void {
    try {
        localStorage.setItem(URL_KEY, url.trim());
        localStorage.setItem(TOKEN_KEY, token.trim());
    } catch {
        /* ignore: can't persist, but the entered values aren't lost until reload */
    }
}

/** Whether a connection has been set up here (or a token was baked in for development). */
export function backendConfigured(): boolean {
    if (ENV_TOKEN) return true;
    try {
        return localStorage.getItem(URL_KEY) !== null;
    } catch {
        return false;
    }
}
