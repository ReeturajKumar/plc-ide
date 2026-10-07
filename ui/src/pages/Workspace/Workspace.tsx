import { useEffect } from "react";
import Header from "../../components/Header/Header";
import Sidebar from "../../components/Sidebar/Sidebar";
import ProjectExplorer from "../../components/ProjectExplorer/ProjectExplorer";
import Editor from "../../components/Editor/Editor";
import Panel from "../../components/Panel/Panel";
import StatusBar from "../../components/StatusBar/StatusBar";
import Resizer from "../../components/common/Resizer";
import { useUIStore } from "../../store/uiStore";
import { useResponsiveLayout } from "../../hooks/useResponsiveLayout";
import { refreshTree } from "../../store/projectActions";
import { useDiagnostics } from "../../hooks/useDiagnostics";

const Workspace = () => {
    const sidebarVisible = useUIStore((s) => s.sidebarVisible);
    const panelVisible = useUIStore((s) => s.panelVisible);
    const panelMaximized = useUIStore((s) => s.panelMaximized);
    useResponsiveLayout();
    useDiagnostics();

    // Pick up files changed outside the IDE (Windows Explorer, git, …) when the window regains focus.
    useEffect(() => {
        const onFocus = () => void refreshTree();
        window.addEventListener("focus", onFocus);
        return () => window.removeEventListener("focus", onFocus);
    }, []);

    return (
        <div className="h-screen w-screen flex flex-col overflow-hidden theme-app">
            <Header />

            <div className="flex flex-1 overflow-hidden">
                <Sidebar />
                {sidebarVisible && (
                    <>
                        <ProjectExplorer />
                        <Resizer />
                    </>
                )}
                <main className="flex-1 flex flex-col min-w-0 overflow-hidden">
                    {/* Kept mounted while the panel is maximized, so the editor keeps its state. */}
                    <div className={`flex-1 min-h-0 overflow-hidden ${panelVisible && panelMaximized ? "hidden" : "flex"}`}>
                        <Editor />
                    </div>
                    {panelVisible && <Panel />}
                </main>
            </div>

            <StatusBar />
        </div>
    );
};

export default Workspace;
