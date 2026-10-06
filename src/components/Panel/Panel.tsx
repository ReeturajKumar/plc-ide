import { useEffect, useRef } from "react";
import { useUIStore, type PanelTab } from "../../store/uiStore";
import { useConsoleStore } from "../../store/consoleStore";
import SimulatorPanel from "../Simulator/SimulatorPanel";
import IoPanel from "../Simulator/IoPanel";
import DebugInfo from "../Simulator/DebugInfo";
import { useSimulatorStore } from "../../store/simulatorStore";
import { openFile } from "../../store/projectActions";
import { IO_MAPPING_PATH } from "../../store/runActions";
import IconButton, { MoreButton } from "../common/IconButton";
import { copyToClipboard } from "../../store/projectActions";
import { revealPosition } from "../../utils/editorRef";

const STATUS_BAR_H = 24; // matches StatusBar h-6

const TABS: { id: PanelTab; label: string; shortcut: string; empty: string }[] = [
    { id: "simulator", label: "Simulator", shortcut: "F5 to run", empty: "" },
    { id: "io", label: "I/O", shortcut: "simulated inputs and outputs", empty: "" },
    { id: "problems", label: "Problems", shortcut: "Ctrl+Shift+M", empty: "No problems detected in the workspace." },
    { id: "output", label: "Output", shortcut: "Ctrl+Shift+U", empty: "No output yet." },
    { id: "terminal", label: "Terminal", shortcut: "Ctrl+`", empty: "Integrated terminal arrives with the PLC runtime (later phase)." },
    {
        id: "debug",
        label: "Debug Console",
        shortcut: "Ctrl+Shift+Y",
        empty: "Not paused. Click left of a line number (or press F9) to set a breakpoint, then RUN (F5).",
    },
];

