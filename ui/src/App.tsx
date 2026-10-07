import "./index.css";
import Welcome from "./pages/Welcome/Welcome";
import Workspace from "./pages/Workspace/Workspace";
import NewProjectDialog from "./components/dialogs/NewProjectDialog";
import ConfirmDialog from "./components/dialogs/ConfirmDialog";
import Notifications from "./components/common/Notifications";
import { useProjectStore } from "./store/projectStore";
import { useUIStore } from "./store/uiStore";
import { useKeyboardShortcuts } from "./hooks/useKeyboardShortcuts";
import { useCloseGuard } from "./hooks/useCloseGuard";

function App() {
    const hasProject = useProjectStore((s) => s.project !== null);
    const activeDialog = useUIStore((s) => s.activeDialog);
    useKeyboardShortcuts();
    useCloseGuard();

    return (
        <>
            {hasProject ? <Workspace /> : <Welcome />}

            {/* Global overlays */}
            {activeDialog === "newProject" && <NewProjectDialog />}
            <ConfirmDialog />
            <Notifications />
        </>
    );
}

export default App;
