import { describe, it, expect } from "vitest";
import { validateProjectName, validateProgramName, parseEntryName, type EntryNameContext } from "./validation";

describe("validateProjectName", () => {
    it("accepts a normal name", () => {
        expect(validateProjectName("MyPLCProject")).toBeNull();
    });
    it("rejects empty / whitespace", () => {
        expect(validateProjectName("")).toBe("Project name cannot be empty.");
        expect(validateProjectName("   ")).toBe("Project name cannot be empty.");
    });
    it("rejects invalid filesystem characters", () => {
        expect(validateProjectName("bad/name")).toMatch(/invalid characters/);
        expect(validateProjectName('a:b*c?')).toMatch(/invalid characters/);
    });
    it("rejects reserved device names", () => {
        expect(validateProjectName("CON")).toMatch(/reserved/);
    });
});

describe("validateProgramName", () => {
    it("accepts a new unique name", () => {
        expect(validateProgramName("motor", ["main"])).toBeNull();
    });
    it("rejects duplicates case-insensitively", () => {
        expect(validateProgramName("Main", ["Main"])).toBe("Program 'Main' already exists.");
    });
    it("rejects empty and invalid names", () => {
        expect(validateProgramName("", [])).toMatch(/empty/);
        expect(validateProgramName("a\\b", [])).toMatch(/invalid characters/);
    });
});

describe("parseEntryName", () => {
    const file: EntryNameContext = { kind: "file", siblings: ["main.st", "docs"], programs: ["main", "belt"], atRoot: false };
    const folder: EntryNameContext = { ...file, kind: "folder" };

    it("makes an extensionless file a .st program, keeps other extensions", () => {
        expect(parseEntryName("motor", file)).toEqual({ name: "motor.st" });
        expect(parseEntryName(" Motor.ST ", file)).toEqual({ name: "Motor.ST" });
        expect(parseEntryName("notes.txt", file)).toEqual({ name: "notes.txt" });
    });

    it("leaves folder names as typed", () => {
        expect(parseEntryName("conveyor", folder)).toEqual({ name: "conveyor" });
        expect(parseEntryName("v1.2", folder)).toEqual({ name: "v1.2" });
    });

    it("refuses empty, invalid and duplicate names", () => {
        expect(parseEntryName("  ", file)).toEqual({ error: "A file or folder name must be provided." });
        expect(parseEntryName("a/b", folder)).toHaveProperty("error", expect.stringContaining("can't contain"));
        expect(parseEntryName("x.", folder)).toHaveProperty("error", expect.stringContaining("end with a dot"));
        expect(parseEntryName("..", folder)).toHaveProperty("error", expect.stringContaining("'..'"));
        expect(parseEntryName("CON", folder)).toHaveProperty("error", expect.stringContaining("reserved by the system"));
        expect(parseEntryName("DOCS", folder)).toEqual({ error: "A file or folder 'DOCS' already exists at this location." });
        expect(parseEntryName("main", file)).toEqual({ error: "Program 'main' already exists." });
    });

    it("applies program rules to .st files only", () => {
        expect(parseEntryName("my program", file)).toHaveProperty("error", expect.stringMatching(/letters, digits and _/));
        expect(parseEntryName("BELT.st", file)).toHaveProperty("error", expect.stringMatching(/^Program 'belt' already exists\.$/i));
        expect(parseEntryName("my notes.txt", file)).toEqual({ name: "my notes.txt" });
    });

    it("protects MyPLC's own files at the project root only", () => {
        const root = { ...file, atRoot: true };
        expect(parseEntryName("project.json", root)).toEqual({ error: "'project.json' is reserved by MyPLC." });
        expect(parseEntryName(".myplc", { ...root, kind: "folder" })).toEqual({ error: "'.myplc' is reserved by MyPLC." });
        expect(parseEntryName("project.json", file)).toEqual({ name: "project.json" });
    });
});

describe("parseEntryName appendSt", () => {
    it("can leave extensionless file names alone (renaming a non-.st file)", () => {
        const ctx: EntryNameContext = { kind: "file", siblings: [], programs: [], atRoot: false, appendSt: false };
        expect(parseEntryName("README", ctx)).toEqual({ name: "README" });
    });
});
