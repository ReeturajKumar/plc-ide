import { useEffect } from "react";
import { useEditorStore } from "../store/editorStore";
import { useProjectStore } from "../store/projectStore";
import { useConsoleStore } from "../store/consoleStore";
import { diagnose, IO_MAPPING_PATH, toProblems } from "../store/runActions";

/** How long typing must pause before the active file is re-checked. */
const CHECK_DELAY_MS = 400;

/**
 * Live diagnostics: re-check the active program shortly after each edit, with the same
 * parser and semantic analyzer as Compile and RUN (without running anything). Problems
 * are kept for the project's programs and open files only.
 */
export function useDiagnostics(): void {
    const openPaths = useEditorStore((s) => s.openFiles.map((f) => f.path).join("\n"));
    const programPaths = useProjectStore((s) => s.project?.programs.map((p) => p.path).join("\n") ?? "");
    const activePath = useEditorStore((s) => s.activePath);
    const content = useEditorStore((s) => s.openFiles.find((f) => f.path === s.activePath)?.content);

    useEffect(() => {
        const keep = `${openPaths}\n${programPaths}\n${IO_MAPPING_PATH}`.split("\n").filter(Boolean);
        useConsoleStore.getState().retainProblems(keep);
    }, [openPaths, programPaths]);

    useEffect(() => {
        if (!activePath || content === undefined) return;
        const timer = setTimeout(async () => {
            let errors;
            try {
                errors = await diagnose(activePath);
            } catch (e) {
                console.error("[diagnostics]", e);
                return;
            }
            // Not a program, or a result that arrived after the file changed again.
            const now = useEditorStore.getState().openFiles.find((f) => f.path === activePath);
            if (!errors || now?.content !== content) return;
            useConsoleStore.getState().setFileProblems(activePath, toProblems(activePath, errors));
        }, CHECK_DELAY_MS);
        return () => clearTimeout(timer);
    }, [activePath, content]);
}
