import type { DataType } from "../../types/runtime";

/** Monitor display: TRUE/FALSE, integers as-is, bit strings in hex, REAL/LREAL always with a
 *  decimal point, TIME in ms, dates and text as IEC literals. */
export function formatValue(dataType: DataType, value: boolean | number | string): string {
    if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
    if (typeof value === "string") return dataType === "WSTRING" ? `"${value}"` : `'${value}'`;
    if (dataType === "CHAR") return `'${String.fromCharCode(value)}'`;
    if (dataType === "WCHAR") return `"${String.fromCharCode(value)}"`;
    if (dataType === "TIME") return `${value} ms`;
    // Dates are milliseconds since 1970 (TOD: since midnight), shown as IEC literals.
    if (dataType === "DATE" || dataType === "TOD" || dataType === "DT") {
        const [day, time] = new Date(value).toISOString().replace(".000Z", "").replace("Z", "").split("T");
        return dataType === "DATE" ? `D#${day}` : dataType === "TOD" ? `TOD#${time}` : `DT#${day}-${time}`;
    }
    if (["BYTE", "WORD", "DWORD", "LWORD"].includes(dataType)) return `16#${value.toString(16).toUpperCase()}`;
    if (dataType !== "REAL" && dataType !== "LREAL") return String(value);
    // Trim float noise (0.1 + 0.2 → 0.3) but keep REALs recognisable (24 → 24.0).
    const rounded = Number(value.toFixed(6));
    return Number.isInteger(rounded) ? rounded.toFixed(1) : String(rounded);
}

export function valueClass(value: boolean | number | string): string {
    if (typeof value === "boolean") return value ? "text-emerald-400" : "text-[var(--theme-text-muted)]";
    if (typeof value === "string") return "text-[#ce9178]"; // editor string color
    return "text-[#b5cea8]"; // editor number color
}

/** A value the user may type into an input variable, or why the text isn't one. */
export type ParsedInput = { ok: true; value: boolean | number | string } | { ok: false; error: string };

const INTEGER_RANGES: Record<string, [number, number]> = {
    SINT: [-128, 127], INT: [-32768, 32767], DINT: [-2147483648, 2147483647],
    USINT: [0, 255], UINT: [0, 65535], UDINT: [0, 4294967295],
    BYTE: [0, 255], WORD: [0, 65535], DWORD: [0, 4294967295],
};
const INTEGERS = new Set([...Object.keys(INTEGER_RANGES), "LINT", "ULINT", "LWORD"]);

/** Types a user can type a value for (BOOL is toggled instead; dates, arrays… aren't typed). */
export function isTypable(dataType: DataType): boolean {
    return INTEGERS.has(dataType) || ["REAL", "LREAL", "STRING", "WSTRING"].includes(dataType);
}

/** The value as an input field shows it (no IEC decoration). */
export const inputText = (value: boolean | number | string) => String(value);

/** Text typed for a variable of `dataType` → its value: whole numbers (also 16#FF, 2#1010) for integers. */
export function parseInput(dataType: DataType, text: string): ParsedInput {
    const t = text.trim();
    if (dataType === "STRING" || dataType === "WSTRING") return { ok: true, value: text };
    if (dataType === "REAL" || dataType === "LREAL") {
        const x = Number(t);
        return t !== "" && Number.isFinite(x) ? { ok: true, value: x } : { ok: false, error: `${dataType} needs a number, e.g. 12.5` };
    }
    if (INTEGERS.has(dataType)) {
        const based = t.match(/^(2|8|16)#([0-9a-f_]+)$/i);
        const n = based ? parseInt(based[2].replace(/_/g, ""), Number(based[1])) : Number(t);
        if (t === "" || !Number.isInteger(n)) return { ok: false, error: `${dataType} needs a whole number, e.g. 42` };
        const range = INTEGER_RANGES[dataType];
        if (range && (n < range[0] || n > range[1])) return { ok: false, error: `${dataType} goes from ${range[0]} to ${range[1]}` };
        return { ok: true, value: n };
    }
    return { ok: false, error: `${dataType} values can't be typed here` };
}
