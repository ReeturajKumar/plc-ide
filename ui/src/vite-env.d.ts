/// <reference types="vite/client" />

interface ImportMetaEnv {
    /** The PLC backend, e.g. ws://192.168.1.50:5020 (see .env.example). */
    readonly VITE_RUNTIME_URL?: string;
    /** The backend's token, if it was started with one. */
    readonly VITE_RUNTIME_TOKEN?: string;
}
