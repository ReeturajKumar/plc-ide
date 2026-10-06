import type { RuntimeStatus, VariableSnapshot } from "../../types/runtime";
import { toggleInput } from "../../store/runActions";
import { formatValue, valueClass } from "./format";

interface VariableMonitorProps {
    variables: VariableSnapshot[];
    status: RuntimeStatus;
}

const VariableMonitor = ({ variables, status }: VariableMonitorProps) => {
    if (variables.length === 0) return null;

    // Inputs only matter to a live program; STOP resets values and RUN reloads the source.
    const canDrive = status !== "STOPPED";
    const showProgram = new Set(variables.map((v) => v.program)).size > 1;

    return (
        <table className="w-full max-w-xl text-left font-mono">
            <caption className="text-left text-[11px] font-semibold tracking-wider text-[var(--theme-text-muted)] pb-1">
                VARIABLES
            </caption>
            <thead>
                <tr className="text-[11px] text-[var(--theme-text-muted)]">
                    {showProgram && <th className="font-normal py-1 pr-6">Program</th>}
                    <th className="font-normal py-1 pr-6">Name</th>
                    <th className="font-normal py-1 pr-6">Type</th>
                    <th className="font-normal py-1 pr-6">Value</th>
                    <th className="font-normal py-1">Input</th>
                </tr>
            </thead>
            <tbody>
                {variables.map((v) => (
                    <tr key={`${v.program}.${v.name}`} className="border-t border-[var(--theme-border)]">
                        {showProgram && <td className="py-1.5 pr-6 text-[var(--theme-text-muted)]">{v.program}</td>}
                        <td className="py-1.5 pr-6 text-[var(--theme-text-primary)]">{v.name}</td>
                        <td className="py-1.5 pr-6 text-[#4ec9b0]">{v.dataType}</td>
                        <td className={`py-1.5 pr-6 font-semibold tabular-nums ${valueClass(v.value)}`}>{formatValue(v.dataType, v.value)}</td>
                        <td className="py-1.5">
                            {v.io ? (
                                <span className="text-[11px] text-sky-300" title={`Mapped to ${v.io}`}>
                                    {v.io}
                                </span>
                            ) : v.isInput && typeof v.value === "boolean" ? (
                                <button
                                    onClick={() => void toggleInput(v.program, v.name, v.value as boolean)}
                                    disabled={!canDrive}
                                    aria-pressed={v.value}
                                    aria-label={`Toggle ${v.name}`}
                                    title={canDrive ? `Set ${v.name} to ${v.value ? "FALSE" : "TRUE"}` : "Run the program to drive inputs"}
                                    className={`w-14 py-0.5 rounded border text-[11px] font-semibold transition-colors cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)] ${
                                        v.value
                                            ? "bg-emerald-500/20 border-emerald-500/60 text-emerald-300"
                                            : "bg-[var(--theme-bg-input)] border-[var(--theme-border)] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
                                    }`}
                                >
                                    {v.value ? "ON" : "OFF"}
                                </button>
                            ) : (
                                <span className="text-[11px] text-[var(--theme-text-muted)]">{v.isInput ? "input" : "output"}</span>
                            )}
                        </td>
                    </tr>
                ))}
            </tbody>
        </table>
    );
};

export default VariableMonitor;
