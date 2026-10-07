// Names that would collide with reserved Windows device names.
const RESERVED = /^(con|prn|aux|nul|com[1-9]|lpt[1-9])$/i;
// Characters not allowed in Windows/most filesystem paths.
const INVALID_CHARS = /[<>:"/\\|?*\x00-\x1f]/;

/** Returns an error message, or null if valid. */
export function validateProjectName(name: string): string | null {
    const trimmed = name.trim();
    if (!trimmed) return "Project name cannot be empty.";
    if (INVALID_CHARS.test(trimmed)) return "Project name contains invalid characters.";
    if (RESERVED.test(trimmed)) return "That project name is reserved by the system.";
    return null;
}

/**
 * Validates a program (source unit) name — without the .st extension.
 * `existing` is the list of current program names (case-insensitive) to block duplicates.
 */
export function validateProgramName(name: string, existing: string[]): string | null {
    const trimmed = name.trim();
    if (!trimmed) return "Program name cannot be empty.";
    if (INVALID_CHARS.test(trimmed)) return "Program name contains invalid characters.";
    // The name is also written into the file as `PROGRAM <name>`, so it must be an ST identifier.
    if (!ST_IDENTIFIER.test(trimmed)) {
        return "Use letters, digits and _ only, starting with a letter or _ (it becomes the ST PROGRAM name).";
    }
    if (RESERVED.test(trimmed)) return "That program name is reserved by the system.";
    const taken = existing.find((n) => n.toLowerCase() === trimmed.toLowerCase());
    if (taken) return `Program '${taken}' already exists.`;
    return null;
}

const ST_IDENTIFIER = /^[A-Za-z_][A-Za-z0-9_]*$/;
const ST_EXTENSION = /\.st$/i;

/** Files MyPLC manages itself; they're hidden from the Explorer and can't be created there. */
export const RESERVED_ROOT_NAMES = ["project.json", ".myplc"];

export interface EntryNameContext {
    kind: "file" | "folder";
    /** Names already in the target folder (the entry being renamed excluded). */
    siblings: string[];
    /** Program names in the whole project (the program being renamed excluded). */
    programs: string[];
    /** Whether the entry lives at the project root. */
    atRoot: boolean;
    /** Whether a file name typed without an extension gets `.st` (default true). */
    appendSt?: boolean;
}

/**
 * Interpret a name typed in the Explorer's inline box. A file typed without an extension
 * becomes a `.st` program (this is a PLC IDE). A `.st` name must be an ST identifier and
 * unique across the project, since it becomes the `PROGRAM` name.
 */
export function parseEntryName(input: string, ctx: EntryNameContext): { name: string } | { error: string } {
    const typed = input.trim();
    if (!typed) return { error: "A file or folder name must be provided." };
    if (INVALID_CHARS.test(typed)) return { error: 'Names can\'t contain \\ / : * ? " < > |' };
    if (typed === "." || typed === ".." || /[. ]$/.test(typed)) {
        return { error: "Names can't be '.' or '..' or end with a dot or space." };
    }
    const appendSt = ctx.kind === "file" && ctx.appendSt !== false && !typed.includes(".");
    const name = appendSt ? `${typed}.st` : typed;
    if (RESERVED.test(name.replace(/\..*$/, ""))) return { error: `'${name}' is reserved by the system.` };
    if (ctx.atRoot && RESERVED_ROOT_NAMES.includes(name.toLowerCase())) {
        return { error: `'${name}' is reserved by MyPLC.` };
    }
    // Program rules first: a .st name taken anywhere in the project is a duplicate program.
    if (ctx.kind === "file" && ST_EXTENSION.test(name)) {
        const error = validateProgramName(name.replace(ST_EXTENSION, ""), ctx.programs);
        if (error) return { error };
    }
    if (ctx.siblings.some((s) => s.toLowerCase() === name.toLowerCase())) {
        return { error: `A file or folder '${name}' already exists at this location.` };
    }
    return { name };
}
