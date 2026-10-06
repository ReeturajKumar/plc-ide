import { describe, it, expect } from "vitest";
import { applyInserts, planFbInsert } from "./fbInsert";

const TON = { name: "TON", inputs: [{ name: "IN", dataType: "BOOL" }, { name: "PT", dataType: "TIME" }], outputs: [] };

const MAIN = ["PROGRAM Main", "VAR", "    Start : BOOL;", "END_VAR", "", "IF Start THEN", "    x := 1;", "END_IF;", "END_PROGRAM"].join("\n");

describe("planFbInsert", () => {
    it("declares in the VAR block and calls at the drop line", () => {
        const plan = planFbInsert(MAIN, TON, 7)!;
        expect(plan.instance).toBe("TON1");
        expect(applyInserts(MAIN, plan.inserts).split("\n")).toEqual([
            "PROGRAM Main", "VAR", "    Start : BOOL;", "    TON1 : TON;", "END_VAR", "", "IF Start THEN",
            "    TON1(IN := FALSE, PT := T#0s);", "    x := 1;", "END_IF;", "END_PROGRAM",
        ]);
        expect(plan.callLine).toBe(8);
    });

    it("picks a free name and keeps calls out of the declarations", () => {
        const src = MAIN.replace("Start : BOOL;", "Start : BOOL;\n    ton1 : TON;");
        const plan = planFbInsert(src, TON, 2)!;
        expect(plan.instance).toBe("TON2");
        const out = applyInserts(src, plan.inserts).split("\n");
        expect(out.indexOf("    TON2 : TON;")).toBe(4);
        expect(out[6]).toBe("TON2(IN := FALSE, PT := T#0s);"); // right after END_VAR
        expect(plan.callLine).toBe(7);
    });

    it("creates a VAR block when there is none (after VAR_INPUT/VAR_OUTPUT in a block)", () => {
        const fb = ["FUNCTION_BLOCK Pump", "VAR_INPUT", "    Run : BOOL;", "END_VAR", "Run := Run;", "END_FUNCTION_BLOCK"].join("\n");
        const plan = planFbInsert(fb, TON, 6)!;
        expect(applyInserts(fb, plan.inserts).split("\n")).toEqual([
            "FUNCTION_BLOCK Pump", "VAR_INPUT", "    Run : BOOL;", "END_VAR", "VAR", "    TON1 : TON;", "END_VAR",
            "Run := Run;", "TON1(IN := FALSE, PT := T#0s);", "END_FUNCTION_BLOCK",
        ]);
    });

    it("refuses drops outside a PROGRAM or FUNCTION_BLOCK", () => {
        expect(planFbInsert("(* notes *)\n", TON, 1)).toBeNull();
        expect(planFbInsert(MAIN + "\n\n", TON, 11)).toBeNull();
    });
});
