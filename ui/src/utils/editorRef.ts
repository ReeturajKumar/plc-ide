import type { editor } from "monaco-editor";
import type { FbSummary } from "../types/runtime";
import { planFbInsert } from "./fbInsert";

// Module singleton for the active Monaco instance so menus (Undo/Redo/Find)
// can drive it without prop-drilling a ref through the tree.
let current: editor.IStandaloneCodeEditor | null = null;

export const setEditor = (e: editor.IStandaloneCodeEditor | null) => {
    current = e;
};

/** Run a built-in Monaco action (e.g. "undo", "actions.find") on the active editor. */
export const runEditorAction = (action: string) => {
    if (!current) return;
    current.focus();
    current.trigger("menu", action, null);
};

export const getEditor = () => current;

/**
 * Insert an instance of `fb` into the open program: its declaration in the VAR block and
 * a call before `line` (default: the cursor's line), as one undoable edit. Returns why it
 * couldn't, or null.
 */
export const insertFunctionBlock = (fb: FbSummary, line?: number): string | null => {
    const model = current?.getModel();
    if (!current || !model) return "Open a program to insert a function block.";
    if (current.getRawOptions().readOnly) return "This editor is read-only (a standard block's definition, or the PLC is running).";
    const plan = planFbInsert(model.getValue(), fb, line ?? current.getPosition()?.lineNumber ?? 1);
    if (!plan) return "Put function blocks inside a PROGRAM or FUNCTION_BLOCK.";
    const count = model.getLineCount();
    const byLine = new Map<number, string[]>();
    for (const { at, text } of plan.inserts) byLine.set(at, [...(byLine.get(at) ?? []), text]);
    const edits = [...byLine].map(([at, texts]) => {
        const text = texts.join("\n");
        const end = model.getLineMaxColumn(count);
        return at < count
            ? { range: { startLineNumber: at + 1, startColumn: 1, endLineNumber: at + 1, endColumn: 1 }, text: `${text}\n` }
            : { range: { startLineNumber: count, startColumn: end, endLineNumber: count, endColumn: end }, text: `\n${text}` };
    });
    current.pushUndoStop();
    current.executeEdits("fb-library", edits);
    current.pushUndoStop();
    current.setPosition({ lineNumber: plan.callLine, column: model.getLineFirstNonWhitespaceColumn(plan.callLine) || 1 });
    current.revealLineInCenter(plan.callLine);
    current.focus();
    return null;
};

/** Put the cursor at a 1-based line/column in the active editor and scroll it into view. */
export const revealPosition = (line: number, column: number) => {
    if (!current) return;
    current.setPosition({ lineNumber: line, column });
    current.revealPositionInCenter({ lineNumber: line, column });
    current.focus();
};
