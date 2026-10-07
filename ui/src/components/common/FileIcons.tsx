/**
 * File and folder icons in the VS Code icon-theme style: files get a badge chosen by
 * extension, folders a color chosen by name and an open/closed shape.
 * To support a new file type or folder, add one entry to the tables below.
 */

/** Badge per file extension (lowercase, without the dot). */
const FILE_BADGES: Record<string, { text: string; background: string; foreground: string }> = {
    st: { text: "ST", background: "#f59e0b", foreground: "#1f1f1f" }, // Structured Text
};

/** Folder color per folder name; others use the default. */
const FOLDER_COLORS: Record<string, string> = {
    programs: "#42a5f5",
    functions: "#ab47bc",
    "function-blocks": "#26a69a",
};
const DEFAULT_FOLDER_COLOR = "#90a4ae";

function extension(fileName: string): string {
    const dot = fileName.lastIndexOf(".");
    return dot > 0 ? fileName.slice(dot + 1).toLowerCase() : "";
}

interface FileIconProps {
    fileName: string;
    className?: string;
}

export const FileIcon = ({ fileName, className = "w-4 h-4" }: FileIconProps) => {
    const badge = FILE_BADGES[extension(fileName)];
    if (!badge) {
        // Unknown type: a plain document.
        return (
            <svg className={`${className} shrink-0`} viewBox="0 0 16 16" aria-hidden="true">
                <path d="M4 1.5h5L12.5 5v9a.5.5 0 0 1-.5.5H4a.5.5 0 0 1-.5-.5V2a.5.5 0 0 1 .5-.5z" fill="#90a4ae" />
                <path d="M9 1.5V5h3.5" fill="#cfd8dc" />
            </svg>
        );
    }
    return (
        <svg className={`${className} shrink-0`} viewBox="0 0 16 16" aria-hidden="true">
            <rect x="1" y="2.5" width="14" height="11" rx="2" fill={badge.background} />
            <text
                x="8"
                y="10.9"
                textAnchor="middle"
                fontSize="7.5"
                fontWeight="700"
                style={{ fontFamily: "var(--font-app)" }}
                fill={badge.foreground}
            >
                {badge.text}
            </text>
        </svg>
    );
};

interface FolderIconProps {
    /** Folder name on disk, e.g. "programs". */
    name: string;
    open?: boolean;
    className?: string;
}

export const FolderIcon = ({ name, open = false, className = "w-4 h-4" }: FolderIconProps) => {
    const color = FOLDER_COLORS[name.toLowerCase()] ?? DEFAULT_FOLDER_COLOR;
    return (
        <svg className={`${className} shrink-0`} viewBox="0 0 24 24" aria-hidden="true">
            {open ? (
                <>
                    <path d="M4 4h5.2l2 2H20a2 2 0 0 1 2 2v1.5H7.2a2 2 0 0 0-1.9 1.4L2.6 19.3 2 18V6a2 2 0 0 1 2-2z" fill={color} opacity="0.7" />
                    <path d="M5.4 11.4A1.5 1.5 0 0 1 6.8 10.4H22.6a.9.9 0 0 1 .86 1.16l-2.3 7.5A1.8 1.8 0 0 1 19.4 20.4H3.2a.9.9 0 0 1-.86-1.16z" fill={color} />
                </>
            ) : (
                <path d="M10 4H4a2 2 0 0 0-2 2v12a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-8l-2-2z" fill={color} />
            )}
        </svg>
    );
};
