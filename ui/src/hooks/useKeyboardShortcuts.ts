import { useEffect } from "react";
import { useUIStore, type PanelTab } from "../store/uiStore";
import { useProjectStore } from "../store/projectStore";
import { openProjectDialog, saveActive } from "../store/projectActions";
import { compileAll, compileCurrent, runOrResume, stepProgram, stepStatement, stopProgram, toggleBreakpoint } from "../store/runActions";
import { useEditorStore } from "../store/editorStore";

/**
 * Global shortcuts.
 * File:  Ctrl/Cmd + S save · + O open · + N new
 * PLC:   F5 run / resume · Shift+F5 stop · F7 compile all · Ctrl+F7 compile
 * Debug: F9 toggle breakpoint · F10 step scan · F11 step statement (both while paused)
 * Panel (VS Code style):
 *   Ctrl+`          toggle panel (Terminal)
 *   Ctrl+J          toggle panel
 *   Ctrl+Shift+M    Problems
 *   Ctrl+Shift+U    Output
 *   Ctrl+Shift+Y    Debug Console
 *   Ctrl+Shift+`    Terminal
 */
export function useKeyboardShortcuts(): void {
    useEffect(() => {
        const onKey = (e: KeyboardEvent) => {
            // F5 must always be swallowed: in the webview it would otherwise reload the IDE.
            if (e.key === "F5") {
                e.preventDefault();
                if (e.shiftKey) void stopProgram();
                else runOrResume();
                return;
            }
            const hasProject = useProjectStore.getState().project !== null;
            // F10 alone; Shift+F10 opens context menus. The webview would otherwise focus its menu.
            if (e.key === "F10" && !e.shiftKey) {
                e.preventDefault();
                if (hasProject) void stepProgram();
                return;
            }
            if (e.key === "F11") {
                e.preventDefault(); // the webview would go full screen
                if (hasProject) void stepStatement();
                return;
            }
            if (e.key === "F9") {
                e.preventDefault();
                const { activePath, cursor } = useEditorStore.getState();
                if (hasProject && activePath) void toggleBreakpoint(activePath, cursor.line);
                return;
            }
            if (e.key === "F7") {
                e.preventDefault();
                if (hasProject) void (e.ctrlKey || e.metaKey ? compileCurrent() : compileAll());
                return;
            }

            const mod = e.ctrlKey || e.metaKey;
            if (!mod) return;
            const key = e.key.toLowerCase();
            const ui = useUIStore.getState();

            const showTab = (tab: PanelTab) => {
                e.preventDefault();
                ui.setPanelTab(tab); // also makes the panel visible
            };

            if (e.shiftKey) {
                if (key === "m") return showTab("problems");
                if (key === "u") return showTab("output");
                if (key === "y") return showTab("debug");
                if (key === "`") return showTab("terminal");
                return;
            }

            if (key === "s") {
                e.preventDefault();
                void saveActive();
            } else if (key === "o") {
                e.preventDefault();
                void openProjectDialog();
            } else if (key === "n") {
                e.preventDefault();
                ui.openDialog("newProject");
            } else if (key === "b") {
                e.preventDefault();
                ui.toggleSidebar();
            } else if (key === "j") {
                e.preventDefault();
                ui.togglePanel();
            } else if (key === "`") {
                // VS Code: Ctrl+` focuses the terminal / toggles the panel.
                e.preventDefault();
                if (ui.panelVisible && ui.panelTab === "terminal") {
                    ui.togglePanel();
                } else {
                    ui.setPanelTab("terminal");
                }
            }
        };
        window.addEventListener("keydown", onKey);
        return () => window.removeEventListener("keydown", onKey);
    }, []);
}
