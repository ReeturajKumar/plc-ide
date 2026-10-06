import type { FunctionBlockSnapshot } from "../../types/runtime";
import { formatValue, valueClass } from "./format";

/** Function block instances, each with its inputs and read-only outputs. */
const FunctionBlockMonitor = ({ blocks }: { blocks: FunctionBlockSnapshot[] }) => {
    if (blocks.length === 0) return null;
    const showProgram = new Set(blocks.map((b) => b.program)).size > 1;

    return (
        <section aria-label="Function blocks" className="font-mono">
            <h3 className="text-[11px] font-semibold tracking-wider text-[var(--theme-text-muted)] pb-1">
                FUNCTION BLOCKS
            </h3>
            <div className="grid gap-2 grid-cols-[repeat(auto-fill,minmax(220px,1fr))] max-w-4xl">
                {blocks.map((block) => {
                    const summary = block.members
                        .filter((m) => m.isOutput)
                        .map((m) => `${m.name}=${formatValue(m.dataType, m.value)}`)
                        .join(", ");
                    return (
                        <div key={`${block.program}.${block.name}`} className="border border-[var(--theme-border)] rounded px-3 py-2">
                            <div className="flex items-baseline gap-2">
                                <span className="text-[var(--theme-text-primary)] font-semibold">{block.name}</span>
                                <span className="text-[#4ec9b0]">{block.blockType}</span>
                                {showProgram && (
                                    <span className="ml-auto text-[11px] text-[var(--theme-text-muted)]">{block.program}</span>
                                )}
                            </div>
                            <div className="text-[11px] text-[var(--theme-text-muted)] truncate" title={summary}>
                                {summary}
                            </div>
                            <table className="w-full mt-1.5 text-left">
                                <tbody>
                                    {block.members.map((m) => (
                                        <tr key={m.name} className="border-t border-[var(--theme-border)]">
                                            <td className="py-0.5 pr-3 text-[var(--theme-text-secondary)]">{m.name}</td>
                                            <td className={`py-0.5 pr-3 font-semibold tabular-nums ${valueClass(m.value)}`}>
                                                {formatValue(m.dataType, m.value)}
                                            </td>
                                            <td className="py-0.5 text-right text-[10px] text-[var(--theme-text-muted)]">
                                                {m.isInternal ? "var" : m.isOutput ? "out" : "in"}
                                            </td>
                                        </tr>
                                    ))}
                                </tbody>
                            </table>
                        </div>
                    );
                })}
            </div>
        </section>
    );
};

export default FunctionBlockMonitor;
