import { useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import Dialog from "../common/Dialog";
import Button from "../common/Button";
import Input from "../common/Input";
import { validateProjectName } from "../../utils/validation";
import { joinPath } from "../../utils/project";
import { newProject } from "../../store/projectActions";
import { useUIStore } from "../../store/uiStore";

const NewProjectDialog = () => {
    const closeDialog = useUIStore((s) => s.closeDialog);
    const [name, setName] = useState("");
    const [location, setLocation] = useState("");
    const [touched, setTouched] = useState(false);
    const [busy, setBusy] = useState(false);

    const nameError = touched ? validateProjectName(name) : null;
    const canCreate = !validateProjectName(name) && !!location && !busy;

    const chooseLocation = async () => {
        const selected = await open({ directory: true, title: "Select project location" });
        if (typeof selected === "string") setLocation(selected);
    };

    const create = async () => {
        setTouched(true);
        if (!canCreate) return;
        setBusy(true);
        const ok = await newProject(name, location);
        setBusy(false);
        if (ok) closeDialog();
    };

    return (
        <Dialog
            title="New Project"
            onClose={busy ? undefined : closeDialog}
            footer={
                <>
                    <Button variant="secondary" onClick={closeDialog} disabled={busy}>
                        Cancel
                    </Button>
                    <Button onClick={create} disabled={!canCreate}>
                        {busy ? "Creating…" : "Create"}
                    </Button>
                </>
            }
        >
            <div className="flex flex-col gap-4">
                <Input
                    id="project-name"
                    label="Project name"
                    placeholder="MyPLCProject"
                    value={name}
                    autoFocus
                    onChange={(e) => setName(e.target.value)}
                    onBlur={() => setTouched(true)}
                    onKeyDown={(e) => e.key === "Enter" && create()}
                    error={nameError}
                />

                <div className="flex flex-col gap-1">
                    <span className="text-[11px] font-medium text-[var(--theme-text-secondary)]">
                        Location
                    </span>
                    <div className="flex gap-2">
                        <input
                            readOnly
                            value={location}
                            placeholder="Choose a folder…"
                            className="flex-1 bg-[var(--theme-bg-input)] border border-[var(--theme-border)] rounded-md px-3 py-1.5 text-xs text-[var(--theme-text-primary)] placeholder:text-[var(--theme-text-muted)]"
                        />
                        <Button variant="secondary" onClick={chooseLocation} disabled={busy}>
                            Browse…
                        </Button>
                    </div>
                    {location && (
                        <span className="text-[11px] text-[var(--theme-text-muted)] truncate">
                            Will create: {joinPath(location, name.trim() || "…")}
                        </span>
                    )}
                </div>
            </div>
        </Dialog>
    );
};

export default NewProjectDialog;
