import { open } from "@tauri-apps/plugin-dialog";
import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { fsApi } from "../utils/tauri";
import {
    baseName,
    childPath,
    createProjectModel,
    defaultFunctionBlockSource,
    defaultProgramSource,
    DEFAULT_MAIN_ST,
    filePaths,
    fromProjectFile,
    isStFile,
    isBuiltinPath,
    isWithin,
    joinPath,
    movePrograms,
    parentPath,
    reconcilePrograms,
    remapPrograms,
    targetFolder,
    toProjectFile,
} from "../utils/project";
import { RESERVED_ROOT_NAMES, validateProjectName } from "../utils/validation";
import type { FileNode, PLCProject } from "../types/project";
import { useProjectStore } from "./projectStore";
import { useEditorStore, isDirty } from "./editorStore";
import { useUIStore } from "./uiStore";
import { checkIoMappings, stopIfActive, syncBreakpoints } from "./runActions";

const RECENTS_KEY = "myplc.recentProjects";

export interface RecentProject {
    name: string;
    rootPath: string;
}

export function getRecentProjects(): RecentProject[] {
    try {
        return JSON.parse(localStorage.getItem(RECENTS_KEY) ?? "[]");
    } catch {
        return [];
    }
}

function rememberProject(name: string, rootPath: string): void {
    const others = getRecentProjects().filter((r) => r.rootPath !== rootPath);
    const next = [{ name, rootPath }, ...others].slice(0, 8);
    localStorage.setItem(RECENTS_KEY, JSON.stringify(next));
}

const msg = (e: unknown): string => (typeof e === "string" ? e : "Something went wrong.");

/** Turn friendly Rust errors into a notification and return false. */
function fail(e: unknown): false {
    useUIStore.getState().notify("error", msg(e));
    return false;
}

/** Replace the workspace with `project`: read its files and open its first program. */
async function loadWorkspace(project: PLCProject) {
    await stopIfActive();
    useUIStore.getState().endExplorerEdit();
    useUIStore.getState().setExplorerSelection(null);
    useEditorStore.getState().closeAll();
    useProjectStore.getState().setProject(project);
    rememberProject(project.name, project.rootPath);
    await refreshTree();
    const first = useProjectStore.getState().project?.programs[0];
    if (first) await openFile(first.path);
}

/** Create a new project on disk and load it into the workspace. */
export async function newProject(name: string, location: string): Promise<boolean> {
    const nameError = validateProjectName(name);
    if (nameError) return fail(nameError);
    if (!location) return fail("Please choose a project location.");

    const rootPath = joinPath(location, name.trim());
    const project = createProjectModel(name, rootPath);
    try {
        await fsApi.createProject(rootPath, toProjectFile(project), DEFAULT_MAIN_ST);
    } catch (e) {
        return fail(e);
    }
    await loadWorkspace(project);
    useUIStore.getState().notify("success", `Created project "${project.name}".`);
    return true;
}

/** Prompt for a folder, then open the project it contains. */
export async function openProjectDialog(): Promise<boolean> {
    const selected = await open({ directory: true, title: "Open MyPLC Project" });
    if (typeof selected !== "string") return false;
    return openProjectFromPath(selected);
}

export async function openProjectFromPath(rootPath: string): Promise<boolean> {
    let json: string;
    try {
        json = await fsApi.readProject(rootPath);
    } catch (e) {
        return fail(e);
    }
    let project;
    try {
        project = fromProjectFile(json, rootPath);
    } catch {
        return fail("The project file is corrupted or unreadable.");
    }
    await loadWorkspace(project);
    return true;
}

/**
 * Re-read the project folder, then bring the program registry in project.json in line
 * with the .st files found. Called after every file operation and when the window
 * regains focus, so changes made outside the IDE show up too.
 */
export async function refreshTree(): Promise<void> {
    const project = useProjectStore.getState().project;
    if (!project) return;
    let tree: FileNode[];
    try {
        tree = await fsApi.readTree(project.rootPath);
    } catch (e) {
        fail(e);
        return;
    }
    if (await adoptProjectFile(project.rootPath)) {
        await checkIoMappings(); // validate, and apply to a running PLC
        await syncBreakpoints();
    }
    const store = useProjectStore.getState();
    if (store.project?.rootPath !== project.rootPath) return; // switched projects meanwhile
    // MyPLC's own files stay out of the Explorer, so they can't be edited or deleted there.
    const visible = tree.filter((n) => !RESERVED_ROOT_NAMES.includes(n.name.toLowerCase()));
    store.setTree(visible);
    const programs = reconcilePrograms(store.project.programs, filePaths(visible).filter(isStFile));
    if (programs) {
        store.setPrograms(programs);
        await persistProjectJson();
    }
}

