/** A source unit inside a project. Phase 1: Structured Text programs only. */
export interface PLCProgram {
    id: string;
    name: string;
    fileName: string;
    language: "ST";
    /** Path relative to the project root, e.g. "programs/main.st". */
    path: string;
}