const Panel = () => {
    const panelHeight = useUIStore((s) => s.panelHeight);
    const panelTab = useUIStore((s) => s.panelTab);
    const setPanelTab = useUIStore((s) => s.setPanelTab);
    const setPanelHeight = useUIStore((s) => s.setPanelHeight);
    const togglePanel = useUIStore((s) => s.togglePanel);
    const panelMaximized = useUIStore((s) => s.panelMaximized);
    const togglePanelMaximized = useUIStore((s) => s.togglePanelMaximized);
    const output = useConsoleStore((s) => s.output);
    const problems = useConsoleStore((s) => s.problems);
    const debugLocation = useSimulatorStore((s) => s.runtime.location);
    const clearOutput = useConsoleStore((s) => s.clearOutput);
    const dragging = useRef(false);
    const scrollRef = useRef<HTMLDivElement>(null);

    // Keep the Output log pinned to the newest line.
    useEffect(() => {
        if (panelTab === "output" && scrollRef.current) scrollRef.current.scrollTop = scrollRef.current.scrollHeight;
    }, [output, panelTab]);

    useEffect(() => {
        const onMove = (e: MouseEvent) => {
            if (!dragging.current) return;
            setPanelHeight(window.innerHeight - e.clientY - STATUS_BAR_H);
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
    }, [setPanelHeight]);

    const startDrag = () => {
        dragging.current = true;
        document.body.style.cursor = "row-resize";
        document.body.style.userSelect = "none";
    };

    const active = TABS.find((t) => t.id === panelTab) ?? TABS[0];

    return (
        <div
            style={panelMaximized ? undefined : { height: panelHeight }}
            className={`${panelMaximized ? "flex-1 min-h-0" : "shrink-0"} flex flex-col theme-panel border-t border-[var(--theme-border)]`}
        >
            {/* Top drag handle */}
            <div
                onMouseDown={startDrag}
                role="separator"
                aria-orientation="horizontal"
                title="Drag to resize"
                className="h-1 shrink-0 cursor-row-resize hover:bg-[var(--theme-border-focus)] transition-colors"
            />

            {/* Tab bar */}
            <div className="theme-panel-header border-b border-[var(--theme-border)] flex items-center justify-between pr-2">
                <div className="flex items-center">
                    {TABS.map((t) => (
                        <button
                            key={t.id}
                            onClick={() => setPanelTab(t.id)}
                            title={`${t.label} (${t.shortcut})`}
                            className={`px-3 py-1.5 text-[11px] font-medium uppercase tracking-wide border-b-2 transition-colors cursor-pointer ${
                                panelTab === t.id
                                    ? "text-[var(--theme-text-primary)] border-[var(--theme-border-focus)]"
                                    : "text-[var(--theme-text-muted)] hover:text-[var(--theme-text-primary)] border-transparent"
                            }`}
                        >
                            {t.label}
                            {t.id === "problems" && problems.length > 0 && (
                                <span className="ml-1.5 rounded-full bg-red-500/80 px-1.5 text-[10px] text-white">{problems.length}</span>
                            )}
                        </button>
                    ))}
                </div>
                <div className="flex items-center gap-1">
                    {panelTab === "output" && (
                        <IconButton title="Clear Output" onClick={clearOutput} disabled={output.length === 0}>
                            <polyline points="4 7 20 7" />
                            <path d="M9 7V4h6v3" />
                            <path d="M6 7l1 13h10l1-13" />
                        </IconButton>
                    )}
                    <MoreButton
                        title="More Actions…"
                        items={() => [
                            { label: "Clear Output", onClick: clearOutput, disabled: output.length === 0 },
                            { label: "Copy Output", onClick: () => void copyToClipboard(output.join("\n")), disabled: output.length === 0 },
                            { separator: true, label: "" },
                            ...TABS.map((t) => ({ label: `${t.id === panelTab ? "✓ " : ""}${t.label}`, onClick: () => setPanelTab(t.id) })),
                        ]}
                    />
                    <IconButton title={panelMaximized ? "Restore Panel Size" : "Maximize Panel Size"} onClick={togglePanelMaximized}>
                        {panelMaximized ? (
                            <>
                                <polyline points="9 4 9 9 4 9" />
                                <polyline points="15 20 15 15 20 15" />
                                <polyline points="20 9 15 9 15 4" />
                                <polyline points="4 15 9 15 9 20" />
                            </>
                        ) : (
                            <>
                                <polyline points="4 9 4 4 9 4" />
                                <polyline points="20 15 20 20 15 20" />
                                <polyline points="15 4 20 4 20 9" />
                                <polyline points="9 20 4 20 4 15" />
                            </>
                        )}
                    </IconButton>
                    <IconButton title="Hide Panel (Ctrl+J)" onClick={togglePanel}>
                        <line x1="6" y1="6" x2="18" y2="18" />
                        <line x1="18" y1="6" x2="6" y2="18" />
                    </IconButton>
                </div>
            </div>

            {/* Content */}
            <div ref={scrollRef} className="flex-1 overflow-auto p-3 font-mono text-[11px] text-[var(--theme-text-muted)] whitespace-pre-wrap">
                {panelTab === "simulator" && <SimulatorPanel />}
                {panelTab === "io" && <IoPanel />}
                {panelTab === "problems" &&
                    (problems.length === 0 ? (
                        active.empty
                    ) : (
                        <div>
                            <div className="mb-1 text-[var(--theme-text-secondary)]">
                                {problems.length === 1 ? "1 error found" : `${problems.length} errors found`}
                            </div>
                            <ul>
                                {problems.map((p, i) => (
                                    <li
                                        key={i}
                                        onClick={() =>
                                            p.path === IO_MAPPING_PATH
                                                ? setPanelTab("io")
                                                : void openFile(p.path).then(() => p.line > 0 && revealPosition(p.line, p.column))
                                        }
                                        title="Go to error"
                                        className="cursor-pointer rounded px-1 py-0.5 hover:bg-[var(--theme-bg-hover)]"
                                    >
                                        <span className="text-red-400">✖</span>{" "}
                                        <span className="text-[var(--theme-text-primary)]">{p.message}</span>{" "}
                                        <span className="text-[var(--theme-text-muted)]">
                                            [{p.kind}] {p.fileName}
                                            {p.line > 0 && ` Ln ${p.line}, Col ${p.column}`}
                                        </span>
                                    </li>
                                ))}
                            </ul>
                        </div>
                    ))}

                {panelTab === "output" && (output.length ? output.join("\n") : active.empty)}
                {panelTab === "terminal" && active.empty}
                {panelTab === "debug" && (debugLocation ? <DebugInfo /> : active.empty)}
            </div>
        </div>
    );
};

export default Panel;
