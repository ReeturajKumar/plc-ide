import { useEffect, useState, type MouseEvent } from "react";
import { useEditorStore } from "../../store/editorStore";
import { openFile } from "../../store/projectActions";
import { revealPosition } from "../../utils/editorRef";
import { BUILTIN_PREFIX } from "../../utils/project";
import ContextMenu from "../common/ContextMenu";
import { useProjectStore } from "../../store/projectStore";
import { useUIStore } from "../../store/uiStore";
import { refreshFileKinds } from "../../store/runActions";
import { compilerApi } from "../../services/runtimeApi";
import { insertFunctionBlock } from "../../utils/editorRef";
import { FB_DRAG_TYPE } from "../../utils/fbInsert";
import type { FbSummary } from "../../types/runtime";

// The standard blocks never change: fetch them once per session.
let standardCache: FbSummary[] | null = null;

const names = (vars: FbSummary["inputs"]) => vars.map((v) => v.name).join(", ") || "—";

/**
 * Show a block's definition: a standard block's ST reference in a read-only tab, or the
 * project file that defines one of your blocks, at its FUNCTION_BLOCK line.
 */
async function openDefinition(fb: FbSummary): Promise<void> {
    if (fb.source !== null) {
        const path = `${BUILTIN_PREFIX}${fb.name}.st`;
        const editor = useEditorStore.getState();
        if (!editor.openFiles.some((f) => f.path === path)) {
            editor.openFile({ path, fileName: `${fb.name}.st`, content: fb.source, savedContent: fb.source });
        }
        editor.setActive(path);
        const line = fb.source.split("\n").findIndex((l) => l.startsWith("FUNCTION_BLOCK")) + 1;
        setTimeout(() => revealPosition(line || 1, 1), 50); // after the tab's model is shown
        return;
    }
    const kinds = useProjectStore.getState().fileKinds;
    const file = Object.entries(kinds).find(([, kind]) => kind.functionBlocks.some((f) => f.name === fb.name))?.[0];
    if (!file) return;
    await openFile(file);
    setTimeout(() => revealPosition(fb.line, 1), 50);
}

const LibraryItem = ({ fb }: { fb: FbSummary }) => {
    const [menu, setMenu] = useState<{ x: number; y: number } | null>(null);
    const insert = () => {
        const problem = insertFunctionBlock(fb);
        if (problem) useUIStore.getState().notify("info", problem);
    };
    const showMenu = (e: MouseEvent) => {
        e.preventDefault();
        setMenu({ x: e.clientX, y: e.clientY });
    };
    return (
        <>
        <div
            role="listitem"
            tabIndex={0}
            draggable
            onDragStart={(e) => {
                e.dataTransfer.setData(FB_DRAG_TYPE, JSON.stringify(fb));
                e.dataTransfer.effectAllowed = "copy";
            }}
            onDoubleClick={insert}
            onContextMenu={showMenu}
            onKeyDown={(e) => {
                if (e.key === "Enter") insert();
                else if (e.key === "F12") void openDefinition(fb);
            }}
            title={[
                `${fb.name}`,
                `Inputs: ${fb.inputs.map((v) => `${v.name} : ${v.dataType}`).join(", ") || "none"}`,
                `Outputs: ${fb.outputs.map((v) => `${v.name} : ${v.dataType}`).join(", ") || "none"}`,
                "Drag into the editor, or double-click to insert at the cursor.",
                "Click { } (or press F12) to see its definition.",
            ].join("\n")}
            className="group flex items-center gap-1.5 py-0.5 pl-3 pr-1 cursor-grab active:cursor-grabbing hover:bg-[var(--theme-bg-hover)] focus:outline-none focus-visible:ring-1 focus-visible:ring-inset focus-visible:ring-[var(--theme-border-focus)]"
        >
            <span className="w-6 shrink-0 rounded bg-sky-500/15 text-center text-[9px] font-semibold text-sky-300">FB</span>
            <span className="text-[var(--theme-text-primary)]">{fb.name}</span>
            <span className="flex-1 truncate text-[11px] text-[var(--theme-text-muted)]">
                {names(fb.inputs)} → {names(fb.outputs)}
            </span>
            <button
                onClick={(e) => {
                    e.stopPropagation();
                    void openDefinition(fb);
                }}
                onDoubleClick={(e) => e.stopPropagation()}
                title={fb.source !== null ? "Show Definition (read-only)" : "Go to Definition"}
                aria-label={`Show the definition of ${fb.name}`}
                className="shrink-0 px-1 rounded text-[10px] font-semibold text-[var(--theme-text-muted)] opacity-60 group-hover:opacity-100 hover:text-[var(--theme-text-primary)] hover:bg-[var(--theme-bg-hover)] cursor-pointer"
            >
                {"{ }"}
            </button>
        </div>
        {menu && (
            <ContextMenu
                x={menu.x}
                y={menu.y}
                onClose={() => setMenu(null)}
                items={[
                    { label: "Insert into Editor", shortcut: "Enter", onClick: insert },
                    { label: fb.source !== null ? "Show Definition" : "Go to Definition", shortcut: "F12", onClick: () => void openDefinition(fb) },
                ]}
            />
        )}
        </>
    );
};

