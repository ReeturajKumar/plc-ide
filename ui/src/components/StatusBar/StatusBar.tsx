import { useEditorStore } from "../../store/editorStore";
import { useProjectStore } from "../../store/projectStore";
import { useUIStore } from "../../store/uiStore";
import { useConsoleStore } from "../../store/consoleStore";
import { useSimulatorStore } from "../../store/simulatorStore";
import { languageFor } from "../../utils/project";

const StatusBar = () => {
    const cursor = useEditorStore((s) => s.cursor);
    const activePath = useEditorStore((s) => s.activePath);
    const hasActive = activePath !== null;
    const projectName = useProjectStore((s) => s.project?.name);
    const panelVisible = useUIStore((s) => s.panelVisible);
    const setPanelTab = useUIStore((s) => s.setPanelTab);
    const togglePanel = useUIStore((s) => s.togglePanel);
    const problemCount = useConsoleStore((s) => s.problems.length);
    const simStatus = useSimulatorStore((s) => s.runtime.status);

    return (
        <footer className="h-6 theme-panel-header border-t flex items-center justify-between px-3 text-[11px] text-[var(--theme-text-muted)] select-none">
            <div className="flex items-center gap-3">
                <span className="truncate">{projectName ? `Project: ${projectName}` : "No project"}</span>
                <button
                    onClick={() => setPanelTab("problems")}
                    title="Problems"
                    className={`transition-colors cursor-pointer ${problemCount ? "text-red-400" : "hover:text-[var(--theme-text-primary)]"}`}
                >
                    ⚠ {problemCount}
                </button>
                {simStatus !== "STOPPED" && (
                    <button
                        onClick={() => setPanelTab("simulator")}
                        title="Show PLC Simulator"
                        className={`cursor-pointer ${simStatus === "RUNNING" ? "text-emerald-400" : "text-amber-400"}`}
                    >
                        {simStatus === "RUNNING" ? "● PLC RUNNING" : "❚❚ PLC PAUSED"}
                    </button>
                )}
            </div>
            <div className="flex items-center gap-4">
                {hasActive && <span>Ln {cursor.line}, Col {cursor.col}</span>}
                {activePath && <span>{languageFor(activePath).label}</span>}
                <span>UTF-8</span>
                <button
                    onClick={togglePanel}
                    title="Toggle Panel (Ctrl+`)"
                    aria-pressed={panelVisible}
                    className={`transition-colors cursor-pointer ${panelVisible ? "text-[var(--theme-text-primary)]" : "hover:text-[var(--theme-text-primary)]"}`}
                >
                    Panel
                </button>
            </div>
        </footer>
    );
};

export default StatusBar;
