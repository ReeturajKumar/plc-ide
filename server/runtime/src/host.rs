//! The runtime host: one loaded project on simulated I/O, its lifecycle (STOPPED /
//! RUNNING / PAUSED) and a background scan cycle at a fixed interval. It answers protocol
//! `Command`s with the protocol `RuntimeState`; `serve` puts it on stdin/stdout. The PLC
//! itself (scan cycle, debugger, function blocks) is `myplc_core`'s `Runtime`; this only
//! decides when it scans.

use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use myplc_core::compiler::project::{ProgramSource, ProjectContext};
use myplc_core::compiler::value::Value;
use myplc_core::io::{self, IoImage, Mapping};
use myplc_core::protocol::{Breakpoint, Command, ErrorCode, ProtocolError, RunOptions, RuntimeState, RuntimeStatus, ScanError};
use myplc_core::runtime::{Runtime, ScanOutcome, Stop};
use myplc_core::{load_on_engine_stack, ENGINE_STACK_BYTES};

const DEFAULT_SCAN_MS: u64 = 100;
const MIN_SCAN_MS: u64 = 10;
const MAX_SCAN_MS: u64 = 10_000;

struct Inner {
    runtime: Option<Runtime>,
    /// What the loaded project's names refer to, to resolve new I/O mappings live.
    context: Option<ProjectContext>,
    /// The simulated process image, the runtime's I/O provider. It outlives runs: inputs
    /// keep the values the user set.
    io: IoImage,
    status: RuntimeStatus,
    scan_time_ms: u64,
    /// The scan failure that stopped the PLC.
    error: Option<ScanError>,
    /// Kept across loads, like the editor's breakpoints.
    breakpoints: Vec<Breakpoint>,
    /// Bumped on every run/stop/load; a scan thread exits once its generation is stale,
    /// so restarting never leaves two threads scanning the same program.
    generation: u64,
    /// Told about every finished scan (see `Host::on_scan`).
    observer: Option<ScanObserver>,
}

/// Sees the state after each scan that completed or failed; returning false asks the host
/// to stop scanning (the PLC is then PAUSED, its state kept). It runs on the scan thread
/// with the host locked, so it must not call the host.
pub type ScanObserver = Box<dyn FnMut(&RuntimeState) -> bool + Send>;

impl Inner {
    fn state(&self) -> RuntimeState {
        let engine = self.runtime.as_ref().map(Runtime::state).unwrap_or_default();
        RuntimeState::from_engine(self.status, self.scan_time_ms, engine, self.io.points().to_vec(), self.error.clone())
    }

    /// How a running scan may stop: at breakpoints, if there are any.
    fn breakpoint_stop(&self) -> Stop {
        match &self.runtime {
            Some(runtime) if runtime.has_breakpoints() => Stop::AtBreakpoint,
            _ => Stop::Never,
        }
    }

    /// One PLC scan on the simulated I/O. The debugger may stop it before a statement
    /// (`stop`): the PLC is then PAUSED with the scan half done. A failing program stops
    /// the PLC (outputs go to their safe state). Returns whether the PLC can still scan.
    fn scan(&mut self, stop: Stop) -> bool {
        let Some(runtime) = self.runtime.as_mut() else { return false };
        let cycle_ms = i64::try_from(self.scan_time_ms).unwrap_or(i64::MAX);
        let can_scan = match runtime.scan_cycle(&mut self.io, cycle_ms, stop) {
            ScanOutcome::Completed => true,
            ScanOutcome::Paused(_) => {
                self.status = RuntimeStatus::Paused;
                return true; // mid-scan: not finished, nothing to report yet
            }
            ScanOutcome::Failed(failure) => {
                self.error = Some(ScanError { program: failure.program, error: failure.error });
                self.status = RuntimeStatus::Stopped;
                self.io.reset_outputs();
                false
            }
        };
        if let Some(mut observer) = self.observer.take() {
            if !observer(&self.state()) && self.status == RuntimeStatus::Running {
                self.status = RuntimeStatus::Paused;
            }
            self.observer = Some(observer);
        }
        can_scan
    }

    fn breakpoint_pairs(&self) -> Vec<(String, usize)> {
        self.breakpoints.iter().map(|b| (b.file.clone(), b.line)).collect()
    }
}

