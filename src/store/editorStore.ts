import { create } from "zustand";
import type { OpenFile } from "../types/editor";
import { baseName, isWithin, remapPath } from "../utils/project";

interface EditorState {
    openFiles: OpenFile[];
    /** Path of the active tab. */
    activePath: string | null;
    cursor: { line: number; col: number };

    openFile: (file: OpenFile) => void;
    closeFile: (path: string) => void;
    /** Close every tab at or under `path` (a deleted file or folder). */
    closeWithin: (path: string) => void;
    setActive: (path: string) => void;
    updateContent: (path: string, content: string) => void;
    markFileSaved: (path: string) => void;
    /** Follow a rename of a file or folder: tabs (and unsaved edits) move with it. */
    remapPaths: (from: string, to: string) => void;
    setCursor: (line: number, col: number) => void;
    closeAll: () => void;
}

export const isDirty = (f: OpenFile): boolean => f.content !== f.savedContent;

export const useEditorStore = create<EditorState>((set) => ({
    openFiles: [],
    activePath: null,
    cursor: { line: 1, col: 1 },

    openFile: (file) =>
        set((s) => {
            // Already open → just focus it.
            if (s.openFiles.some((f) => f.path === file.path)) return { activePath: file.path };
            return { openFiles: [...s.openFiles, file], activePath: file.path };
        }),

    closeFile: (path) =>
        set((s) => {
            const openFiles = s.openFiles.filter((f) => f.path !== path);
            const activePath = s.activePath === path ? (openFiles[openFiles.length - 1]?.path ?? null) : s.activePath;
            return { openFiles, activePath };
        }),

    closeWithin: (path) =>
        set((s) => {
            const openFiles = s.openFiles.filter((f) => !isWithin(f.path, path));
            const activeGone = s.activePath !== null && isWithin(s.activePath, path);
            return { openFiles, activePath: activeGone ? (openFiles[openFiles.length - 1]?.path ?? null) : s.activePath };
        }),

    setActive: (path) => set({ activePath: path }),

    updateContent: (path, content) =>
        set((s) => ({ openFiles: s.openFiles.map((f) => (f.path === path ? { ...f, content } : f)) })),

    markFileSaved: (path) =>
        set((s) => ({ openFiles: s.openFiles.map((f) => (f.path === path ? { ...f, savedContent: f.content } : f)) })),

    remapPaths: (from, to) =>
        set((s) => ({
            openFiles: s.openFiles.map((f) => {
                const path = remapPath(f.path, from, to);
                return path === f.path ? f : { ...f, path, fileName: baseName(path) };
            }),
            activePath: s.activePath === null ? null : remapPath(s.activePath, from, to),
        })),

    setCursor: (line, col) => set({ cursor: { line, col } }),
    closeAll: () => set({ openFiles: [], activePath: null, cursor: { line: 1, col: 1 } }),
}));
