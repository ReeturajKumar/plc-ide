import type { DataType } from "../../types/runtime";

/** Monitor display: TRUE/FALSE, integers as-is, REAL always with a decimal point, TIME in ms. */
export function formatValue(dataType: DataType, value: boolean | number): string {
    if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
    if (dataType === "TIME") return `${value} ms`;
    if (dataType !== "REAL") return String(value);
    // Trim float noise (0.1 + 0.2 → 0.3) but keep REALs recognisable (24 → 24.0).
    const rounded = Number(value.toFixed(6));
    return Number.isInteger(rounded) ? rounded.toFixed(1) : String(rounded);
}

export function valueClass(value: boolean | number): string {
    if (typeof value === "boolean") return value ? "text-emerald-400" : "text-[var(--theme-text-muted)]";
    return "text-[#b5cea8]"; // editor number color
}