fn rejected(message: impl Into<String>) -> ProtocolError {
    ProtocolError::new(ErrorCode::Rejected, message)
}

// A poisoned lock only means some earlier holder panicked; the data is still usable,
// and the host must keep serving rather than propagate the panic.
fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub struct Host {
    inner: Arc<Mutex<Inner>>,
}

impl Default for Host {
    fn default() -> Self {
        Self::new()
    }
}

impl Host {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                runtime: None,
                context: None,
                io: IoImage::new(&io::CONFIG),
                status: RuntimeStatus::Stopped,
                scan_time_ms: DEFAULT_SCAN_MS,
                error: None,
                breakpoints: Vec::new(),
                generation: 0,
                observer: None,
            })),
        }
    }

    pub fn state(&self) -> RuntimeState {
        lock(&self.inner).state()
    }

    /// Watch every scan as it finishes (e.g. to trace it, or to end a run after N scans).
    pub fn on_scan(&self, observer: impl FnMut(&RuntimeState) -> bool + Send + 'static) {
        lock(&self.inner).observer = Some(Box::new(observer));
    }

    /// Carry out one command; the state afterwards, or why it was refused.
    pub fn execute(&self, command: Command) -> Result<RuntimeState, ProtocolError> {
        match command {
            Command::LoadProject { programs, mappings, scan_time_ms } => self.load_project(&programs, &mappings, scan_time_ms),
            Command::Run(options) => self.run(options.unwrap_or_default()),
            Command::Stop => Ok(self.stop()),
            Command::Pause => self.pause(),
            Command::Resume => self.resume(),
            Command::StepScan => self.step_scan(),
            Command::StepStatement => self.step_statement(),
            Command::SetInput { address, value } => {
                let mut inner = lock(&self.inner);
                inner.io.set_input(&address, value).map_err(rejected)?;
                Ok(inner.state())
            }
            Command::SetVariable { program, name, value } => {
                let mut inner = lock(&self.inner);
                let runtime = inner.runtime.as_mut().ok_or_else(|| rejected("Press RUN to load the program before changing inputs."))?;
                runtime.set_in(&program, &name, Value::Bool(value)).map_err(|e| rejected(e.message))?;
                Ok(inner.state())
            }
            Command::SetBreakpoints { breakpoints } => {
                let mut inner = lock(&self.inner);
                inner.breakpoints = breakpoints;
                let pairs = inner.breakpoint_pairs();
                if let Some(runtime) = inner.runtime.as_mut() {
                    runtime.set_breakpoints(&pairs);
                }
                Ok(inner.state())
            }
            Command::ApplyIoMappings { mappings } => self.apply_mappings(&mappings),
            Command::GetState => Ok(self.state()),
        }
    }

    /// Compile and load: every variable at its initial value, the PLC STOPPED. Any running
    /// scan stops. With any compile error nothing stays loaded. Simulated inputs keep their
    /// values; outputs go to their safe state.
    fn load_project(&self, programs: &[ProgramSource], mappings: &[Mapping], scan_time_ms: Option<u64>) -> Result<RuntimeState, ProtocolError> {
        let loaded = load_on_engine_stack(programs, mappings);
        let mut inner = lock(&self.inner);
        inner.generation += 1;
        inner.status = RuntimeStatus::Stopped;
        inner.error = None; // compile errors are returned, not shown as a scan failure
        inner.io.reset_outputs();
        let (mut runtime, context) = match loaded {
            (Some(loaded), _) => loaded,
            (None, report) => {
                inner.runtime = None;
                inner.context = None;
                return Err(ProtocolError::new(ErrorCode::CompilationError(report), "Compilation failed."));
            }
        };
        runtime.set_breakpoints(&inner.breakpoint_pairs());
        inner.runtime = Some(runtime);
        inner.context = Some(context);
        inner.scan_time_ms = scan_time_ms.unwrap_or(DEFAULT_SCAN_MS).clamp(MIN_SCAN_MS, MAX_SCAN_MS);
        Ok(inner.state())
    }

    /// Start scanning the loaded project from its initial state (or load it paused, ready
    /// for STEP).
    fn run(&self, options: RunOptions) -> Result<RuntimeState, ProtocolError> {
        let mut inner = lock(&self.inner);
        let Some(runtime) = inner.runtime.as_mut() else { return Err(rejected("Load a project before RUN.")) };
        runtime.reset();
        inner.generation += 1;
        inner.error = None;
        inner.io.reset_outputs();
        inner.status = if options.paused { RuntimeStatus::Paused } else { RuntimeStatus::Running };
        let generation = inner.generation;
        let state = inner.state();
        drop(inner);

        if self.spawn_scan_loop(generation).is_err() {
            lock(&self.inner).status = RuntimeStatus::Stopped;
            return Err(ProtocolError::new(ErrorCode::Unavailable, "Could not start the PLC scan cycle."));
        }
        Ok(state)
    }

    fn spawn_scan_loop(&self, generation: u64) -> std::io::Result<()> {
        let inner = Arc::clone(&self.inner);
        let scan_loop = move || loop {
            // One scan = READ INPUTS → EXECUTE → UPDATE OUTPUTS, under the lock, so every
            // scan sees a consistent input image and commands never see half a scan.
            let interval = {
                let mut state = lock(&inner);
                if state.generation != generation || state.status == RuntimeStatus::Stopped {
                    return;
                }
                if state.status == RuntimeStatus::Running {
                    let stop = state.breakpoint_stop();
                    if !state.scan(stop) {
                        return;
                    }
                }
                state.scan_time_ms
            };
            thread::sleep(Duration::from_millis(interval)); // WAIT, then next scan
        };
        thread::Builder::new().name("plc-scan".into()).stack_size(ENGINE_STACK_BYTES).spawn(scan_loop).map(|_| ())
    }

    /// Stop scanning and reset variables to their initial values and outputs to their safe
    /// state. Simulated inputs are kept. The loaded programs stay, so the monitor still
    /// lists their variables.
    fn stop(&self) -> RuntimeState {
        let mut inner = lock(&self.inner);
        inner.generation += 1;
        inner.status = RuntimeStatus::Stopped;
        inner.error = None;
        inner.io.reset_outputs();
        if let Some(runtime) = inner.runtime.as_mut() {
            runtime.reset();
        }
        inner.state()
    }

    /// Suspend scanning, keeping variables and cycle count.
    fn pause(&self) -> Result<RuntimeState, ProtocolError> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Running {
            return Err(rejected("The program is not running."));
        }
        inner.status = RuntimeStatus::Paused;
        Ok(inner.state())
    }

    fn resume(&self) -> Result<RuntimeState, ProtocolError> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Paused {
            return Err(rejected("The program is not paused."));
        }
        inner.status = RuntimeStatus::Running;
        Ok(inner.state())
    }

    /// STEP SCAN: finish the scan the debugger stopped (or run one complete scan), then
    /// stay paused. Breakpoints don't stop it: it always ends with a complete scan.
    fn step_scan(&self) -> Result<RuntimeState, ProtocolError> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Paused {
            return Err(rejected("STEP runs one scan while the PLC is paused."));
        }
        inner.scan(Stop::Never);
        Ok(inner.state())
    }

    /// STEP STATEMENT: run the statement the debugger stopped at and stop before the next
    /// one, which may be in the next program. After the scan's last statement, the outputs
    /// are written and the next scan starts, stopping at its first statement.
    fn step_statement(&self) -> Result<RuntimeState, ProtocolError> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Paused {
            return Err(rejected("STEP STATEMENT runs one statement while the PLC is paused."));
        }
        let still_scanning = inner.scan(Stop::NextStatement);
        if still_scanning && inner.runtime.as_ref().is_some_and(|rt| rt.location().is_none()) {
            inner.scan(Stop::NextStatement);
        }
        Ok(inner.state())
    }

    /// Change the I/O mappings of the loaded programs right away (from the next scan).
    /// Invalid mappings change nothing. Without loaded programs there is nothing to apply.
    fn apply_mappings(&self, mappings: &[Mapping]) -> Result<RuntimeState, ProtocolError> {
        let mut inner = lock(&self.inner);
        if let Inner { runtime: Some(runtime), context: Some(context), .. } = &mut *inner {
            let resolved = context
                .resolve_mappings(mappings)
                .map_err(|errors| ProtocolError::new(ErrorCode::IoMappingError(errors), "The I/O mappings are invalid."))?;
            runtime.set_mappings(&resolved);
        }
        Ok(inner.state())
    }
}

