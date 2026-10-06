import { useEffect, useRef } from "react";
import { useUIStore } from "../../store/uiStore";

/** Vertical drag handle that resizes the explorer panel. */
const Resizer = () => {
    const setExplorerWidth = useUIStore((s) => s.setExplorerWidth);
    const dragging = useRef(false);

    useEffect(() => {
        const onMove = (e: MouseEvent) => {
            if (!dragging.current) return;
            // Width = pointer X minus the fixed activity-bar width (44px).
            setExplorerWidth(e.clientX - 44);
        };
        const onUp = () => {
            if (!dragging.current) return;
            dragging.current = false;
            document.body.style.cursor = "";
            document.body.style.userSelect = "";
        };
        window.addEventListener("mousemove", onMove);
        window.addEventListener("mouseup", onUp);
        return () => {
            window.removeEventListener("mousemove", onMove);
            window.removeEventListener("mouseup", onUp);
        };
    }, [setExplorerWidth]);

    const start = () => {
        dragging.current = true;
        document.body.style.cursor = "col-resize";
        document.body.style.userSelect = "none";
    };

    return (
        <div
            onMouseDown={start}
            role="separator"
            aria-orientation="vertical"
            title="Drag to resize"
            className="w-1 shrink-0 cursor-col-resize bg-transparent hover:bg-[var(--theme-border-focus)] transition-colors"
        />
    );
};

export default Resizer;
