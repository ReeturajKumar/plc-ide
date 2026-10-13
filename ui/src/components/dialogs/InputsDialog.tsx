import { useState } from "react";
import Dialog from "../common/Dialog";
import Button from "../common/Button";
import { useUIStore, type InputValues } from "../../store/uiStore";
import { inputText, parseInput } from "../Simulator/format";

/**
 * Before RUN: the program's inputs (variables no program writes), to fill in. Run starts
 * with the typed values, Skip with the current ones, Cancel doesn't start.
 */
const InputsDialog = () => {
    const prompt = useUIStore((s) => s.inputsPrompt);
    const resolve = useUIStore((s) => s.resolveInputs);
    const [texts, setTexts] = useState<Record<string, string>>({});
    const [bools, setBools] = useState<Record<string, boolean>>({});
    if (!prompt) return null;

    const key = (v: (typeof prompt)[number]) => `${v.program}.${v.name}`;
    const showProgram = new Set(prompt.map((v) => v.program)).size > 1;
    const parsed = prompt.map((v) =>
        v.dataType === "BOOL"
            ? ({ ok: true, value: bools[key(v)] ?? (v.value as boolean) } as const)
            : parseInput(v.dataType, texts[key(v)] ?? inputText(v.value)),
    );
    const valid = parsed.every((p) => p.ok);
    const close = (values: InputValues | null) => {
        setTexts({});
        setBools({});
        resolve(values);
    };
    const run = () => {
        if (!valid) return;
        close(prompt.map((v, i) => [v, (parsed[i] as { value: boolean | number | string }).value]));
    };

    return (
        <Dialog
            title="Program inputs"
            onClose={() => close(null)}
            footer={
                <>
                    <Button variant="secondary" onClick={() => close(null)}>
                        Cancel
                    </Button>
                    <Button variant="secondary" onClick={() => close([])} title="Run with the current values">
                        Skip
                    </Button>
                    <Button onClick={run} disabled={!valid}>
                        Run
                    </Button>
                </>
            }
        >
            <p className="text-xs text-[var(--theme-text-secondary)] pb-3">
                No program sets these variables. Set values to test them in the Simulator.
            </p>
            <form
                className="grid grid-cols-[auto_auto_1fr] items-center gap-x-3 gap-y-2 font-mono text-xs max-h-[50vh] overflow-y-auto"
                onSubmit={(e) => {
                    e.preventDefault();
                    run();
                }}
            >
                {prompt.map((v, i) => {
                    const id = `input-${key(v)}`;
                    const result = parsed[i];
                    return (
                        <div key={key(v)} className="contents">
                            <label htmlFor={id} className="text-[var(--theme-text-primary)]">
                                {showProgram ? `${v.program}.${v.name}` : v.name}
                            </label>
                            <span className="text-[#4ec9b0]">{v.dataType}</span>
                            {v.dataType === "BOOL" ? (
                                <input
                                    id={id}
                                    type="checkbox"
                                    checked={bools[key(v)] ?? (v.value as boolean)}
                                    onChange={(e) => setBools({ ...bools, [key(v)]: e.target.checked })}
                                    className="justify-self-start w-4 h-4 accent-emerald-500 cursor-pointer"
                                />
                            ) : (
                                <div>
                                    <input
                                        id={id}
                                        autoFocus={i === 0}
                                        value={texts[key(v)] ?? inputText(v.value)}
                                        onChange={(e) => setTexts({ ...texts, [key(v)]: e.target.value })}
                                        aria-invalid={!result.ok}
                                        className={`w-full px-2 py-1 rounded border bg-[var(--theme-bg-input)] text-[var(--theme-text-primary)] focus:outline-none focus:border-[var(--theme-border-focus)] ${result.ok ? "border-[var(--theme-border)]" : "border-red-500/70"
                                            }`}
                                    />
                                    {!result.ok && <div className="pt-0.5 text-[11px] text-red-400">{result.error}</div>}
                                </div>
                            )}
                        </div>
                    );
                })}
                <button type="submit" hidden />
            </form>
        </Dialog>
    );
};

export default InputsDialog;
