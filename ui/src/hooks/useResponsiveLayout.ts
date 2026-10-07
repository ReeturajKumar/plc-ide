import { useEffect } from "react";
import { useUIStore } from "../store/uiStore";

const BREAKPOINT = 760;

/**
 * Auto-collapses the explorer when the window gets narrow and restores it when
 * it widens again. Only acts on threshold crossings, so manual toggles stick.
 */
export function useResponsiveLayout(): void {
    useEffect(() => {
        let wasNarrow = window.innerWidth < BREAKPOINT;
        // Apply initial state once on mount.
        if (wasNarrow) useUIStore.getState().setSidebarVisible(false);

        const onResize = () => {
            const narrow = window.innerWidth < BREAKPOINT;
            if (narrow === wasNarrow) return;
            wasNarrow = narrow;
            useUIStore.getState().setSidebarVisible(!narrow);
        };
        window.addEventListener("resize", onResize);
        return () => window.removeEventListener("resize", onResize);
    }, []);
}
