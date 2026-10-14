import { create } from "zustand";
import type { Variable } from "../types/protocol";

export interface Notification {
    id: string;
    kind: "error" | "info" | "success";
    message: string;
}

export type DialogKind = "newProject" | "settings" | null;

/** An inline name edit in the Explorer: a new program, or renaming an existing one. */
export type ExplorerEdit =
    | { mode: "create"; kind: "file" | "folder"; parent: string }
    | { mode: "rename"; path: string };
export type ConfirmChoice = "save" | "discard" | "cancel";

/** Values the user gave the program's inputs before RUN. */
export type InputValues = [Variable, boolean | number | string][];

interface ConfirmPrompt {
    title: string;
    message: string;
}

export const EXPLORER_MIN = 180;
export const EXPLORER_MAX = 480;
export const PANEL_MIN = 100;
export const PANEL_MAX = 500;
const EXPLORER_KEY = "myplc.explorerWidth";
const PANEL_KEY = "myplc.panelHeight";
const WRAP_KEY = "myplc.wordWrap";
const MINIMAP_KEY = "myplc.minimap";
const AUTOSAVE_KEY = "myplc.autoSave";
const AUTOSAVE_DELAY_KEY = "myplc.autoSaveDelay";

/** Auto save delay limits, in milliseconds. */
export const AUTOSAVE_DELAY_MIN = 200;
export const AUTOSAVE_DELAY_MAX = 600_000;
export const clampAutoSaveDelay = (ms: number) => Math.round(Math.max(AUTOSAVE_DELAY_MIN, Math.min(AUTOSAVE_DELAY_MAX, ms)));

const readFlag = (key: string, fallback: boolean) => {
    try {
        const stored = localStorage.getItem(key);
        return stored === null ? fallback : stored === "1";
    } catch {
        return fallback;
    }
};
const storeFlag = (key: string, value: boolean) => {
    try { localStorage.setItem(key, value ? "1" : "0"); } catch { /* ignore */ }
};

export type PanelTab = "simulator" | "io" | "problems" | "output" | "terminal" | "debug";

const clampWidth = (w: number) => Math.max(EXPLORER_MIN, Math.min(EXPLORER_MAX, w));
const clampHeight = (h: number) => Math.max(PANEL_MIN, Math.min(PANEL_MAX, h));
const readStored = (key: string, fallback: number, clamp: (n: number) => number) => {
    try {
        const stored = Number(localStorage.getItem(key));
        return stored ? clamp(stored) : fallback;
    } catch {
        return fallback;
    }
};

interface UIState {
    sidebarVisible: boolean;
    explorerWidth: number;
    panelVisible: boolean;
    panelHeight: number;
    /** The panel fills the editor area (VS Code's Maximize Panel). */
    panelMaximized: boolean;
    panelTab: PanelTab;
    /** Editor view options, remembered per machine. */
    wordWrap: boolean;
    minimap: boolean;
    /** Save changed files by themselves, `autoSaveDelay` ms after the last edit. */
    autoSave: boolean;
    autoSaveDelay: number;
    activeDialog: DialogKind;
    explorerEdit: ExplorerEdit | null;
    /** Path of the selected Explorer item; new files and folders go next to or into it. */
    explorerSelection: string | null;
    notifications: Notification[];
    confirmPrompt: ConfirmPrompt | null;

    toggleSidebar: () => void;
    setSidebarVisible: (visible: boolean) => void;
    setExplorerWidth: (width: number) => void;
    togglePanel: () => void;
    togglePanelMaximized: () => void;
    toggleWordWrap: () => void;
    toggleMinimap: () => void;
    setAutoSave: (on: boolean) => void;
    setAutoSaveDelay: (ms: number) => void;
    setPanelHeight: (height: number) => void;
    setPanelTab: (tab: PanelTab) => void;
    openDialog: (dialog: DialogKind) => void;
    /** Start an inline edit in the Explorer (showing it if hidden). */
    startExplorerEdit: (edit: ExplorerEdit) => void;
    endExplorerEdit: () => void;
    setExplorerSelection: (path: string | null) => void;
    closeDialog: () => void;

    notify: (kind: Notification["kind"], message: string) => void;
    dismiss: (id: string) => void;

    /** Show a Save / Don't Save / Cancel prompt; resolves with the user's choice. */
    confirmUnsaved: (prompt: ConfirmPrompt) => Promise<ConfirmChoice>;
    resolveConfirm: (choice: ConfirmChoice) => void;

