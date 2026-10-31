import { useState, type ReactNode } from "react";
import Dialog from "../common/Dialog";
import Button from "../common/Button";
import { AUTOSAVE_DELAY_MAX, AUTOSAVE_DELAY_MIN, useUIStore } from "../../store/uiStore";
import { getConnection, setConnection } from "../../utils/connection";
import { reconnect } from "../../utils/runtimeSocket";

const INPUT_CLASS =
    "px-2 py-1 rounded border bg-[var(--theme-bg-input)] text-xs text-[var(--theme-text-primary)] border-[var(--theme-border)] focus:outline-none focus:border-[var(--theme-border-focus)]";

const PRESETS = [1000, 3000, 5000, 10_000, 30_000];
const seconds = (ms: number) => `${ms / 1000} s`;

/** A setting: label and help on the left, its control on the right. */
const Row = ({ label, help, children }: { label: string; help: string; children: ReactNode }) => (
    <div className="flex items-start justify-between gap-4 py-3 border-t border-[var(--theme-border)] first:border-t-0">
        <div>
            <div className="text-xs font-medium text-[var(--theme-text-primary)]">{label}</div>
            <div className="text-[11px] text-[var(--theme-text-muted)] pt-0.5">{help}</div>
        </div>
        <div className="shrink-0">{children}</div>
    </div>
);

const Toggle = ({ on, onChange, label }: { on: boolean; onChange: (on: boolean) => void; label: string }) => (
    <button
        role="switch"
        aria-checked={on}
        aria-label={label}
        onClick={() => onChange(!on)}
        className={`relative w-9 h-5 rounded-full transition-colors cursor-pointer focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--theme-border-focus)] ${on ? "bg-emerald-500/80" : "bg-[var(--theme-bg-input)] border border-[var(--theme-border)]"
            }`}
    >
        <span className={`absolute top-0.5 w-4 h-4 rounded-full bg-white transition-all ${on ? "left-[18px]" : "left-0.5"}`} />
    </button>
);

/** Editor settings (File → Settings…, Ctrl+,): auto save and view options, kept on this machine. */
const SettingsDialog = () => {
    const ui = useUIStore();
    const close = () => ui.closeDialog();
    const [delayText, setDelayText] = useState(String(ui.autoSaveDelay));
    const typed = Number(delayText);
    const delayValid = delayText.trim() !== "" && Number.isFinite(typed) && typed >= AUTOSAVE_DELAY_MIN && typed <= AUTOSAVE_DELAY_MAX;

    const applyDelay = (ms: number) => {
        ui.setAutoSaveDelay(ms);
        setDelayText(String(ms));
    };

    const [conn, setConn] = useState(getConnection());
    const saveBackend = () => {
        setConnection(conn);
        reconnect();
        ui.notify("info", "Backend connection saved.");
    };

    return (
        <Dialog title="Settings" onClose={close} width="w-[min(48rem,95vw)]" footer={<Button onClick={close}>Done</Button>}>
            <div className="grid grid-cols-1 md:grid-cols-2 gap-x-10">
                {/* Backend */}
                <section className="flex flex-col gap-3 md:border-r md:border-[var(--theme-border)] md:pr-10">
                    <div className="text-[10px] font-semibold tracking-wider text-[var(--theme-text-muted)] uppercase">Backend</div>
                    <div>
                        <label className="text-xs font-medium text-[var(--theme-text-primary)]">Address</label>
                        <input
                            type="text"
                            value={conn.url}
                            onChange={(e) => setConn({ ...conn, url: e.target.value })}
                            aria-label="Backend address"
                            className={`${INPUT_CLASS} w-full mt-1`}
                            placeholder="wss://…"
                        />
                        <div className="text-[11px] text-[var(--theme-text-muted)] mt-1">PLC server WebSocket URL.</div>
                    </div>
                    <div>
                        <label className="text-xs font-medium text-[var(--theme-text-primary)]">Token</label>
                        <input
                            type="password"
                            value={conn.token}
                            onChange={(e) => setConn({ ...conn, token: e.target.value })}
                            aria-label="Access token"
                            className={`${INPUT_CLASS} w-full mt-1`}
                            placeholder="token"
                        />
                        <div className="text-[11px] text-[var(--theme-text-muted)] mt-1">Stored on this device only. Blank if none.</div>
                    </div>
                    <div className="flex justify-end">
                        <Button onClick={saveBackend}>Save &amp; reconnect</Button>
                    </div>
                </section>

                {/* Editor */}
                <section>
                    <div className="text-[10px] font-semibold tracking-wider text-[var(--theme-text-muted)] uppercase pb-1">Editor</div>

                    <Row label="Auto Save" help="Save changed files by themselves after a pause in typing.">
                        <Toggle on={ui.autoSave} onChange={ui.setAutoSave} label="Auto Save" />
                    </Row>

                    <Row
                        label="Auto Save Delay"
                        help={``}
                    >
                        <div className={`flex flex-col items-end gap-1.5 ${ui.autoSave ? "" : "opacity-50"}`}>
                            <div className="flex items-center gap-1.5">
                                <input
                                    type="number"
                                    min={AUTOSAVE_DELAY_MIN}
                                    max={AUTOSAVE_DELAY_MAX}
                                    step={100}
                                    value={delayText}
                                    disabled={!ui.autoSave}
                                    onChange={(e) => {
                                        setDelayText(e.target.value);
                                        const ms = Number(e.target.value);
                                        if (ms >= AUTOSAVE_DELAY_MIN && ms <= AUTOSAVE_DELAY_MAX) ui.setAutoSaveDelay(ms);
                                    }}
                                    aria-label="Auto save delay in milliseconds"
                                    aria-invalid={!delayValid}
                                    className={`w-24 px-2 py-1 rounded border bg-[var(--theme-bg-input)] text-xs text-[var(--theme-text-primary)] focus:outline-none focus:border-[var(--theme-border-focus)] disabled:cursor-not-allowed ${delayValid ? "border-[var(--theme-border)]" : "border-red-500/70"
                                        }`}
                                />
                                <span className="text-[11px] text-[var(--theme-text-muted)]">ms</span>
                            </div>
                            <div className="flex gap-1">
                                {PRESETS.map((ms) => (
                                    <button
                                        key={ms}
                                        disabled={!ui.autoSave}
                                        onClick={() => applyDelay(ms)}
                                        className={`px-1.5 py-0.5 rounded border text-[10px] cursor-pointer disabled:cursor-not-allowed ${ui.autoSaveDelay === ms
                                            ? "border-emerald-500/60 bg-emerald-500/15 text-emerald-300"
                                            : "border-[var(--theme-border)] text-[var(--theme-text-secondary)] hover:bg-[var(--theme-bg-hover)]"
                                            }`}
                                    >
                                        {seconds(ms)}
                                    </button>
                                ))}
                            </div>
                            {!delayValid && (
                                <div className="text-[11px] text-red-400">
                                    Between {AUTOSAVE_DELAY_MIN} and {AUTOSAVE_DELAY_MAX} ms (now {ui.autoSaveDelay} ms)
                                </div>
                            )}
                        </div>
                    </Row>

                    <Row label="Word Wrap" help="Wrap long lines at the editor's edge.">
                        <Toggle on={ui.wordWrap} onChange={ui.toggleWordWrap} label="Word Wrap" />
                    </Row>

                    <Row label="Minimap" help="Show the code overview at the editor's right edge.">
                        <Toggle on={ui.minimap} onChange={ui.toggleMinimap} label="Minimap" />
                    </Row>
                </section>
            </div>
        </Dialog>
    );
};

export default SettingsDialog;
