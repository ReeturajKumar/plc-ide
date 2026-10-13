import { compilerApi } from "../services/compilerApi";
import type { FbSummary } from "../types/runtime";

/** What each standard block does, and what each member means. Names and types come from the compiler. */
const DOCS: Record<string, { text: string; members: Record<string, string> }> = {
    TON: {
        text: "On-delay timer: Q turns TRUE once IN has been TRUE for the time PT. Q and ET reset when IN goes FALSE.",
        members: { IN: "Starts timing while TRUE.", PT: "Delay time.", Q: "TRUE once IN has been TRUE for PT.", ET: "Elapsed time, 0 … PT." },
    },
    TOF: {
        text: "Off-delay timer: Q follows IN going TRUE, and stays TRUE for the time PT after IN goes FALSE.",
        members: { IN: "Q is TRUE while IN is TRUE.", PT: "Off-delay time.", Q: "TRUE while IN, and for PT after it falls.", ET: "Time since IN fell, 0 … PT." },
    },
    TP: {
        text: "Pulse timer: a rising edge of IN starts a pulse on Q that lasts exactly PT, whatever IN does meanwhile.",
        members: { IN: "A rising edge starts the pulse.", PT: "Pulse length.", Q: "TRUE during the pulse.", ET: "Time into the pulse, 0 … PT." },
    },
    CTU: {
        text: "Up counter: each rising edge of CU adds one to CV. R (or RESET) sets CV to 0. Q is TRUE when CV ≥ PV.",
        members: {
            CU: "A rising edge counts up.",
            R: "TRUE: CV := 0 (IEC name).",
            RESET: "Same as R.",
            PV: "Preset value.",
            Q: "TRUE when CV ≥ PV.",
            CV: "Counter value.",
        },
    },
    CTD: {
        text: "Down counter: each rising edge of CD subtracts one from CV. LD (or LOAD) sets CV to PV. Q is TRUE when CV ≤ 0.",
        members: {
            CD: "A rising edge counts down.",
            LD: "TRUE: CV := PV (IEC name).",
            LOAD: "Same as LD.",
            PV: "Preset value.",
            Q: "TRUE when CV ≤ 0.",
            CV: "Counter value.",
        },
    },
    CTUD: {
        text: "Up/down counter: rising edges of CU count up, of CD count down. RESET sets CV to 0, LOAD sets it to PV (RESET wins). Counting needs both FALSE.",
        members: {
            CU: "A rising edge counts up.",
            CD: "A rising edge counts down.",
            RESET: "TRUE: CV := 0.",
            LOAD: "TRUE: CV := PV.",
            PV: "Preset value.",
            QU: "TRUE when CV ≥ PV.",
            QD: "TRUE when CV ≤ 0.",
            CV: "Counter value.",
        },
    },
    R_TRIG: {
        text: "Rising-edge detector: Q is TRUE for one call after CLK goes FALSE → TRUE.",
        members: { CLK: "Signal to watch.", Q: "TRUE for one call on a rising edge." },
    },
    F_TRIG: {
        text: "Falling-edge detector: Q is TRUE for one call after CLK goes TRUE → FALSE.",
        members: { CLK: "Signal to watch.", Q: "TRUE for one call on a falling edge." },
    },
    SR: {
        text: "Set-dominant latch: SET1 sets Q1, RESET resets it; Q1 keeps its value while both are FALSE. If both are TRUE, Q1 is set.",
        members: { SET1: "TRUE: Q1 := TRUE (wins over RESET).", RESET: "TRUE: Q1 := FALSE.", Q1: "The latched value." },
    },
    RS: {
        text: "Reset-dominant latch: SET sets Q1, RESET1 resets it; Q1 keeps its value while both are FALSE. If both are TRUE, Q1 is reset.",
        members: { SET: "TRUE: Q1 := TRUE.", RESET1: "TRUE: Q1 := FALSE (wins over SET).", Q1: "The latched value." },
    },
};

// The standard blocks' interfaces, fetched once (retried after a failure).
let standard: Promise<FbSummary[]> | null = null;

/** Hover text (Markdown sections) for standard block `name`, or null if it isn't one. */
export async function fbHover(name: string): Promise<string[] | null> {
    standard ??= compilerApi.standardFunctionBlocks().catch(() => {
        standard = null;
        return [];
    });
    const fb = (await standard).find((f) => f.name.toUpperCase() === name.toUpperCase());
    if (!fb) return null;
    const doc = DOCS[fb.name];
    const row = (dir: string) => (v: FbSummary["inputs"][number]) => `| ${v.name} | ${dir} | ${v.dataType} | ${doc?.members[v.name] ?? ""} |`;
    const table = [
        "| Parameter | | Data type | Description |",
        "|---|---|---|---|",
        ...fb.inputs.map(row("in")),
        ...fb.outputs.map(row("out")),
    ].join("\n");
    return [
        `**${fb.name}** — standard function block`,
        doc?.text ?? "",
        table,
        `_Declare an instance under VAR (e.g. \`${fb.name}_1 : ${fb.name};\`), call it with its inputs, then read its outputs (\`${fb.name}_1.${fb.outputs[0]?.name ?? "Q"}\`)._`,
    ];
}
