/** An open editor tab. Tabs are identified by `path`, so renaming a file or any of its
 * folders just remaps the path and keeps the tab (and unsaved edits). */
export interface OpenFile {
    /** Path relative to the project root, with `/` separators. */
    path: string;
    fileName: string;
    /** Content last loaded from / saved to disk. */
    savedContent: string;
    /** Current buffer content in the editor. */
    content: string;
}
