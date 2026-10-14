import "./index.css";
import Welcome from "./pages/Welcome/Welcome";
import Workspace from "./pages/Workspace/Workspace";
import NewProjectDialog from "./components/dialogs/NewProjectDialog";
import ConfirmDialog from "./components/dialogs/ConfirmDialog";
import InputsDialog from "./components/dialogs/InputsDialog";
import SettingsDialog from "./components/dialogs/SettingsDialog";
import { useAutoSave } from "./hooks/useAutoSave";
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
    useAutoSave();

    return (
        <>
            {hasProject ? <Workspace /> : <Welcome />}

            {/* Global overlays */}
            {activeDialog === "newProject" && <NewProjectDialog />}
            {activeDialog === "settings" && <SettingsDialog />}
            <ConfirmDialog />
            <InputsDialog />
            <Notifications />
        </>
    );
}

export default App;
