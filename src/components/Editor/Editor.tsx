import { useEffect } from "react";
import MonacoEditor, { useMonaco, type OnMount, type BeforeMount } from "@monaco-editor/react";
import { useEditorStore, isDirty } from "../../store/editorStore";
import { closeTab, copyToClipboard, projectPath } from "../../store/projectActions";
import IconButton, { MoreButton } from "../common/IconButton";
import { compileAll, compileCurrent, runOrResume } from "../../store/runActions";
import type { MenuItem } from "../common/Menu";
import { locationPath, toggleBreakpoint } from "../../store/runActions";
import { useProjectStore } from "../../store/projectStore";
import { useSimulatorStore } from "../../store/simulatorStore";
import type { editor as MonacoEditorNs } from "monaco-editor";
import { getEditor, insertFunctionBlock, setEditor } from "../../utils/editorRef";
import { FB_DRAG_TYPE } from "../../utils/fbInsert";
import { useUIStore } from "../../store/uiStore";
import type { FbSummary } from "../../types/runtime";
import { useConsoleStore } from "../../store/consoleStore";
import { FileIcon } from "../common/FileIcons";
import { isBuiltinPath, languageFor } from "../../utils/project";

// Register the ST language + dark theme BEFORE the editor is created, so the
// `theme="st-dark"` prop applies immediately (otherwise Monaco flashes the
// default light theme — a white background — until onMount runs).
const setupStructuredText: BeforeMount = (monaco) => {
    if (monaco.languages.getLanguages().some((l: { id: string }) => l.id === "st")) return;

    monaco.languages.register({ id: "st" });
    monaco.languages.setMonarchTokensProvider("st", {
        ignoreCase: true,
        keywords: [
            "PROGRAM", "END_PROGRAM", "FUNCTION", "END_FUNCTION", "FUNCTION_BLOCK",
            "END_FUNCTION_BLOCK", "VAR", "VAR_INPUT", "VAR_OUTPUT", "VAR_IN_OUT",
            "VAR_GLOBAL", "END_VAR", "IF", "THEN", "ELSIF", "ELSE", "END_IF",
            "CASE", "OF", "END_CASE", "FOR", "TO", "BY", "DO", "END_FOR",
            "WHILE", "END_WHILE", "REPEAT", "UNTIL", "END_REPEAT", "RETURN",
            "EXIT", "AND", "OR", "XOR", "NOT", "MOD",
        ],
        typeKeywords: [
            "BOOL", "BYTE", "WORD", "DWORD", "LWORD", "SINT", "INT", "DINT",
            "LINT", "USINT", "UINT", "UDINT", "ULINT", "REAL", "LREAL",
            "TIME", "DATE", "TOD", "DT", "STRING", "WSTRING", "ARRAY",
            "TON", "TOF", "TP", "CTU", "CTD", "R_TRIG", "F_TRIG",
        ],
        builtins: [
            "ABS", "SQRT", "MIN", "MAX", "TRUNC", "ROUND", "SIN", "COS",
        ],
        tokenizer: {
            root: [
                [/\b(TRUE|FALSE)\b/i, "constant"],
                // Before identifiers, or `T` of `T#2s` would be read as a name.
                [/\b(T|TIME)#[\w.]+/i, "number"],
                [/[A-Za-z_]\w*/, {
                    cases: { "@keywords": "keyword", "@typeKeywords": "type", "@builtins": "predefined", "@default": "identifier" },
                }],
                [/\/\/.*$/, "comment"],
                [/\(\*/, "comment", "@comment"],
                [/\d+\.\d+/, "number.float"],
                [/\d+/, "number"],
                [/:=|[=<>]+|[-+*/]/, "operator"],
                [/[;:.,()]/, "delimiter"],
            ],
            comment: [
                [/\*\)/, "comment", "@pop"],
                [/./, "comment"],
            ],
        },
    });

    monaco.languages.setLanguageConfiguration("st", {
        comments: { lineComment: "//", blockComment: ["(*", "*)"] },
        brackets: [["(", ")"], ["[", "]"]],
        autoClosingPairs: [{ open: "(", close: ")" }, { open: "'", close: "'" }],
    });

    // VS Code Dark+ token colors for an authentic editor look.
    monaco.editor.defineTheme("st-dark", {
        base: "vs-dark",
        inherit: true,
        rules: [
            { token: "keyword", foreground: "569cd6" },
            { token: "type", foreground: "4ec9b0" },
            { token: "predefined", foreground: "dcdcaa" }, // standard functions, like VS Code function names
            { token: "constant", foreground: "569cd6" },
            { token: "operator", foreground: "d4d4d4" },
            { token: "comment", foreground: "6a9955", fontStyle: "italic" },
            { token: "number", foreground: "b5cea8" },
            { token: "string", foreground: "ce9178" },
            { token: "identifier", foreground: "d4d4d4" },
            { token: "delimiter", foreground: "d4d4d4" },
        ],
        colors: {
            "editor.background": "#1e1e1e",
            "editor.foreground": "#d4d4d4",
            "editorLineNumber.foreground": "#858585",
            "editorLineNumber.activeForeground": "#c6c6c6",
            "editor.lineHighlightBackground": "#2a2a2a",
            "editor.selectionBackground": "#264f78",
            "editorCursor.foreground": "#aeafad",
            "editorIndentGuide.background1": "#404040",
        },
    });
    monaco.editor.setTheme("st-dark");
};

// The app font (see --font-app in index.css), read once so Monaco uses the same stack.
const APP_FONT =
    getComputedStyle(document.documentElement).getPropertyValue("--font-app").trim() || "Consolas, 'Courier New', monospace";

// Breakpoint and current-line decorations of the editor (created on mount).
let debugDecorations: MonacoEditorNs.IEditorDecorationsCollection | null = null;

// Wire the editor instance for menu actions + status-bar cursor tracking.
const onEditorMount: OnMount = (editor, monaco) => {
    setEditor(editor);
    debugDecorations = editor.createDecorationsCollection();
    // Clicking the gutter left of the line numbers toggles a breakpoint, as in VS Code.
    editor.onMouseDown((e) => {
        const path = useEditorStore.getState().activePath;
        if (e.target.type === monaco.editor.MouseTargetType.GUTTER_GLYPH_MARGIN && e.target.position && path) {
            void toggleBreakpoint(path, e.target.position.lineNumber);
        }
    });
    // Monaco measures glyph widths once; re-measure when the font finishes loading.
    void document.fonts.ready.then(() => monaco.editor.remeasureFonts());
    editor.onDidChangeCursorPosition((e) =>
        useEditorStore.getState().setCursor(e.position.lineNumber, e.position.column)
    );
    // Switching tabs swaps the model without a cursor event; report the new tab's position.
    editor.onDidChangeModel(() => {
        const position = editor.getPosition();
        useEditorStore.getState().setCursor(position?.lineNumber ?? 1, position?.column ?? 1);
    });
};

const Editor = () => {
    const openFiles = useEditorStore((s) => s.openFiles);
    const activePath = useEditorStore((s) => s.activePath);
    const setActive = useEditorStore((s) => s.setActive);
    const updateContent = useEditorStore((s) => s.updateContent);
    const problems = useConsoleStore((s) => s.problems);
    const monaco = useMonaco();
    const breakpoints = useProjectStore((s) => (activePath ? s.project?.breakpoints?.[activePath] : undefined));
    const breakable = useProjectStore((s) => (activePath ? s.breakableLines[activePath] : undefined));
    const status = useSimulatorStore((s) => s.runtime.status);
    const location = useSimulatorStore((s) => s.runtime.location);

    const active = openFiles.find((f) => f.path === activePath) ?? null;

    // Show this file's problems as squiggles (hover shows the message) on the exact token.
    useEffect(() => {
        const model = getEditor()?.getModel();
        if (!monaco || !model || !activePath) return;
        const markers = problems
            .filter((p) => p.path === activePath && p.line > 0 && p.line <= model.getLineCount())
            .map((p) => {
                const word = model.getWordAtPosition({ lineNumber: p.line, column: p.column });
                const endColumn = word && word.startColumn === p.column ? word.endColumn : p.column + 1;
                return {
                    severity: monaco.MarkerSeverity.Error,
                    message: p.message,
                    source: p.kind,
                    startLineNumber: p.line,
                    startColumn: p.column,
                    endLineNumber: p.line,
                    endColumn,
                };
            });
        monaco.editor.setModelMarkers(model, "myplc", markers);
    }, [monaco, problems, activePath]);

    // Breakpoints (red dots; hollow when the line no longer holds a statement) and, while
    // the debugger is stopped in this file, the current line with an arrow.
    const current = location && activePath && locationPath(location.file) === activePath ? location.line : null;
    useEffect(() => {
        const model = getEditor()?.getModel();
        if (!monaco || !model || !debugDecorations) return;
        const lineCount = model.getLineCount();
        const whole = (line: number) => new monaco.Range(line, 1, line, 1);
        const decorations: MonacoEditorNs.IModelDeltaDecoration[] = (breakpoints ?? [])
            .filter((line) => line <= lineCount)
            .map((line) => {
                const verified = !breakable || breakable.includes(line);
                return {
                    range: whole(line),
                    options: {
                        glyphMarginClassName: verified ? "debug-breakpoint" : "debug-breakpoint-unverified",
                        glyphMarginHoverMessage: { value: verified ? "Breakpoint (click to remove)" : "Breakpoint on a line without a statement: it won't stop" },
                        stickiness: monaco.editor.TrackedRangeStickiness.NeverGrowsWhenTypingAtEdges,
                    },
                };
            });
        if (current && current <= lineCount) {
            decorations.push({
                range: whole(current),
                options: { isWholeLine: true, className: "debug-current-line", glyphMarginClassName: "debug-current-arrow", zIndex: 10 },
            });
        }
        debugDecorations.set(decorations);
    }, [monaco, activePath, breakpoints, breakable, current]);

    // Editing is off while the PLC runs or is paused: the running code must match the source.
    const builtin = active ? isBuiltinPath(active.path) : false;
    const readOnly = builtin || status !== "STOPPED";
    const wordWrap = useUIStore((s) => s.wordWrap);
    const minimap = useUIStore((s) => s.minimap);
    const isSt = active ? languageFor(active.path).id === "st" : false;

    const closeAll = async (except?: string) => {
        for (const f of useEditorStore.getState().openFiles) if (f.path !== except) await closeTab(f.path);
    };
    const moreItems = (): MenuItem[] => {
        const ui = useUIStore.getState();
        const sep: MenuItem = { separator: true, label: "" };
        return [
            { label: "Compile All", shortcut: "F7", onClick: () => void compileAll() },
            sep,
            { label: `${ui.wordWrap ? "✓ " : ""}Word Wrap`, onClick: ui.toggleWordWrap },
            { label: `${ui.minimap ? "✓ " : ""}Minimap`, onClick: ui.toggleMinimap },
            sep,
            {
                label: "Reveal in Explorer",
                disabled: !active,
                onClick: () => {
                    if (!active) return;
                    ui.setSidebarVisible(true);
                    ui.setExplorerSelection(active.path);
                },
            },
            { label: "Copy Path", disabled: !active, onClick: () => active && void copyToClipboard(projectPath(active.path)) },
            sep,
            { label: "Close Others", disabled: openFiles.length < 2, onClick: () => active && void closeAll(active.path) },
            { label: "Close All", disabled: openFiles.length === 0, onClick: () => void closeAll() },
        ];
    };

    return (
        <div className="flex-1 flex flex-col overflow-hidden theme-panel">
            {/* Tab bar, with the editor actions on the right as in VS Code */}
            <div className="theme-panel-header border-b flex items-center">
            <div className="flex-1 min-w-0 flex items-center px-2 pt-1 gap-1 overflow-x-auto">
                {openFiles.map((f) => {
                    const dirty = isDirty(f);
                    const activeTab = f.path === activePath;
                    return (
                        <div
                            key={f.path}
                            onClick={() => setActive(f.path)}
                            className={`flex items-center gap-2 px-4 py-1.5 rounded-t-lg text-xs font-medium cursor-pointer border-t-2 ${
                                activeTab
                                    ? "theme-panel border-[var(--theme-border-focus)] text-[var(--theme-text-primary)]"
                                    : "text-[var(--theme-text-muted)] hover:text-[var(--theme-text-primary)] border-transparent"
                            }`}
                        >
                            <FileIcon fileName={f.fileName} className="w-3.5 h-3.5" />
                            <span>{f.fileName}{dirty ? " *" : ""}</span>
                            <span
                                onClick={(e) => { e.stopPropagation(); void closeTab(f.path); }}
                                className="text-[var(--theme-text-muted)] hover:text-[var(--theme-text-primary)] ml-1"
                            >
                                ×
                            </span>
                        </div>
                    );
                })}
            </div>
            <div className="shrink-0 flex items-center gap-0.5 px-2">
                <IconButton
                    title={status === "PAUSED" ? "Resume (F5)" : "Run All Programs (F5)"}
                    onClick={runOrResume}
                    disabled={status === "RUNNING" || openFiles.length === 0}
                >
                    <polygon points="7 4 19 12 7 20 7 4" />
                </IconButton>
                <IconButton title="Compile This Program (Ctrl+F7)" onClick={() => void compileCurrent()} disabled={!isSt}>
                    <polyline points="20 6 9 17 4 12" />
                </IconButton>
                <IconButton
                    title="Toggle Breakpoint on the Cursor Line (F9)"
                    onClick={() => {
                        const { activePath: path, cursor } = useEditorStore.getState();
                        if (path) void toggleBreakpoint(path, cursor.line);
                    }}
                    disabled={!isSt}
                >
                    <circle cx="12" cy="12" r="5" fill="currentColor" />
                </IconButton>
                <MoreButton title="More Actions…" items={moreItems} />
            </div>
            </div>

            {/* Editor canvas */}
            <div
                className="flex-1 theme-editor-canvas overflow-hidden"
                // Function blocks dragged from the library: declare + call where dropped.
                onDragOver={(e) => {
                    if (!e.dataTransfer.types.includes(FB_DRAG_TYPE)) return;
                    e.preventDefault();
                    e.dataTransfer.dropEffect = "copy";
                }}
                onDrop={(e) => {
                    const data = e.dataTransfer.getData(FB_DRAG_TYPE);
                    if (!data) return;
                    e.preventDefault();
                    const line = getEditor()?.getTargetAtClientPoint(e.clientX, e.clientY)?.position?.lineNumber;
                    const problem = insertFunctionBlock(JSON.parse(data) as FbSummary, line);
                    if (problem) useUIStore.getState().notify("info", problem);
                }}
            >
                {active ? (
                    <MonacoEditor
                        path={active.path}
                        height="100%"
                        language={languageFor(active.path).id}
                        theme="st-dark"
                        value={active.content}
                        onChange={(v) => updateContent(active.path, v ?? "")}
                        beforeMount={setupStructuredText}
                        onMount={onEditorMount}
                        loading={<div className="h-full w-full theme-editor-canvas" />}
                        options={{
                            fontSize: 13,
                            lineHeight: 20,
                            fontFamily: APP_FONT,
                            fontLigatures: true,
                            dropIntoEditor: { enabled: false }, // library drops are handled above
                            minimap: { enabled: minimap, renderCharacters: false },
                            wordWrap: wordWrap ? "on" : "off",
                            scrollBeyondLastLine: false,
                            automaticLayout: true,
                            tabSize: 4,
                            lineNumbersMinChars: 4,
                            glyphMargin: true, // breakpoints
                            readOnly,
                            readOnlyMessage: {
                                value: builtin
                                    ? "This is the read-only definition of a standard function block."
                                    : "Stop the PLC (Shift+F5) to edit the program.",
                            },
                            renderLineHighlight: "line",
                            smoothScrolling: true,
                            cursorBlinking: "smooth",
                            roundedSelection: true,
                            guides: { indentation: true },
                            scrollbar: { verticalScrollbarSize: 10, horizontalScrollbarSize: 10 },
                            padding: { top: 12, bottom: 12 },
                        }}
                    />
                ) : (
                    <div className="h-full flex items-center justify-center text-xs text-[var(--theme-text-muted)]">
                        Select a program from the Explorer to start editing.
                    </div>
                )}
            </div>
        </div>
    );
};

export default Editor;
