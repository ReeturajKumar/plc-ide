import { useSimulatorStore } from "../../store/simulatorStore";
import { useEditorStore } from "../../store/editorStore";
import { locationPath } from "../../store/runActions";

/**
 * Where the debugger stopped: program, source file, function block instance, line and
 * the statement about to run. Nothing while the PLC runs or stops between scans.
 */
const DebugInfo = () => {
    const status = useSimulatorStore((s) => s.runtime.status);
    const location = useSimulatorStore((s) => s.runtime.location);
    const path = location ? locationPath(location.file) : undefined;
    // The statement's text, from the open file (the debugger opens it when it stops).
    const statement = useEditorStore((s) =>
        location && path ? s.openFiles.find((f) => f.path === path)?.content.split(/\r?\n/)[location.line - 1]?.trim() : undefined
    );
    if (!location) return null;

    const rows: [string, string][] = [
        ["Status", status],
        ["Program", location.program],
        ...(location.functionBlock ? ([["Function Block", location.functionBlock], ["Instance", location.instance ?? "—"]] as [string, string][]) : []),
        ["File", `${location.file}.st`],
        ["Line", String(location.line)],
        ["Statement", statement ?? "—"],
    ];
    return (
        <section aria-label="Debugger" className="max-w-xl rounded border border-amber-500/40 bg-amber-500/10 px-3 py-2 font-mono text-[11px]">
            <div className="mb-1 font-semibold tracking-wider text-amber-300">DEBUGGER — paused before this statement</div>
            <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-0.5">
                {rows.map(([label, value]) => (
                    <div key={label} className="contents">
                        <dt className="text-[var(--theme-text-muted)]">{label}</dt>
                        <dd className="text-[var(--theme-text-primary)] truncate">{value}</dd>
                    </div>
                ))}
            </dl>
            <div className="mt-1 text-[var(--theme-text-muted)]">
                F11 step statement · F10 step scan · F5 resume · Shift+F5 stop
            </div>
        </section>
    );
};

export default DebugInfo;
