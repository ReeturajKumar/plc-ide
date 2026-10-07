import { useCallback, useEffect, useState, type KeyboardEvent, type MouseEvent, type ReactNode } from "react";
import { useProjectStore } from "../../store/projectStore";
import { useEditorStore, isDirty } from "../../store/editorStore";
import { useUIStore } from "../../store/uiStore";
import {
    copyToClipboard,
    createEntry,
    deleteEntry,
    moveProgram,
    openFile,
    projectPath,
    refreshTree,
    renameEntry,
    revealInFileExplorer,
    startNewEntry,
} from "../../store/projectActions";
import { compileAll, compileCurrent, runProgram } from "../../store/runActions";
import ContextMenu from "../common/ContextMenu";
import type { MenuItem } from "../common/Menu";
import type { FileNode } from "../../types/project";
import { FileIcon, FolderIcon } from "../common/FileIcons";
import InlineNameInput from "./InlineNameInput";
import FunctionBlockLibrary from "./FunctionBlockLibrary";
import { parseEntryName, type EntryNameContext } from "../../utils/validation";
import { compileStatus, findNode, isStFile, isWithin, parentPath, type CompileStatus } from "../../utils/project";
import type { PLCProgram } from "../../types/program";

interface OpenMenu {
    x: number;
    y: number;
    items: MenuItem[];
}

const INDENT_PX = 12;
const SEPARATOR: MenuItem = { separator: true, label: "" };

const Chevron = ({ open }: { open: boolean }) => (
    <svg
        className={`w-3.5 h-3.5 shrink-0 text-[var(--theme-text-muted)] transition-transform ${open ? "rotate-90" : ""}`}
        viewBox="0 0 24 24"
        fill="none"
        stroke="currentColor"
        strokeWidth="2"
        aria-hidden="true"
    >
        <polyline points="9 18 15 12 9 6" />
    </svg>
);

const STATUS_STYLE: Record<CompileStatus, { icon: string; className: string }> = {
    "Not Compiled": { icon: "○", className: "text-[var(--theme-text-muted)]" },
    Compiling: { icon: "◌", className: "text-sky-400" },
    Compiled: { icon: "✓", className: "text-emerald-400" },
    Error: { icon: "✗", className: "text-red-400" },
    Modified: { icon: "●", className: "text-amber-400" },
};

/** A toolbar button in the Explorer header (New File, New Folder, …). */
const ToolbarButton = ({ title, onClick, children }: { title: string; onClick: () => void; children: ReactNode }) => (
    <button
        onClick={onClick}
        title={title}
        aria-label={title}
        className="w-5 h-5 flex items-center justify-center rounded text-[var(--theme-text-muted)] hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)] cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-[var(--theme-border-focus)]"
    >
        <svg className="w-3.5 h-3.5" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
            {children}
        </svg>
    </button>
);

