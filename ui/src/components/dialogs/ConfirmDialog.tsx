import Dialog from "../common/Dialog";
import Button from "../common/Button";
import { useUIStore } from "../../store/uiStore";

/**
 * Renders the active Save / Don't Save / Cancel prompt driven by uiStore.confirmUnsaved().
 * Reused for deletes, where "Don't Save" reads as the destructive confirm.
 */
const ConfirmDialog = () => {
    const prompt = useUIStore((s) => s.confirmPrompt);
    const resolve = useUIStore((s) => s.resolveConfirm);
    if (!prompt) return null;

    const isDelete = prompt.title.startsWith("Delete");

    return (
        <Dialog
            title={prompt.title}
            onClose={() => resolve("cancel")}
            footer={
                <>
                    <Button variant="secondary" onClick={() => resolve("cancel")}>
                        Cancel
                    </Button>
                    <Button variant="secondary" onClick={() => resolve("discard")}>
                        {isDelete ? "Delete" : "Don't Save"}
                    </Button>
                    {!isDelete && <Button onClick={() => resolve("save")}>Save</Button>}
                </>
            }
        >
            <p className="text-xs text-[var(--theme-text-secondary)]">{prompt.message}</p>
        </Dialog>
    );
};

export default ConfirmDialog;
