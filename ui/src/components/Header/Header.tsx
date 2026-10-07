import { useEffect, useRef, useState } from "react";
import Menu, { type MenuItem } from "../common/Menu";
import { useProjectStore } from "../../store/projectStore";
import { useUIStore } from "../../store/uiStore";
import {
    closeProject,
    openProjectDialog,
    startNewEntry,
    saveActive,
    saveAll,
} from "../../store/projectActions";
import { compileAll, compileCurrent, pauseProgram, resumeProgram, runProgram, stepProgram, stepStatement, stopProgram, toggleBreakpoint } from "../../store/runActions";
import { useEditorStore } from "../../store/editorStore";
import IconButton from "../common/IconButton";
import { useConsoleStore } from "../../store/consoleStore";
import { useSimulatorStore } from "../../store/simulatorStore";
import { runEditorAction } from "../../utils/editorRef";

const Header = () => {
    const hasProject = useProjectStore((s) => s.project !== null);
    const simStatus = useSimulatorStore((s) => s.runtime.status);
    const sidebarVisible = useUIStore((s) => s.sidebarVisible);
    const panelVisible = useUIStore((s) => s.panelVisible);
    const openDialog = useUIStore((s) => s.openDialog);
    const toggleSidebar = useUIStore((s) => s.toggleSidebar);
    const togglePanel = useUIStore((s) => s.togglePanel);
    const setPanelTab = useUIStore((s) => s.setPanelTab);
    const notify = useUIStore((s) => s.notify);
    const [mobileOpen, setMobileOpen] = useState(false);
    const mobileRef = useRef<HTMLDivElement>(null);

    useEffect(() => {
        if (!mobileOpen) return;
        const onDown = (e: MouseEvent) => {
            if (mobileRef.current && !mobileRef.current.contains(e.target as Node)) setMobileOpen(false);
        };
        window.addEventListener("mousedown", onDown);
        return () => window.removeEventListener("mousedown", onDown);
    }, [mobileOpen]);

    const menus: { label: string; items: MenuItem[] }[] = [
        {
            label: "File",
            items: [
                { label: "New Project…", shortcut: "Ctrl+N", onClick: () => openDialog("newProject") },
                { label: "Open Project…", shortcut: "Ctrl+O", onClick: () => void openProjectDialog() },
                { separator: true, label: "" },
                { label: "Save", shortcut: "Ctrl+S", onClick: () => void saveActive(), disabled: !hasProject },
                { label: "Save All", onClick: () => void saveAll(), disabled: !hasProject },
                { separator: true, label: "" },
                { label: "Close Project", onClick: () => void closeProject(), disabled: !hasProject },
            ],
        },
        {
            label: "Edit",
            items: [
                { label: "Undo", shortcut: "Ctrl+Z", onClick: () => runEditorAction("undo") },
                { label: "Redo", shortcut: "Ctrl+Y", onClick: () => runEditorAction("redo") },
                { separator: true, label: "" },
                { label: "Find", shortcut: "Ctrl+F", onClick: () => runEditorAction("actions.find") },
                { label: "Select All", shortcut: "Ctrl+A", onClick: () => runEditorAction("editor.action.selectAll") },
            ],
        },
        {
            label: "View",
            items: [
                { label: "Toggle Explorer", onClick: toggleSidebar },
                { label: "Toggle Panel", shortcut: "Ctrl+J", onClick: togglePanel },
                { separator: true, label: "" },
                { label: "Problems", shortcut: "Ctrl+Shift+M", onClick: () => setPanelTab("problems") },
                { label: "Output", shortcut: "Ctrl+Shift+U", onClick: () => setPanelTab("output") },
                { label: "Terminal", shortcut: "Ctrl+`", onClick: () => setPanelTab("terminal") },
                { label: "Debug Console", shortcut: "Ctrl+Shift+Y", onClick: () => setPanelTab("debug") },
            ],
        },
        {
            label: "Project",
            items: [
                // Names are typed inline in the Explorer, as in VS Code.
                { label: "New File…", onClick: () => startNewEntry("file"), disabled: !hasProject },
                { label: "New Folder…", onClick: () => startNewEntry("folder"), disabled: !hasProject },
                { label: "Save Project", onClick: () => void saveAll(), disabled: !hasProject },
            ],
        },
        {
            label: "Run",
            items: [
                { label: "Compile", shortcut: "Ctrl+F7", onClick: () => void compileCurrent(), disabled: !hasProject },
                { label: "Compile All", shortcut: "F7", onClick: () => void compileAll(), disabled: !hasProject },
                { separator: true, label: "" },
                {
                    label: "Run All Programs",
                    shortcut: "F5",
                    onClick: () => void runProgram(),
                    disabled: !hasProject || simStatus !== "STOPPED",
                },
                simStatus === "PAUSED"
                    ? { label: "Resume", shortcut: "F5", onClick: () => void resumeProgram() }
                    : { label: "Pause", onClick: () => void pauseProgram(), disabled: simStatus !== "RUNNING" },
                {
                    label: "Stop",
                    shortcut: "Shift+F5",
                    onClick: () => void stopProgram(),
                    disabled: simStatus === "STOPPED",
                },
                { label: "Step Statement", shortcut: "F11", onClick: () => void stepStatement(), disabled: simStatus !== "PAUSED" },
                { label: "Step Scan", shortcut: "F10", onClick: () => void stepProgram(), disabled: simStatus !== "PAUSED" },
                {
                    label: "Toggle Breakpoint",
                    shortcut: "F9",
                    onClick: () => {
                        const { activePath, cursor } = useEditorStore.getState();
                        if (activePath) void toggleBreakpoint(activePath, cursor.line);
                    },
                    disabled: !hasProject,
                },
                { separator: true, label: "" },
                { label: "Show Simulator", onClick: () => setPanelTab("simulator") },
                { label: "Clear Output", onClick: () => useConsoleStore.getState().clearOutput() },
            ],
        },
        {
            label: "Help",
            items: [
                { label: "About MyPLC IDE", onClick: () => notify("info", "MyPLC IDE — Phase 1") },
            ],
        },
    ];

    return (
        <header className="h-9 theme-header border-b select-none flex items-center px-3 text-xs gap-2">
            <div className="flex items-center pr-1 shrink-0">
                <span className="font-semibold text-[var(--theme-text-primary)] tracking-tight text-[13px]">
                    {/* Shorten brand on very narrow widths. */}
                    <span className="hidden sm:inline">MyPLC IDE</span>
                    <span className="inline sm:hidden">MyPLC</span>
                </span>
            </div>

            {/* Inline menu bar (wider screens) */}
            <nav className="hidden md:flex items-center gap-0.5 text-[var(--theme-text-secondary)] text-[12px] min-w-0">
                {menus.map((m) => (
                    <Menu key={m.label} label={m.label} items={m.items} />
                ))}
            </nav>

            {/* Hamburger menu (narrow screens) */}
            <div ref={mobileRef} className="relative md:hidden">
                <button
                    onClick={() => setMobileOpen((o) => !o)}
                    aria-label="Menu"
                    aria-expanded={mobileOpen}
                    className="w-7 h-6 rounded flex items-center justify-center text-[var(--theme-text-secondary)] hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)] cursor-pointer"
                >
                    <svg className="w-4 h-4" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round">
                        <line x1="3" y1="6" x2="21" y2="6" />
                        <line x1="3" y1="12" x2="21" y2="12" />
                        <line x1="3" y1="18" x2="21" y2="18" />
                    </svg>
                </button>
                {mobileOpen && (
                    <div className="absolute left-0 top-full mt-1 z-50 flex flex-col theme-panel border border-[var(--theme-border)] rounded-md shadow-xl p-1 text-[12px] text-[var(--theme-text-secondary)]">
                        {menus.map((m) => (
                            <Menu key={m.label} label={m.label} items={m.items} />
                        ))}
                    </div>
                )}
            </div>

            {/* Layout toggles, as in VS Code's title bar */}
            <div className="ml-auto flex items-center gap-0.5">
                <IconButton title="Toggle Primary Side Bar (Ctrl+B)" active={sidebarVisible} onClick={toggleSidebar}>
                    <rect x="3" y="4" width="18" height="16" rx="2" />
                    <line x1="9" y1="4" x2="9" y2="20" />
                    {sidebarVisible && <rect x="3.9" y="4.9" width="4.2" height="14.2" rx="1" fill="currentColor" stroke="none" />}
                </IconButton>
                <IconButton title="Toggle Panel (Ctrl+J)" active={panelVisible} onClick={togglePanel}>
                    <rect x="3" y="4" width="18" height="16" rx="2" />
                    <line x1="3" y1="14" x2="21" y2="14" />
                    {panelVisible && <rect x="3.9" y="14.9" width="16.2" height="4.2" rx="1" fill="currentColor" stroke="none" />}
                </IconButton>
            </div>
        </header>
    );
};

export default Header;
