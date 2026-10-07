import { create } from "zustand";
import type { ErrorKind } from "../types/runtime";

/** One diagnostic, shown in the Problems panel and as an editor squiggle. */
export interface Problem {
    /** Project-relative path of the file it belongs to. */
    path: string;
    fileName: string;
    kind: ErrorKind;
    /** 1-based; 0 when the problem has no source position. */
    line: number;
    column: number;
    message: string;
}

interface ConsoleState {
    output: string[];
    problems: Problem[];

    appendOutput: (lines: string[]) => void;
    /** Replace the problems of one file (an empty list clears them). */
    setFileProblems: (path: string, problems: Problem[]) => void;
    /** Drop problems of files that are no longer open. */
    retainProblems: (openPaths: string[]) => void;
    clearOutput: () => void;
}

export const useConsoleStore = create<ConsoleState>((set) => ({
    output: [],
    problems: [],

    appendOutput: (lines) => set((s) => ({ output: [...s.output, ...lines] })),
    setFileProblems: (path, problems) =>
        set((s) => ({ problems: [...s.problems.filter((p) => p.path !== path), ...problems] })),
    retainProblems: (openPaths) =>
        set((s) => {
            const kept = s.problems.filter((p) => openPaths.includes(p.path));
            return kept.length === s.problems.length ? s : { problems: kept };
        }),
    clearOutput: () => set({ output: [] }),
}));
