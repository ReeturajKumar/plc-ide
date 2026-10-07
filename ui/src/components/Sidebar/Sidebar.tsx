import type { ReactNode } from "react";
import { useUIStore } from "../../store/uiStore";
import { useEditorStore } from "../../store/editorStore";
import { closeProject, openProjectDialog, saveActive } from "../../store/projectActions";
import { runProgram, stopProgram } from "../../store/runActions";
import { useSimulatorStore } from "../../store/simulatorStore";

const iconProps = {
    className: "w-5 h-5",
    viewBox: "0 0 24 24",
    fill: "none",
    stroke: "currentColor",
    strokeWidth: 2,
    strokeLinecap: "round" as const,
    strokeLinejoin: "round" as const,
};

interface BarButton {
    id: string;
    title: string;
    icon: ReactNode;
    onClick: () => void;
    active?: boolean;
    disabled?: boolean;
}

const Sidebar = () => {
    const sidebarVisible = useUIStore((s) => s.sidebarVisible);
    const toggleSidebar = useUIStore((s) => s.toggleSidebar);
    const openDialog = useUIStore((s) => s.openDialog);
    const hasActiveFile = useEditorStore((s) => s.activePath !== null);
    const simActive = useSimulatorStore((s) => s.runtime.status !== "STOPPED");

    const top: BarButton[] = [
        {
            id: "explorer",
            title: "Explorer (toggle)",
            active: sidebarVisible,
            onClick: toggleSidebar,
            icon: (
                <svg {...iconProps}>
                    <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.93a2 2 0 0 1-1.66-.9l-.82-1.2A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13c0 1.1.9 2 2 2Z" />
                </svg>
            ),
        },
        {
            id: "new",
            title: "New Project (Ctrl+N)",
            onClick: () => openDialog("newProject"),
            icon: (
                <svg {...iconProps}>
                    <path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z" />
                    <line x1="12" y1="11" x2="12" y2="17" />
                    <line x1="9" y1="14" x2="15" y2="14" />
                </svg>
            ),
        },
        {
            id: "open",
            title: "Open Project (Ctrl+O)",
            onClick: () => void openProjectDialog(),
            icon: (
                <svg {...iconProps}>
                    <path d="M22 19a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h5l2 3h9a2 2 0 0 1 2 2z" />
                </svg>
            ),
        },
        {
            id: "save",
            title: "Save (Ctrl+S)",
            disabled: !hasActiveFile,
            onClick: () => void saveActive(),
            icon: (
                <svg {...iconProps}>
                    <path d="M19 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11l5 5v11a2 2 0 0 1-2 2z" />
                    <polyline points="17 21 17 13 7 13 7 21" />
                    <polyline points="7 3 7 8 15 8" />
                </svg>
            ),
        },
        {
            id: "run",
            title: simActive ? "Stop PLC (Shift+F5)" : "Run PLC (F5)",
            active: simActive,
            disabled: !simActive && !hasActiveFile,
            onClick: () => void (simActive ? stopProgram() : runProgram()),
            icon: (
                <svg {...iconProps} fill="currentColor" stroke="none">
                    {simActive ? <rect x="6" y="6" width="12" height="12" rx="1" /> : <polygon points="6 3 20 12 6 21 6 3" />}
                </svg>
            ),
        },
    ];

    const bottom: BarButton[] = [
        {
            id: "collapse",
            title: sidebarVisible ? "Collapse Explorer" : "Expand Explorer",
            onClick: toggleSidebar,
            icon: (
                <svg {...iconProps}>
                    {sidebarVisible ? (
                        <>
                            <line x1="19" y1="12" x2="5" y2="12" />
                            <polyline points="12 19 5 12 12 5" />
                        </>
                    ) : (
                        <>
                            <line x1="5" y1="12" x2="19" y2="12" />
                            <polyline points="12 5 19 12 12 19" />
                        </>
                    )}
                </svg>
            ),
        },
        {
            id: "close",
            title: "Close Project",
            onClick: () => void closeProject(),
            icon: (
                <svg {...iconProps}>
                    <path d="M9 21H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h4" />
                    <polyline points="16 17 21 12 16 7" />
                    <line x1="21" y1="12" x2="9" y2="12" />
                </svg>
            ),
        },
    ];

    const renderButton = (b: BarButton) => (
        <button
            key={b.id}
            onClick={b.onClick}
            disabled={b.disabled}
            title={b.title}
            aria-label={b.title}
            aria-pressed={b.active}
            className={`relative w-8 h-8 rounded-lg flex items-center justify-center transition-colors cursor-pointer disabled:opacity-30 disabled:cursor-not-allowed focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)] ${
                b.active
                    ? "text-[var(--theme-text-primary)] bg-[var(--theme-bg-active)]"
                    : "text-[var(--theme-text-muted)] hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)]"
            }`}
        >
            {b.active && (
                <span className="absolute left-[-6px] top-1.5 bottom-1.5 w-0.5 rounded-full bg-[var(--theme-text-primary)]" />
            )}
            {b.icon}
        </button>
    );

    return (
        <aside className="w-11 shrink-0 theme-sidebar border-r flex flex-col items-center justify-between py-3 select-none z-10">
            <div className="flex flex-col items-center gap-3">{top.map(renderButton)}</div>
            <div className="flex flex-col items-center gap-3">{bottom.map(renderButton)}</div>
        </aside>
    );
};

export default Sidebar;
