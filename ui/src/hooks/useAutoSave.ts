import { useEffect } from "react";
import { useEditorStore, isDirty } from "../store/editorStore";
import { useUIStore } from "../store/uiStore";
import { autoSave } from "../store/projectActions";

/**
 * Auto save (Settings → Auto Save): once nothing has been typed for the delay, every
 * changed file is written. Each edit restarts the wait, like VS Code's "afterDelay".
 */
export function useAutoSave(): void {
    const enabled = useUIStore((s) => s.autoSave);
    const delay = useUIStore((s) => s.autoSaveDelay);
    const files = useEditorStore((s) => s.openFiles);
    const dirty = files.some(isDirty);

    useEffect(() => {
        if (!enabled || !dirty) return;
        const timer = setTimeout(() => void autoSave(), delay);
        return () => clearTimeout(timer);
    }, [enabled, delay, dirty, files]);
}