/**
 * project.json may have been edited outside the IDE (by hand, a tool, git): take its I/O
 * mappings and breakpoints, so the open project doesn't keep stale ones and later save
 * them over the file. Only when the IDE has no unsaved project changes of its own (it
 * saves those right away). Returns whether anything changed.
 */
async function adoptProjectFile(rootPath: string): Promise<boolean> {
    let disk: PLCProject;
    try {
        disk = fromProjectFile(await fsApi.readProject(rootPath), rootPath);
    } catch {
        return false; // unreadable right now: keep what the IDE has
    }
    const store = useProjectStore.getState();
    const current = store.project;
    if (!current || current.rootPath !== rootPath || store.dirty) return false;
    const same = (a: unknown, b: unknown) => JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
    if (same(current.io, disk.io) && same(current.breakpoints, disk.breakpoints)) return false;
    store.adoptExternal({ io: disk.io, breakpoints: disk.breakpoints });
    return true;
}

/** Open a project file in a tab (or focus it if already open). */
export async function openFile(path: string): Promise<void> {
    const project = useProjectStore.getState().project;
    if (!project) return;

    const editor = useEditorStore.getState();
    if (editor.openFiles.some((f) => f.path === path)) {
        editor.setActive(path);
        return;
    }
    try {
        const content = await fsApi.readFile(project.rootPath, path);
        editor.openFile({ path, fileName: baseName(path), savedContent: content, content });
    } catch (e) {
        fail(e);
    }
}

/** Persist project.json (call after any registry change). */
export async function persistProjectJson(): Promise<boolean> {
    const project = useProjectStore.getState().project;
    if (!project) return false;
    try {
        await fsApi.writeProjectJson(project.rootPath, toProjectFile(project));
        useProjectStore.getState().markSaved();
        return true;
    } catch (e) {
        return fail(e);
    }
}

/** Save the active editor tab to disk. */
export async function saveActive(): Promise<boolean> {
    const { project } = useProjectStore.getState();
    const editor = useEditorStore.getState();
    const active = editor.openFiles.find((f) => f.path === editor.activePath);
    if (!project || !active) return false;
    if (!isDirty(active) || isBuiltinPath(active.path)) return true;
    try {
        await fsApi.writeFile(project.rootPath, active.path, active.content);
        editor.markFileSaved(active.path);
        return true;
    } catch (e) {
        return fail(e);
    }
}

/** Save every dirty open tab. */
export async function saveAll(): Promise<boolean> {
    const { project } = useProjectStore.getState();
    const editor = useEditorStore.getState();
    if (!project) return false;
    for (const f of editor.openFiles) {
        if (!isDirty(f) || isBuiltinPath(f.path)) continue;
        try {
            await fsApi.writeFile(project.rootPath, f.path, f.content);
            editor.markFileSaved(f.path);
        } catch (e) {
            return fail(e);
        }
    }
    return true;
}

/**
 * Create a file or folder named `name` (already validated) inside `parent`. A new `.st`
 * file starts as an empty program named after it; new files open in the editor.
 */
export async function createEntry(parent: string, kind: "file" | "folder", name: string): Promise<boolean> {
    const project = useProjectStore.getState().project;
    if (!project) return false;
    const path = childPath(parent, name);
    try {
        if (kind === "folder") await fsApi.createDir(project.rootPath, path);
        else {
            const pou = name.replace(/\.st$/i, "");
            // New files in the function-blocks folder start as a FUNCTION_BLOCK.
            const template = /(^|\/)function-blocks$/i.test(parent) ? defaultFunctionBlockSource : defaultProgramSource;
            const content = isStFile(name) ? template(pou) : "";
            await fsApi.createFile(project.rootPath, path, content);
        }
    } catch (e) {
        return fail(e);
    }
    await refreshTree();
    if (kind === "file") await openFile(path);
    return true;
}

/** Start an inline New File / New Folder in the folder targeted by the Explorer selection. */
export function startNewEntry(kind: "file" | "folder"): void {
    const ui = useUIStore.getState();
    const parent = targetFolder(useProjectStore.getState().tree, ui.explorerSelection);
    ui.startExplorerEdit({ mode: "create", kind, parent });
}

/** Rename a file or folder in place. Open tabs and program ids follow the new path. */
export async function renameEntry(path: string, newName: string): Promise<boolean> {
    const project = useProjectStore.getState().project;
    if (!project) return false;
    const to = childPath(parentPath(path), newName);
    if (to === path) return true;
    try {
        await fsApi.renameFile(project.rootPath, path, to);
    } catch (e) {
        return fail(e);
    }
    useEditorStore.getState().remapPaths(path, to);
    useProjectStore.getState().setPrograms(remapPrograms(project.programs, path, to));
    await persistProjectJson();
    await refreshTree();
    return true;
}

