import Button from "../common/Button";
import type { RuntimeStatus } from "../../types/protocol";
import { pauseProgram, resumeProgram, runProgram, stepProgram, stepStatement, stopProgram } from "../../store/runActions";

/**
 * Run and debug controls; each is enabled only where it is a valid transition:
 * STOPPED → RUN · RUNNING → PAUSE, STOP · PAUSED → RESUME, STEP SCAN, STEP STATEMENT, STOP.
 */
const RuntimeControls = ({ status }: { status: RuntimeStatus }) => {
    const paused = status === "PAUSED";
    return (
        <div className="flex flex-wrap items-center gap-2">
            <Button onClick={() => void runProgram()} disabled={status !== "STOPPED"} title="Run (F5)">
                ▶ RUN
            </Button>
            {paused ? (
                <Button variant="secondary" onClick={() => void resumeProgram()} title="Resume (F5)">
                    ▶ RESUME
                </Button>
            ) : (
                <Button variant="secondary" onClick={() => void pauseProgram()} disabled={status !== "RUNNING"} title="Pause">
                    ❚❚ PAUSE
                </Button>
            )}
            <Button variant="secondary" onClick={() => void stopProgram()} disabled={status === "STOPPED"} title="Stop (Shift+F5)">
                ■ STOP
            </Button>
            <Button variant="secondary" onClick={() => void stepStatement()} disabled={!paused} title="Step Statement: run the next statement, then pause (F11)">
                ↴ STEP STATEMENT
            </Button>
            <Button variant="secondary" onClick={() => void stepProgram()} disabled={!paused} title="Step Scan: finish this scan / run one complete scan, then pause (F10)">
                ⏭ STEP SCAN
            </Button>
        </div>
    );
};

export default RuntimeControls;