const Group = ({ title, items, empty }: { title: string; items: FbSummary[]; empty: string }) => (
    <div role="group" aria-label={title}>
        <div className="pl-3 pt-1 pb-0.5 text-[10px] font-semibold tracking-wider text-[var(--theme-text-muted)] uppercase">{title}</div>
        {items.map((fb) => (
            <LibraryItem key={fb.name} fb={fb} />
        ))}
        {items.length === 0 && <p className="pl-3 pr-2 py-0.5 text-[11px] text-[var(--theme-text-muted)]">{empty}</p>}
    </div>
);

/**
 * The function block library: the standard blocks and the project's FUNCTION_BLOCKs.
 * Drag one into the editor (or double-click it) to declare an instance and call it.
 */
const FunctionBlockLibrary = () => {
    const [open, setOpen] = useState(true);
    const [standard, setStandard] = useState<FbSummary[]>(standardCache ?? []);
    const fileKinds = useProjectStore((s) => s.fileKinds);
    const programs = useProjectStore((s) => s.project?.programs);

    useEffect(() => {
        if (standardCache) return;
        compilerApi
            .standardFunctionBlocks()
            .then((list) => setStandard((standardCache = list)))
            .catch((e) => console.error("[library]", e));
    }, []);

    // Learn the project's blocks when the files change (they also update as you type).
    const fileList = programs?.map((p) => p.path).join("\n");
    useEffect(() => {
        void refreshFileKinds();
    }, [fileList]);

    const project = Object.values(fileKinds)
        .flatMap((k) => k.functionBlocks)
        .sort((a, b) => a.name.localeCompare(b.name));

    return (
        <section aria-label="Function blocks" className="shrink-0 max-h-[40%] flex flex-col border-t border-[var(--theme-border)]">
            <div className="h-7 shrink-0 flex items-center pl-2 pr-2 text-[11px] font-semibold tracking-wider text-[var(--theme-text-muted)] uppercase">
                <button
                    onClick={() => setOpen(!open)}
                    aria-expanded={open}
                    title="Drag a function block into the editor, or double-click it"
                    className="flex items-center gap-1 cursor-pointer hover:text-[var(--theme-text-primary)]"
                >
                    <svg
                        className={`w-3.5 h-3.5 shrink-0 transition-transform ${open ? "rotate-90" : ""}`}
                        viewBox="0 0 24 24"
                        fill="none"
                        stroke="currentColor"
                        strokeWidth="2"
                        aria-hidden="true"
                    >
                        <polyline points="9 18 15 12 9 6" />
                    </svg>
                    <span>Function Blocks</span>
                </button>
            </div>
            {open && (
                <div role="list" aria-label="Function block library" className="overflow-y-auto pb-2">
                    <Group title="Project" items={project} empty="None yet. Define one with FUNCTION_BLOCK … END_FUNCTION_BLOCK." />
                    <Group title="Standard" items={standard} empty="Loading…" />
                </div>
            )}
        </section>
    );
};

export default FunctionBlockLibrary;