/** Move a program up (-1) or down (+1) in the execution order, and save it in project.json. */
export async function moveProgram(path: string, delta: number): Promise<void> {
    const project = useProjectStore.getState().project;
    if (!project) return;
    const next = movePrograms(project.programs, project.programs.findIndex((p) => p.path === path), delta);
    if (!next) return;
    useProjectStore.getState().setPrograms(next);
    await persistProjectJson();
}

/** Map `address` to `variable` (empty: unmap it), save it in project.json, and apply it. */
export async function setIoMapping(address: string, variable: string): Promise<void> {
    const project = useProjectStore.getState().project;
    if (!project) return;
    const others = (project.io?.mappings ?? []).filter((m) => m.address !== address);
    const name = variable.trim();
    useProjectStore.getState().setIoMappings(name ? [...others, { address, variable: name }] : others);
    await persistProjectJson();
    await checkIoMappings(); // validate now, and apply to a running PLC
}

/** Remove every I/O mapping (after confirmation), save, and apply. */
export async function clearIoMappings(): Promise<void> {
    const count = useProjectStore.getState().project?.io?.mappings.length ?? 0;
    if (count === 0) return;
    const choice = await useUIStore.getState().confirmUnsaved({
        title: "Delete all I/O mappings",
        message: `Remove all ${count} I/O mapping${count === 1 ? "" : "s"}? Programs can still use DI/DO/AI/AO directly.`,
    });
    if (choice !== "discard") return; // the dialog's Delete button
    useProjectStore.getState().setIoMappings([]);
    await persistProjectJson();
    await checkIoMappings();
}

/** Delete a file, or a folder and everything in it, after confirmation. */
export async function deleteEntry(node: FileNode): Promise<boolean> {
    const project = useProjectStore.getState().project;
    if (!project) return false;

    const unsaved = useEditorStore.getState().openFiles.some((f) => isWithin(f.path, node.path) && isDirty(f));
    const what = node.isDir ? `the folder "${node.name}" and everything in it` : `"${node.name}"`;
    const choice = await useUIStore.getState().confirmUnsaved({
        title: node.isDir ? "Delete folder" : "Delete file",
        message: `Delete ${what}? This cannot be undone.${unsaved ? " Unsaved changes in open files will be lost." : ""}`,
    });
    if (choice !== "discard") return false; // "Delete" maps to the discard action.

    try {
        await fsApi.deleteEntry(project.rootPath, node.path);
    } catch (e) {
        return fail(e);
    }
    useEditorStore.getState().closeWithin(node.path);
    await refreshTree(); // drops the deleted programs from the registry
    return true;
}

/** Close a tab, prompting to save if it has unsaved changes. */
export async function closeTab(path: string): Promise<void> {
    const editor = useEditorStore.getState();
    const file = editor.openFiles.find((f) => f.path === path);
    if (!file) return;

    if (isDirty(file)) {
        const choice = await useUIStore.getState().confirmUnsaved({
            title: "Unsaved changes",
            message: `Save changes to ${file.fileName} before closing?`,
        });
        if (choice === "cancel") return;
        if (choice === "save") {
            editor.setActive(path);
            const ok = await saveActive();
            if (!ok) return;
        }
    }
    editor.closeFile(path);
}

/** Close the whole project, prompting once if anything is unsaved. Returns false if cancelled. */
export async function closeProject(): Promise<boolean> {
    const editor = useEditorStore.getState();
    const anyDirty = editor.openFiles.some(isDirty);
    if (anyDirty) {
        const choice = await useUIStore.getState().confirmUnsaved({
            title: "Unsaved changes",
            message: "Save changes before closing the project?",
        });
        if (choice === "cancel") return false;
        if (choice === "save" && !(await saveAll())) return false;
    }
    await stopIfActive();
    useUIStore.getState().endExplorerEdit();
    editor.closeAll();
    useProjectStore.getState().closeProject();
    return true;
}

/** Absolute path of something inside the open project, e.g. `programs/main.st`. */
export function projectPath(relPath = ""): string {
    const root = useProjectStore.getState().project?.rootPath ?? "";
    return relPath ? joinPath(root, relPath) : root;
}

export async function copyToClipboard(text: string): Promise<void> {
    try {
        await navigator.clipboard.writeText(text);
        useUIStore.getState().notify("info", `Copied ${text}`);
    } catch (e) {
        console.error("[clipboard]", e);
        fail("Couldn't copy to the clipboard.");
    }
}

/** Open the OS file manager with `path` selected. */
export async function revealInFileExplorer(path: string): Promise<void> {
    try {
        await revealItemInDir(path);
    } catch (e) {
        console.error("[reveal]", e);
        fail("Couldn't open the file explorer for this item.");
    }
}