const ProjectExplorer = () => {
    const project = useProjectStore((s) => s.project);
    const tree = useProjectStore((s) => s.tree);
    const compiled = useProjectStore((s) => s.compiled);
    const compiling = useProjectStore((s) => s.compiling);
    const fileKinds = useProjectStore((s) => s.fileKinds);
    const activePath = useEditorStore((s) => s.activePath);
    const openFiles = useEditorStore((s) => s.openFiles);
    const explorerWidth = useUIStore((s) => s.explorerWidth);
    const edit = useUIStore((s) => s.explorerEdit);
    const selection = useUIStore((s) => s.explorerSelection);
    const { startExplorerEdit, endExplorerEdit, setExplorerSelection } = useUIStore.getState();
    const [rootOpen, setRootOpen] = useState(true);
    const [expanded, setExpanded] = useState<Set<string>>(() => new Set(["programs"]));
    const [menu, setMenu] = useState<OpenMenu | null>(null);
    const [programsOpen, setProgramsOpen] = useState(true);
    // Path of the program being renamed from the PROGRAMS list.
    const [renamingProgram, setRenamingProgram] = useState<string | null>(null);
    // The name being typed for a new entry, so its icon can follow the extension.
    const [draftName, setDraftName] = useState("");
    const closeMenu = useCallback(() => setMenu(null), []);

    // A different project starts with just its programs folder open.
    const rootPath = project?.rootPath;
    useEffect(() => {
        setExpanded(new Set(["programs"]));
        setRootOpen(true);
    }, [rootPath]);

    // A new entry's box must be visible, so its folder and every folder above it show open.
    const createParent = edit?.mode === "create" ? edit.parent : null;
    useEffect(() => setDraftName(""), [createParent]);

    if (!project) return null;

    const dirtyPaths = new Set(openFiles.filter(isDirty).map((f) => f.path));
    const isOpen = (folder: string) => expanded.has(folder) || (createParent !== null && isWithin(createParent, folder));
    const toggle = (folder: string) =>
        setExpanded((prev) => {
            const next = new Set(prev);
            if (!next.delete(folder)) next.add(folder);
            return next;
        });

    // --- naming rules for the inline box ---

    const childrenOf = (folder: string) => (folder ? (findNode(tree, folder)?.children ?? []) : tree);

    const nameContext = (kind: "file" | "folder", folder: string, renaming?: FileNode): EntryNameContext => ({
        kind,
        siblings: childrenOf(folder).filter((n) => n.path !== renaming?.path).map((n) => n.name),
        programs: project.programs.filter((p) => p.path !== renaming?.path).map((p) => p.name),
        atRoot: folder === "",
        // Renaming keeps an extensionless name extensionless unless the file was a program.
        appendSt: renaming ? isStFile(renaming.path) : true,
    });

    const nameError = (value: string, ctx: EntryNameContext) => {
        const parsed = parseEntryName(value, ctx);
        return "error" in parsed ? parsed.error : null;
    };

    const submitCreate = (value: string, kind: "file" | "folder", folder: string) => {
        endExplorerEdit();
        const parsed = parseEntryName(value, nameContext(kind, folder));
        if ("error" in parsed) return;
        setExplorerSelection(folder ? `${folder}/${parsed.name}` : parsed.name);
        if (kind === "folder") setExpanded((prev) => new Set(prev).add(folder ? `${folder}/${parsed.name}` : parsed.name));
        void createEntry(folder, kind, parsed.name);
    };

    const submitRename = (value: string, node: FileNode) => {
        endExplorerEdit();
        const folder = parentPath(node.path);
        const parsed = parseEntryName(value, nameContext(node.isDir ? "folder" : "file", folder, node));
        if ("error" in parsed) return;
        const to = folder ? `${folder}/${parsed.name}` : parsed.name;
        // Keep a renamed folder open (and its open subfolders).
        setExpanded((prev) => new Set([...prev].map((p) => (isWithin(p, node.path) ? to + p.slice(node.path.length) : p))));
        setExplorerSelection(to);
        void renameEntry(node.path, parsed.name);
    };

    const newIn = (kind: "file" | "folder", folder: string) => startExplorerEdit({ mode: "create", kind, parent: folder });
    const rename = (node: FileNode) => startExplorerEdit({ mode: "rename", path: node.path });

    // RUN executes the whole project, whichever file is open.
    const run = () => void runProgram();

    // --- right-click menus: only actions this IDE actually supports ---

    const pathItems = (relPath: string): MenuItem[] => [
        { label: "Copy Path", onClick: () => void copyToClipboard(projectPath(relPath)) },
        ...(relPath ? [{ label: "Copy Relative Path", onClick: () => void copyToClipboard(relPath) }] : []),
        SEPARATOR,
        { label: "Reveal in File Explorer", onClick: () => void revealInFileExplorer(projectPath(relPath)) },
    ];

    const createItems = (folder: string): MenuItem[] => [
        { label: "New File…", onClick: () => newIn("file", folder) },
        { label: "New Folder…", onClick: () => newIn("folder", folder) },
    ];

    const nodeItems = (node: FileNode): MenuItem[] => [
        ...(node.isDir
            ? createItems(node.path)
            : [
                  { label: "Open", shortcut: "Enter", onClick: () => void openFile(node.path) },
                  ...(isStFile(node.path) ? [{ label: "Run All Programs", shortcut: "F5", onClick: run }] : []),
                  SEPARATOR,
                  ...createItems(parentPath(node.path)),
              ]),
        SEPARATOR,
        { label: "Rename…", shortcut: "F2", onClick: () => rename(node) },
        { label: "Delete", shortcut: "Del", onClick: () => void deleteEntry(node) },
        SEPARATOR,
        ...pathItems(node.path),
    ];

    const rootItems = (): MenuItem[] => [
        ...createItems(""),
        SEPARATOR,
        ...pathItems(""),
        { label: "Refresh", onClick: () => void refreshTree() },
    ];

    const showMenu = (e: MouseEvent, items: MenuItem[], select: string | null) => {
        e.preventDefault();
        e.stopPropagation(); // the innermost row decides the menu
        setExplorerSelection(select);
        setMenu({ x: e.clientX, y: e.clientY, items });
    };

    const activate = (node: FileNode) => {
        setExplorerSelection(node.path);
        if (node.isDir) toggle(node.path);
        else void openFile(node.path);
    };

    const onRowKey = (e: KeyboardEvent<HTMLDivElement>, node: FileNode) => {
        if (e.key === "Enter") activate(node);
        else if (e.key === "F2") rename(node);
        else if (e.key === "Delete") void deleteEntry(node);
        else if (e.key === "ArrowRight" && node.isDir && !isOpen(node.path)) toggle(node.path);
        else if (e.key === "ArrowLeft" && node.isDir && isOpen(node.path)) toggle(node.path);
        else if (e.key === "ContextMenu" || (e.key === "F10" && e.shiftKey)) {
            const rect = e.currentTarget.getBoundingClientRect();
            setExplorerSelection(node.path);
            setMenu({ x: rect.left + 24, y: rect.bottom, items: nodeItems(node) });
        } else return;
        e.preventDefault();
    };

    // --- the PROGRAMS list: execution order and compilation state ---

    const newProgram = () => {
        // New programs go next to the existing ones (programs/ in a new project).
        const first = project.programs[0];
        newIn("file", first ? parentPath(first.path) : findNode(tree, "programs")?.isDir ? "programs" : "");
    };

    const compileProgram = async (path: string) => {
        await openFile(path); // Compile works on the program open in the editor
        await compileCurrent();
    };

    const submitProgramRename = (value: string, node: FileNode) => {
        setRenamingProgram(null);
        submitRename(value, node);
    };

    const programItems = (program: PLCProgram, index: number): MenuItem[] => {
        const node = findNode(tree, program.path);
        return [
            { label: "Open", onClick: () => void openFile(program.path) },
            { label: "Compile", shortcut: "Ctrl+F7", onClick: () => void compileProgram(program.path) },
            { label: "Compile All", shortcut: "F7", onClick: () => void compileAll() },
            { label: "Run All Programs", shortcut: "F5", onClick: run },
            SEPARATOR,
            { label: "Move Up", onClick: () => void moveProgram(program.path, -1), disabled: index === 0 },
            { label: "Move Down", onClick: () => void moveProgram(program.path, 1), disabled: index === project.programs.length - 1 },
            SEPARATOR,
            { label: "Rename…", onClick: () => setRenamingProgram(program.path), disabled: !node },
            { label: "Delete", onClick: () => node && void deleteEntry(node), disabled: !node },
        ];
    };

    const programRows = project.programs.map((program, index) => {
        const source = openFiles.find((f) => f.path === program.path)?.content;
        const record = compiled[program.path];
        const status = compileStatus(record, source, compiling.includes(program.path));
        const style = STATUS_STYLE[status];
        const node = findNode(tree, program.path);
        const label = status === "Error" && record ? `Error (${record.errorCount})` : status;
        // A file with only FUNCTION_BLOCKs defines types; it isn't run in the scan.
        const kind = fileKinds[program.path];
        const blocksOnly = kind !== undefined && !kind.hasProgram;
        return (
            <div
                key={program.path}
                role="listitem"
                tabIndex={0}
                onClick={() => void openFile(program.path)}
                onKeyDown={(e) => {
                    if (e.key === "Enter") void openFile(program.path);
                    else if (e.key === "F2" && node) setRenamingProgram(program.path);
                    else return;
                    e.preventDefault();
                }}
                onContextMenu={(e) => showMenu(e, programItems(program, index), program.path)}
                title={
                    blocksOnly
                        ? `${program.path} — function block${kind.functionBlocks.length === 1 ? "" : "s"} ${kind.functionBlocks.map((f) => f.name).join(", ")} (not run in the scan) — ${status}`
                        : `${index + 1}. ${program.path} — ${status}`
                }
                className={`flex items-center gap-1.5 py-0.5 pl-3 pr-2 cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[var(--theme-border-focus)] ${
                    activePath === program.path
                        ? "bg-[var(--theme-bg-active)] text-[var(--theme-text-primary)]"
                        : "hover:bg-[var(--theme-bg-hover)]"
                }`}
            >
                <span className="w-4 shrink-0 text-right text-[11px] tabular-nums text-[var(--theme-text-muted)]">
                    {blocksOnly ? <span className="text-[9px] font-semibold text-sky-300">FB</span> : index + 1}
                </span>
                <FileIcon fileName={program.fileName} />
                {renamingProgram === program.path && node ? (
                    <InlineNameInput
                        initialValue={node.name}
                        validate={(value) => nameError(value, nameContext("file", parentPath(node.path), node))}
                        onSubmit={(value) => submitProgramRename(value, node)}
                        onCancel={() => setRenamingProgram(null)}
                    />
                ) : (
                    <>
                        <span className="truncate flex-1">
                            {program.name}
                            {dirtyPaths.has(program.path) && <span className="ml-1 text-[var(--theme-text-muted)]">●</span>}
                        </span>
                        <span className={`shrink-0 text-[11px] ${style.className}`}>
                            {style.icon} {label}
                        </span>
                    </>
                )}
            </div>
        );
    });

    // --- tree rendering ---

    const indent = (depth: number) => ({ paddingLeft: 8 + depth * INDENT_PX });

    const createRow = (folder: string, depth: number) => {
        if (edit?.mode !== "create" || edit.parent !== folder) return null;
        const kind = edit.kind;
        const ctx = nameContext(kind, folder);
        return (
            <div className="flex items-center gap-1.5 py-0.5 pr-2" style={indent(depth)}>
                <span className="w-3.5 shrink-0" />
                {kind === "folder" ? (
                    <FolderIcon name={draftName} />
                ) : (
                    <FileIcon fileName={draftName.includes(".") ? draftName : `${draftName}.st`} />
                )}
                <InlineNameInput
                    initialValue=""
                    placeholder={kind === "folder" ? "folder name" : "name.st"}
                    validate={(value) => nameError(value, ctx)}
                    onSubmit={(value) => submitCreate(value, kind, folder)}
                    onCancel={endExplorerEdit}
                    onValueChange={setDraftName}
                />
            </div>
        );
    };

    const renderNodes = (nodes: FileNode[], folder: string, depth: number): ReactNode => (
        <>
            {createRow(folder, depth)}
            {nodes.map((node) => {
                const open = node.isDir && isOpen(node.path);
                const renaming = edit?.mode === "rename" && edit.path === node.path;
                const selected = selection === node.path;
                const active = activePath === node.path;
                return (
                    <div key={node.path} role="none">
                        <div
                            role="treeitem"
                            tabIndex={0}
                            aria-selected={selected}
                            aria-expanded={node.isDir ? open : undefined}
                            onClick={() => activate(node)}
                            onContextMenu={(e) => showMenu(e, nodeItems(node), node.path)}
                            onKeyDown={(e) => onRowKey(e, node)}
                            title={node.path}
                            className={`flex items-center gap-1.5 py-0.5 pr-2 cursor-pointer focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[var(--theme-border-focus)] ${
                                active || selected
                                    ? "bg-[var(--theme-bg-active)] text-[var(--theme-text-primary)]"
                                    : "hover:bg-[var(--theme-bg-hover)]"
                            }`}
                            style={indent(depth)}
                        >
                            {node.isDir ? <Chevron open={open} /> : <span className="w-3.5 shrink-0" />}
                            {node.isDir ? <FolderIcon name={node.name} open={open} /> : <FileIcon fileName={node.name} />}
                            {renaming ? (
                                <InlineNameInput
                                    initialValue={node.name}
                                    validate={(value) =>
                                        nameError(value, nameContext(node.isDir ? "folder" : "file", parentPath(node.path), node))
                                    }
                                    onSubmit={(value) => submitRename(value, node)}
                                    onCancel={endExplorerEdit}
                                />
                            ) : (
                                <span className="truncate flex-1">
                                    {node.name}
                                    {dirtyPaths.has(node.path) && <span className="ml-1 text-[var(--theme-text-muted)]">●</span>}
                                </span>
                            )}
                        </div>
                        {open && <div role="group">{renderNodes(node.children, node.path, depth + 1)}</div>}
                    </div>
                );
            })}
        </>
    );

    return (
        <aside
            style={{ width: explorerWidth }}
            className="shrink-0 theme-panel border-r flex flex-col select-none text-[13px] text-[var(--theme-text-secondary)]"
        >
            <div className="h-9 flex items-center justify-between pl-4 pr-2 text-[11px] font-semibold tracking-wider text-[var(--theme-text-muted)] uppercase">
                <span>Explorer</span>
                <div className="flex items-center gap-0.5">
                    <ToolbarButton title="New File…" onClick={() => startNewEntry("file")}>
                        <path d="M14.5 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7.5L14.5 2z" />
                        <polyline points="14 2 14 8 20 8" />
                        <line x1="12" y1="12" x2="12" y2="18" />
                        <line x1="9" y1="15" x2="15" y2="15" />
                    </ToolbarButton>
                    <ToolbarButton title="New Folder…" onClick={() => startNewEntry("folder")}>
                        <path d="M4 20h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.7-.9l-.8-1.2A2 2 0 0 0 7.9 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2z" />
                        <line x1="12" y1="10" x2="12" y2="16" />
                        <line x1="9" y1="13" x2="15" y2="13" />
                    </ToolbarButton>
                    <ToolbarButton title="Refresh Explorer" onClick={() => void refreshTree()}>
                        <polyline points="23 4 23 10 17 10" />
                        <path d="M20.5 15a9 9 0 1 1-2.1-9.4L23 10" />
                    </ToolbarButton>
                    <ToolbarButton title="Collapse Folders" onClick={() => setExpanded(new Set())}>
                        <rect x="3" y="3" width="18" height="18" rx="2" />
                        <line x1="8" y1="12" x2="16" y2="12" />
                    </ToolbarButton>
                </div>
            </div>

            <div
                role="tree"
                aria-label="Project files"
                className="flex-1 overflow-y-auto pb-2"
                onClick={() => setExplorerSelection(null)}
                onContextMenu={(e) => showMenu(e, rootItems(), null)}
            >
                {/* Root project folder */}
                <div
                    onClick={(e) => {
                        e.stopPropagation();
                        setRootOpen(!rootOpen);
                    }}
                    onContextMenu={(e) => showMenu(e, rootItems(), null)}
                    title={project.rootPath}
                    className="flex items-center gap-1 px-2 py-1 cursor-pointer hover:bg-[var(--theme-bg-hover)] text-[var(--theme-text-primary)] font-semibold text-[11px] tracking-wide uppercase"
                >
                    <Chevron open={rootOpen || createParent !== null} />
                    <span className="truncate">{project.name}</span>
                </div>

                {(rootOpen || createParent !== null) && (
                    <div role="group" onClick={(e) => e.stopPropagation()}>
                        {renderNodes(tree, "", 0)}
                        {tree.length === 0 && createParent === null && (
                            <p className="px-4 py-2 text-[12px] text-[var(--theme-text-muted)]">
                                This project is empty. Right-click to create a file or folder.
                            </p>
                        )}
                    </div>
                )}
            </div>

            {/* Programs: run in this order every scan */}
            <section aria-label="Programs" className="shrink-0 max-h-[45%] flex flex-col border-t border-[var(--theme-border)]">
                <div className="h-7 shrink-0 flex items-center justify-between pl-2 pr-2 text-[11px] font-semibold tracking-wider text-[var(--theme-text-muted)] uppercase">
                    <button
                        onClick={() => setProgramsOpen(!programsOpen)}
                        className="flex items-center gap-1 cursor-pointer hover:text-[var(--theme-text-primary)]"
                        aria-expanded={programsOpen}
                        title="Programs, in execution order"
                    >
                        <Chevron open={programsOpen} />
                        <span>Programs</span>
                        <span className="font-normal normal-case tracking-normal">({project.programs.length})</span>
                    </button>
                    <div className="flex items-center gap-0.5">
                        <ToolbarButton title="New Program…" onClick={newProgram}>
                            <line x1="12" y1="5" x2="12" y2="19" />
                            <line x1="5" y1="12" x2="19" y2="12" />
                        </ToolbarButton>
                        <ToolbarButton title="Compile All (F7)" onClick={() => void compileAll()}>
                            <polyline points="20 6 9 17 4 12" />
                        </ToolbarButton>
                    </div>
                </div>
                {programsOpen && (
                    <div role="list" aria-label="Programs in execution order" className="overflow-y-auto pb-2">
                        {programRows}
                        {project.programs.length === 0 && (
                            <p className="px-4 py-1 text-[12px] text-[var(--theme-text-muted)]">No programs yet. Click + to create one.</p>
                        )}
                    </div>
                )}
            </section>

            <FunctionBlockLibrary />

            {menu && <ContextMenu x={menu.x} y={menu.y} items={menu.items} onClose={closeMenu} />}
        </aside>
    );
};

export default ProjectExplorer;
