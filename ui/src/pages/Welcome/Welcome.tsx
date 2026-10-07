import { useUIStore } from "../../store/uiStore";
import { getRecentProjects, openProjectDialog, openProjectFromPath } from "../../store/projectActions";

const Welcome = () => {
    const openDialog = useUIStore((s) => s.openDialog);
    const recents = getRecentProjects();

    return (
        <div className="h-screen w-screen theme-app flex flex-col items-center justify-center select-none">
            <div className="w-[560px] max-w-[90vw]">
                <h1 className="text-2xl font-semibold text-[var(--theme-text-primary)] tracking-tight">
                    MyPLC IDE
                </h1>
                <p className="text-xs text-[var(--theme-text-muted)] mt-1">
                    IEC 61131-3 Structured Text development environment
                </p>

                <div className="mt-8 grid grid-cols-2 gap-3">
                    <button
                        onClick={() => openDialog("newProject")}
                        className="theme-panel border border-[var(--theme-border)] rounded-lg p-4 text-left hover:bg-[var(--theme-bg-hover)] transition-colors cursor-pointer focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)]"
                    >
                        <div className="text-sm font-medium text-[var(--theme-text-primary)]">New Project</div>
                        <div className="text-[11px] text-[var(--theme-text-muted)] mt-1">
                            Create a new PLC project
                        </div>
                    </button>
                    <button
                        onClick={openProjectDialog}
                        className="theme-panel border border-[var(--theme-border)] rounded-lg p-4 text-left hover:bg-[var(--theme-bg-hover)] transition-colors cursor-pointer focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)]"
                    >
                        <div className="text-sm font-medium text-[var(--theme-text-primary)]">Open Project</div>
                        <div className="text-[11px] text-[var(--theme-text-muted)] mt-1">
                            Open an existing project folder
                        </div>
                    </button>
                </div>

                {recents.length > 0 && (
                    <div className="mt-8">
                        <div className="text-[11px] font-semibold uppercase tracking-wider text-[var(--theme-text-muted)] mb-2">
                            Recent
                        </div>
                        <ul className="flex flex-col">
                            {recents.map((r) => (
                                <li key={r.rootPath}>
                                    <button
                                        onClick={() => openProjectFromPath(r.rootPath)}
                                        className="w-full text-left px-3 py-2 rounded-md hover:bg-[var(--theme-bg-hover)] transition-colors cursor-pointer group"
                                    >
                                        <span className="text-xs text-[var(--theme-text-primary)]">{r.name}</span>
                                        <span className="text-[11px] text-[var(--theme-text-muted)] ml-2 group-hover:text-[var(--theme-text-secondary)]">
                                            {r.rootPath}
                                        </span>
                                    </button>
                                </li>
                            ))}
                        </ul>
                    </div>
                )}
            </div>
        </div>
    );
};

export default Welcome;
