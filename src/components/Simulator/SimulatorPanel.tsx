import RuntimeControls from "./RuntimeControls";
import VariableMonitor from "./VariableMonitor";
import FunctionBlockMonitor from "./FunctionBlockMonitor";
import DebugInfo from "./DebugInfo";
import { useSimulatorStore } from "../../store/simulatorStore";
import { useProjectStore } from "../../store/projectStore";

const statusColor = {
    RUNNING: "text-emerald-400",
    PAUSED: "text-amber-400",
    STOPPED: "text-[var(--theme-text-muted)]",
} as const;

const SimulatorPanel = () => {
    const runtime = useSimulatorStore((s) => s.runtime);
    const errors = useSimulatorStore((s) => s.errors);
    // Loaded programs; before RUN, the project's (what RUN will execute, in this order).
    const registered = useProjectStore((s) => s.project?.programs);
    const fileKinds = useProjectStore((s) => s.fileKinds);
    // Files with only FUNCTION_BLOCKs define types; they don't run in the scan.
    const programs = runtime.programs.length
        ? runtime.programs
        : (registered ?? []).filter((p) => fileKinds[p.path]?.hasProgram !== false).map((p) => p.name);

    return (
        <div className="flex flex-col gap-3 font-sans text-xs whitespace-normal text-[var(--theme-text-secondary)]">
            <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
                <span className="font-semibold tracking-wider text-[var(--theme-text-primary)]">PLC SIMULATOR</span>
                <RuntimeControls status={runtime.status} />
            </div>

            <DebugInfo />

            <div className="flex flex-wrap gap-x-6 gap-y-1">
                <span>
                    Status: <span className={`font-semibold ${statusColor[runtime.status]}`}>{runtime.status}</span>
                </span>
                <span>
                    {programs.length === 1 ? "Program" : "Programs"}:{" "}
                    <span className="text-[var(--theme-text-primary)]">{programs.length ? programs.join(" → ") : "—"}</span>
                </span>
                <span>
                    Cycle: <span className="text-[var(--theme-text-primary)] tabular-nums">{runtime.cycleCount}</span>
                </span>
                <span>
                    Scan Time: <span className="text-[var(--theme-text-primary)]">{runtime.scanTimeMs} ms</span>
                </span>
            </div>

            {errors.length > 0 && (
                <div role="alert" className="max-w-xl rounded border border-red-500/40 bg-red-500/10 px-3 py-2 font-mono text-red-300">
                    <div className="font-semibold">
                        {errors.length === 1 ? "1 ERROR FOUND" : `${errors.length} ERRORS FOUND`} — program not running
                    </div>
                    {errors.map((e, i) => (
                        <div key={i}>
                            {e.fileName && `${e.fileName} `}
                            {e.line > 0 && `Line ${e.line}, Column ${e.column}: `}
                            {e.message}
                        </div>
                    ))}
                </div>
            )}

            {runtime.variables.length === 0 && runtime.functionBlocks.length === 0 ? (
                <p className="text-[var(--theme-text-muted)]">Press RUN to load the program's variables.</p>
            ) : (
                <>
                    <VariableMonitor variables={runtime.variables} status={runtime.status} />
                    <FunctionBlockMonitor blocks={runtime.functionBlocks} />
                </>
            )}
        </div>
    );
};

export default SimulatorPanel;
