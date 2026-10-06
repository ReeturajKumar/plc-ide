import { describe, it, expect } from "vitest";
import {
    createProjectModel,
    programForPath,
    reconcilePrograms,
    movePrograms,
    compileStatus,
    remapPrograms,
    remapPath,
    parentPath,
    childPath,
    filePaths,
    toProjectFile,
    fromProjectFile,
    joinPath,
} from "./project";

describe("createProjectModel", () => {
    it("builds a project with one main program", () => {
        const p = createProjectModel("Demo", "C:\\proj\\Demo");
        expect(p.name).toBe("Demo");
        expect(p.rootPath).toBe("C:\\proj\\Demo");
        expect(p.version).toBe("0.1.0");
        expect(p.programs).toHaveLength(1);
        expect(p.programs[0]).toMatchObject({
            name: "main",
            fileName: "main.st",
            language: "ST",
            path: "programs/main.st",
        });
        expect(p.id).toBeTruthy();
    });
});

describe("project paths", () => {
    it("splits and joins /-separated paths", () => {
        expect(parentPath("programs/conveyor/belt.st")).toBe("programs/conveyor");
        expect(parentPath("main.st")).toBe("");
        expect(childPath("", "programs")).toBe("programs");
        expect(childPath("programs", "a.st")).toBe("programs/a.st");
    });

    it("remaps a file, a folder's contents, and nothing else", () => {
        expect(remapPath("programs/a.st", "programs/a.st", "programs/b.st")).toBe("programs/b.st");
        expect(remapPath("programs/x/a.st", "programs", "src")).toBe("src/x/a.st");
        // "programs2" is not inside "programs".
        expect(remapPath("programs2/a.st", "programs", "src")).toBe("programs2/a.st");
    });

    it("lists every file in a tree", () => {
        const tree = [
            { name: "p", path: "p", isDir: true, children: [
                { name: "q", path: "p/q", isDir: true, children: [{ name: "a.st", path: "p/q/a.st", isDir: false, children: [] }] },
                { name: "b.txt", path: "p/b.txt", isDir: false, children: [] },
            ] },
        ];
        expect(filePaths(tree)).toEqual(["p/q/a.st", "p/b.txt"]);
    });
});

describe("program registry", () => {
    it("derives a program from a .st path", () => {
        expect(programForPath("programs/conveyor/Belt.st", "id1")).toEqual({
            id: "id1", name: "Belt", fileName: "Belt.st", language: "ST", path: "programs/conveyor/Belt.st",
        });
    });

    it("reconciles with the files on disk, keeping ids", () => {
        const main = programForPath("programs/main.st", "m");
        const gone = programForPath("programs/old.st", "o");
        expect(reconcilePrograms([main], ["programs/main.st"])).toBeNull();
        const next = reconcilePrograms([main, gone], ["programs/main.st", "lib/new.st"])!;
        expect(next.map((p) => p.path)).toEqual(["programs/main.st", "lib/new.st"]);
        expect(next[0]).toBe(main);
    });

    it("keeps the registry order (the execution order) and appends new files", () => {
        const [z, a] = [programForPath("Zeta.st", "z"), programForPath("Alpha.st", "a")];
        const next = reconcilePrograms([z, a], ["Alpha.st", "Beta.st", "Zeta.st"])!;
        expect(next.map((p) => p.name)).toEqual(["Zeta", "Alpha", "Beta"]);
    });

    it("moves programs within the execution order", () => {
        const programs = ["A", "B", "C"].map((n) => programForPath(`${n}.st`, n));
        expect(movePrograms(programs, 2, -1)!.map((p) => p.name)).toEqual(["A", "C", "B"]);
        expect(movePrograms(programs, 0, 1)!.map((p) => p.name)).toEqual(["B", "A", "C"]);
        expect(movePrograms(programs, 0, -1)).toBeNull();
        expect(movePrograms(programs, 2, 1)).toBeNull();
    });
});

describe("compileStatus", () => {
    const ok = { source: "PROGRAM a END_PROGRAM", errorCount: 0 };
    it("follows the last compilation and goes stale on edits", () => {
        expect(compileStatus(undefined, "x", false)).toBe("Not Compiled");
        expect(compileStatus(ok, ok.source, false)).toBe("Compiled");
        expect(compileStatus(ok, undefined, false)).toBe("Compiled");
        expect(compileStatus({ ...ok, errorCount: 2 }, ok.source, false)).toBe("Error");
        expect(compileStatus(ok, ok.source + " ", false)).toBe("Modified");
        expect(compileStatus(ok, ok.source, true)).toBe("Compiling");
    });

    it("remaps a folder rename, keeping ids", () => {
        const programs = [programForPath("programs/a.st", "a"), programForPath("other/b.st", "b")];
        const next = remapPrograms(programs, "programs", "src");
        expect(next.map((p) => [p.id, p.path])).toEqual([["a", "src/a.st"], ["b", "other/b.st"]]);
    });
});

describe("project.json round-trip", () => {
    it("drops rootPath on write and restores it on read", () => {
        const p = createProjectModel("Demo", "C:\\proj\\Demo");
        const json = toProjectFile(p);
        expect(json).not.toContain("rootPath");
        const back = fromProjectFile(json, "C:\\proj\\Demo");
        expect(back.rootPath).toBe("C:\\proj\\Demo");
        expect(back.programs[0].fileName).toBe("main.st");
    });
});

describe("joinPath", () => {
    it("uses backslash for Windows roots", () => {
        expect(joinPath("C:\\proj", "Demo")).toBe("C:\\proj\\Demo");
    });
    it("uses forward slash for POSIX roots", () => {
        expect(joinPath("/home/u", "Demo")).toBe("/home/u/Demo");
    });
});
