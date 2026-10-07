import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useEditorStore, isDirty } from "../store/editorStore";
import { useUIStore } from "../store/uiStore";
import { saveAll } from "../store/projectActions";

/** Intercept window close: if anything is unsaved, prompt Save / Don't Save / Cancel. */
export function useCloseGuard(): void {
    useEffect(() => {
        const win = getCurrentWindow();
        const unlisten = win.onCloseRequested(async (event) => {
            const anyDirty = useEditorStore.getState().openFiles.some(isDirty);
            if (!anyDirty) return;

            event.preventDefault();
            const choice = await useUIStore.getState().confirmUnsaved({
                title: "Unsaved changes",
                message: "Save changes before closing?",
            });
            if (choice === "cancel") return;
            if (choice === "save" && !(await saveAll())) return;
            await win.destroy();
        });
        return () => {
            void unlisten.then((fn) => fn());
        };
    }, []);
}
