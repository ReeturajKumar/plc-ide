import type { FbSummary } from "../types/runtime";

/** Drag-and-drop data type for a function block from the library (JSON `FbSummary`). */
export const FB_DRAG_TYPE = "application/x-myplc-fb";

/** Placeholder input values by type, for the inserted call. */
const DEFAULTS: Record<string, string> = { BOOL: "FALSE", INT: "0", DINT: "0", REAL: "0.0", TIME: "T#0s" };

/** One line of text to insert before `at` (0-based line index; `at` = line count appends). */
export interface LineInsert {
    at: number;
    text: string;
}

export interface FbInsertPlan {
    instance: string;
    inserts: LineInsert[];
    /** 1-based line of the call after the inserts. */
    callLine: number;
}

const HEADER = /^\s*(PROGRAM|FUNCTION_BLOCK)\b/i;
const FOOTER = /^\s*END_(PROGRAM|FUNCTION_BLOCK)\b/i;
const VAR_START = /^\s*VAR\s*(\(\*.*\*\)\s*)?$/i; // plain VAR, not VAR_INPUT / VAR_OUTPUT
const VAR_END = /^\s*END_VAR\b/i;

/**
 * Where to put a new instance of `fb` when it is dropped before `line` (1-based) of
 * `source`: a declaration in the VAR block of the PROGRAM / FUNCTION_BLOCK around that line
 * (a VAR block is created if there is none) and a call with every input, at the drop line
 * or, if that is among the declarations, right after them. Null if `line` isn't inside a
 * PROGRAM or FUNCTION_BLOCK.
 */
export function planFbInsert(source: string, fb: Pick<FbSummary, "name" | "inputs">, line: number): FbInsertPlan | null {
    const lines = source.split(/\r?\n/);
    const drop = Math.min(Math.max(line, 1), lines.length) - 1;

    let header = -1;
    for (let i = drop; i >= 0; i--) {
        if (HEADER.test(lines[i])) {
            header = i;
            break;
        }
    }
    if (header < 0) return null;
    let footer = lines.length;
    for (let i = header + 1; i < lines.length; i++) {
        if (FOOTER.test(lines[i])) {
            footer = i;
            break;
        }
    }
    if (drop > footer) return null; // after this POU's END_…

    // Unique instance name: TON1, TON2, … (identifiers are case-insensitive).
    const taken = new Set((source.match(/[A-Za-z_]\w*/g) ?? []).map((w) => w.toLowerCase()));
    let n = 1;
    while (taken.has(`${fb.name}${n}`.toLowerCase())) n++;
    const instance = `${fb.name}${n}`;

    let varStart = -1;
    let varEnd = -1;
    let lastEndVar = header;
    for (let i = header + 1; i < footer; i++) {
        if (varStart < 0 && VAR_START.test(lines[i])) varStart = i;
        if (VAR_END.test(lines[i])) {
            lastEndVar = i;
            if (varStart >= 0 && varEnd < 0) varEnd = i;
        }
    }

    const declaration = `    ${instance} : ${fb.name};`;
    const declInserts: LineInsert[] =
        varEnd >= 0
            ? [{ at: varEnd, text: declaration }]
            : [{ at: lastEndVar + 1, text: "VAR" }, { at: lastEndVar + 1, text: declaration }, { at: lastEndVar + 1, text: "END_VAR" }];

    // The call goes where it was dropped, but never into the declarations.
    const callAt = Math.min(Math.max(drop, lastEndVar + 1), footer);
    const near = lines[callAt]?.trim() ? lines[callAt] : (lines[callAt - 1] ?? "");
    const indent = HEADER.test(near) || VAR_END.test(near) || FOOTER.test(near) ? "" : (near.match(/^\s*/)?.[0] ?? "");
    const args = fb.inputs.map((input) => `${input.name} := ${DEFAULTS[input.dataType.toUpperCase()] ?? "0"}`).join(", ");
    const call = { at: callAt, text: `${indent}${instance}(${args});` };

    const before = declInserts.filter((d) => d.at <= callAt).length;
    return { instance, inserts: [...declInserts, call], callLine: callAt + before + 1 };
}

/** `source` with the plan's inserts applied (what the editor will show). */
export function applyInserts(source: string, inserts: LineInsert[]): string {
    const lines = source.split(/\r?\n/);
    const out: string[] = [];
    for (let i = 0; i <= lines.length; i++) {
        out.push(...inserts.filter((x) => x.at === i).map((x) => x.text));
        if (i < lines.length) out.push(lines[i]);
    }
    return out.join("\n");
}
