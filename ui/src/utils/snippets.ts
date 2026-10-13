/**
 * Operation templates (from the team's ST reference sheet): math, logic, comparison and
 * MOVE. Each is a Monaco snippet: `${1:IN1}` fields are selected in turn with Tab.
 * Function blocks are inserted by the function block library instead (declaration + call).
 */
/** Drag-and-drop data type for an operation from the library (its snippet template). */
export const SNIPPET_DRAG_TYPE = "application/x-myplc-snippet";

export interface Snippet {
    /** What the user picks or types, e.g. "ADD". */
    name: string;
    group: "Math" | "Logic" | "Compare" | "Move";
    /** Monaco snippet text. */
    template: string;
    description: string;
    /** Parameter, data types, description: the reference sheet's table. */
    params: [string, string, string][];
}

const binary = (
    name: string,
    group: Snippet["group"],
    op: string,
    types: string,
    result: string,
    description: string,
    in1: string,
    in2: string,
): Snippet => ({
    name,
    group,
    template: `\${1:Result} := \${2:IN1} ${op} \${3:IN2};`,
    description,
    params: [
        ["IN1", types, in1],
        ["IN2", types, in2],
        ["OUT", result, "Output value"],
    ],
});

const compare = (name: string, op: string, what: string) =>
    binary(name, "Compare", op, "ELEMENTARY", "BOOL", `TRUE if IN1 is ${what} IN2 (\`${op}\`).`, "First value", "Second value");

export const SNIPPETS: Snippet[] = [
    binary("ADD", "Math", "+", "ANY_NUM", "ANY_NUM", "Adds IN1 and IN2 (`+`).", "First addend", "Second addend"),
    binary("SUB", "Math", "-", "ANY_NUM", "ANY_NUM", "Subtracts IN2 from IN1 (`-`).", "Minuend", "Subtrahend"),
    binary("MUL", "Math", "*", "ANY_NUM", "ANY_NUM", "Multiplies IN1 by IN2 (`*`).", "Multiplicand", "Multiplier"),
    binary("DIV", "Math", "/", "ANY_NUM", "ANY_NUM", "Divides IN1 by IN2 (`/`).", "Dividend", "Divisor"),
    binary("AND", "Logic", "AND", "ANY_BIT", "ANY_BIT", "Logical / bitwise AND (`AND` or `&`).", "First operand", "Second operand"),
    binary("OR", "Logic", "OR", "ANY_BIT", "ANY_BIT", "Logical / bitwise OR.", "First operand", "Second operand"),
    binary("XOR", "Logic", "XOR", "ANY_BIT", "ANY_BIT", "Logical / bitwise exclusive OR.", "First operand", "Second operand"),
    {
        name: "NOT",
        group: "Logic",
        template: "${1:Result} := NOT(${2:IN1});",
        description: "Logical / bitwise negation.",
        params: [
            ["IN1", "ANY_BIT", "Operand"],
            ["OUT", "ANY_BIT", "Negated value"],
        ],
    },
    compare("EQ", "=", "equal to"),
    compare("NE", "<>", "not equal to"),
    compare("LT", "<", "less than"),
    compare("LE", "<=", "less than or equal to"),
    compare("GT", ">", "greater than"),
    compare("GE", ">=", "greater than or equal to"),
    {
        name: "MOVE",
        group: "Move",
        template: "${1:Result} := MOVE(${2:IN});",
        description: "Copies IN to the result.",
        params: [
            ["IN", "ANY", "Value to copy"],
            ["OUT", "ANY", "Copy of IN"],
        ],
    },
];

/** Markdown: description and the parameter table. */
export function snippetDoc(s: Pick<Snippet, "description" | "params">): string {
    const rows = s.params.map(([p, t, d]) => `| ${p} | ${t} | ${d} |`).join("\n");
    return `${s.description}\n\n| Parameter | Data types | Description |\n|---|---|---|\n${rows}`;
}

/** The template as plain text, fields shown by name: `Result := IN1 + IN2;`. */
export const snippetPreview = (template: string) => template.replace(/\$\{\d+:([^}]*)\}/g, "$1");
