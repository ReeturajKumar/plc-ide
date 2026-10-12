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