    /** The inputs asked for before RUN (see InputsDialog), if that prompt is open. */
    inputsPrompt: Variable[] | null;
    /** Ask for values of `inputs`; resolves with them ([] to keep the current ones), or null if cancelled. */
    askInputs: (inputs: Variable[]) => Promise<InputValues | null>;
    resolveInputs: (values: InputValues | null) => void;
}

// Resolver for the active confirm prompt, kept out of state so it isn't serialized/rendered.
let confirmResolver: ((choice: ConfirmChoice) => void) | null = null;
let inputsResolver: ((values: InputValues | null) => void) | null = null;

export const useUIStore = create<UIState>((set) => ({
    sidebarVisible: true,
    explorerWidth: readStored(EXPLORER_KEY, 256, clampWidth),
    panelVisible: true,
    panelHeight: readStored(PANEL_KEY, 180, clampHeight),
    panelMaximized: false,
    panelTab: "problems",
    wordWrap: readFlag(WRAP_KEY, false),
    minimap: readFlag(MINIMAP_KEY, true),
    autoSave: readFlag(AUTOSAVE_KEY, false),
    autoSaveDelay: readStored(AUTOSAVE_DELAY_KEY, 1000, clampAutoSaveDelay),
    activeDialog: null,
    explorerEdit: null,
    explorerSelection: null,
    notifications: [],
    confirmPrompt: null,

    // Hiding the Explorer mid-edit cancels the edit, as in VS Code.
    toggleSidebar: () =>
        set((s) => (s.sidebarVisible ? { sidebarVisible: false, explorerEdit: null } : { sidebarVisible: true })),
    setSidebarVisible: (visible) => set(visible ? { sidebarVisible: true } : { sidebarVisible: false, explorerEdit: null }),
    setExplorerWidth: (width) => {
        const w = clampWidth(width);
        try { localStorage.setItem(EXPLORER_KEY, String(w)); } catch { /* ignore */ }
        set({ explorerWidth: w });
    },
    // Hiding a maximized panel restores it, so it reopens at its normal height.
    togglePanel: () => set((s) => ({ panelVisible: !s.panelVisible, panelMaximized: false })),
    togglePanelMaximized: () => set((s) => ({ panelMaximized: !s.panelMaximized, panelVisible: true })),
    toggleWordWrap: () =>
        set((s) => {
            storeFlag(WRAP_KEY, !s.wordWrap);
            return { wordWrap: !s.wordWrap };
        }),
    toggleMinimap: () =>
        set((s) => {
            storeFlag(MINIMAP_KEY, !s.minimap);
            return { minimap: !s.minimap };
        }),
    setAutoSave: (on) => {
        storeFlag(AUTOSAVE_KEY, on);
        set({ autoSave: on });
    },
    setAutoSaveDelay: (ms) => {
        const delay = clampAutoSaveDelay(ms);
        try { localStorage.setItem(AUTOSAVE_DELAY_KEY, String(delay)); } catch { /* ignore */ }
        set({ autoSaveDelay: delay });
    },
    setPanelHeight: (height) => {
        const h = clampHeight(height);
        try { localStorage.setItem(PANEL_KEY, String(h)); } catch { /* ignore */ }
        set({ panelHeight: h });
    },
    setPanelTab: (tab) => set({ panelTab: tab, panelVisible: true }),
    openDialog: (dialog) => set({ activeDialog: dialog }),
    startExplorerEdit: (edit) => set({ explorerEdit: edit, sidebarVisible: true }),
    endExplorerEdit: () => set({ explorerEdit: null }),
    setExplorerSelection: (path) => set({ explorerSelection: path }),
    closeDialog: () => set({ activeDialog: null }),

    notify: (kind, message) =>
        set((s) => ({
            notifications: [...s.notifications, { id: crypto.randomUUID(), kind, message }],
        })),
    dismiss: (id) => set((s) => ({ notifications: s.notifications.filter((n) => n.id !== id) })),

    confirmUnsaved: (prompt) =>
        new Promise<ConfirmChoice>((resolve) => {
            confirmResolver = resolve;
            set({ confirmPrompt: prompt });
        }),
    resolveConfirm: (choice) => {
        confirmResolver?.(choice);
        confirmResolver = null;
        set({ confirmPrompt: null });
    },

    inputsPrompt: null,
    askInputs: (inputs) =>
        new Promise<InputValues | null>((resolve) => {
            inputsResolver = resolve;
            set({ inputsPrompt: inputs });
        }),
    resolveInputs: (values) => {
        inputsResolver?.(values);
        inputsResolver = null;
        set({ inputsPrompt: null });
    },
}));
