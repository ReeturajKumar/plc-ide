use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::state::{RuntimeState, RuntimeStatus};
use crate::st::error::{StError, StErrors, StResult};
use crate::st::function_blocks::ScanClock;
use crate::st::parser::parse_file;
use crate::st::ast::{statement_lines, FunctionBlockDef, VarType};
use crate::st::runtime::{Runtime, Stop};
use crate::io::{self, InputValue, IoImage, Mapping};
use crate::st::value::Value;

pub const DEFAULT_SCAN_MS: u64 = 100;
const MIN_SCAN_MS: u64 = 10;
const MAX_SCAN_MS: u64 = 10_000;

/// Stack for the threads that load and scan programs. The parser, checker and runtime
/// recurse over the program tree; at `MAX_NESTING` a debug build needs about 1 MB
/// (measured), which is the whole Windows main thread where Tauri runs sync commands.
/// 8 MB is reserved address space, not committed memory, and leaves ~8x headroom.
pub const ENGINE_STACK_BYTES: usize = 8 * 1024 * 1024;

/// One program of the project, as the IDE sends it: its name and current source.
#[derive(Debug, Clone, Deserialize)]
pub struct ProgramSource {
    pub name: String,
    pub source: String,
}

/// The compilation result of one program: compiled if `errors` is empty.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompileResult {
    pub name: String,
    pub errors: StErrors,
    /// Lines where a statement starts (breakpoints can stop there); empty after a syntax error.
    pub lines: Vec<usize>,
    /// Declared variables (empty after a syntax error), e.g. for I/O mapping suggestions.
    pub variables: Vec<DeclaredVariable>,
    /// FUNCTION_BLOCKs the file defines.
    pub function_blocks: Vec<FbSummary>,
    /// The file has a PROGRAM (which runs every scan); without one it only defines types.
    pub has_program: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredVariable {
    pub name: String,
    /// BOOL, INT, …, or the function block type (TON, CTU, …).
    pub data_type: String,
}

/// A function block's interface, for the IDE's function block library.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FbSummary {
    pub name: String,
    pub inputs: Vec<DeclaredVariable>,
    pub outputs: Vec<DeclaredVariable>,
    /// Where `FUNCTION_BLOCK` is in its file (1 for a standard block's reference source).
    pub line: usize,
    /// A standard block's read-only ST definition; None for the project's own blocks.
    pub source: Option<String>,
}

impl FbSummary {
    fn of(fb: &crate::st::ast::FunctionBlockDef) -> Self {
        let vars = |decls: &[crate::st::ast::VarDecl]| {
            decls.iter().map(|d| DeclaredVariable { name: d.name.clone(), data_type: d.type_name.clone() }).collect()
        };
        Self { name: fb.name.clone(), inputs: vars(&fb.inputs), outputs: vars(&fb.outputs), line: fb.line, source: None }
    }
}

/// The standard function blocks (TON, TOF, TP, CTU, CTD, R_TRIG, F_TRIG) with their inputs
/// and outputs, for the IDE's function block library.
pub fn standard_function_blocks() -> Vec<FbSummary> {
    let vars = |members: &[(&str, crate::st::ast::DataType)]| {
        members.iter().map(|(n, t)| DeclaredVariable { name: n.to_string(), data_type: t.name().to_string() }).collect()
    };
    crate::st::function_blocks::FbKind::ALL
        .iter()
        .map(|kind| FbSummary {
            name: kind.name().to_string(),
            inputs: vars(kind.inputs()),
            outputs: vars(kind.outputs()),
            line: 1,
            source: Some(crate::st::function_blocks::reference_source(*kind)),
        })
        .collect()
}

/// A breakpoint on a program's source line.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Breakpoint {
    pub program: String,
    pub line: usize,
}

/// Compiling a project: one result per program, in order, and the I/O mapping errors.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompileReport {
    pub programs: Vec<CompileResult>,
    pub mapping: StErrors,
}

impl CompileReport {
    fn failed(programs: &[ProgramSource], message: &str) -> Self {
        let programs = programs
            .iter()
            .map(|p| CompileResult { name: p.name.clone(), errors: vec![StError::general(message)], lines: Vec::new(), variables: Vec::new(), function_blocks: Vec::new(), has_program: false })
            .collect();
        Self { programs, mapping: StErrors::new() }
    }
}

struct Inner {
    runtime: Option<Runtime>,
    /// The simulated process image. It outlives runs: inputs keep the values the user set.
    io: IoImage,
    status: RuntimeStatus,
    cycle_count: u64,
    scan_time_ms: u64,
    error: Option<StError>,
    /// The program the scan error happened in.
    error_program: Option<String>,
    /// Kept across runs, like the editor's breakpoints.
    breakpoints: Vec<Breakpoint>,
    /// Bumped on every start/stop; a scan thread exits once its generation is stale,
    /// so restarting never leaves two threads scanning the same program.
    generation: u64,
    /// PLC time in ms since RUN: exactly one scan interval per executed scan. Timers are
    /// defined by scan time, not wall-clock time, so scan 20 of a 100 ms cycle is always
    /// 2000 ms however long the scans really took, and a pause freezes them.
    clock_ms: i64,
}

impl Inner {
    fn snapshot(&self) -> RuntimeState {
        RuntimeState {
            status: self.status,
            programs: self.runtime.as_ref().map(Runtime::program_names).unwrap_or_default(),
            cycle_count: self.cycle_count,
            scan_time_ms: self.scan_time_ms,
            variables: self.runtime.as_ref().map(Runtime::variables).unwrap_or_default(),
            function_blocks: self.runtime.as_ref().map(Runtime::function_blocks).unwrap_or_default(),
            error: self.error.clone(),
            error_program: self.error_program.clone(),
            io: self.io.points().to_vec(),
            location: self.runtime.as_ref().and_then(Runtime::location).cloned(),
        }
    }

    /// How a running scan may stop: at breakpoints, if there are any.
    fn breakpoint_stop(&self) -> Stop {
        match &self.runtime {
            Some(runtime) if runtime.has_breakpoints() => Stop::AtBreakpoint,
            _ => Stop::Never,
        }
    }

    /// One PLC scan: READ INPUTS -> every program once, in project order -> WRITE OUTPUTS.
    /// The debugger may stop it before a statement (`stop`): the PLC is then PAUSED with
    /// the scan half done, and the next call carries on from there instead of starting a
    /// new scan. A failing program stops the PLC (outputs go to their safe state). Returns
    /// whether the PLC can still scan.
    fn scan(&mut self, stop: Stop) -> bool {
        let Some(runtime) = self.runtime.as_mut() else { return false };
        let outcome = if runtime.location().is_some() {
            runtime.proceed(stop)
        } else {
            let cycle_ms = i64::try_from(self.scan_time_ms).unwrap_or(i64::MAX);
            self.clock_ms = self.clock_ms.saturating_add(cycle_ms);
            runtime.read_inputs(&self.io);
            runtime.run(ScanClock { now_ms: self.clock_ms, cycle_ms }, stop)
        };
        match outcome {
            Ok(None) => {
                runtime.write_outputs(&mut self.io);
                self.cycle_count += 1;
                true
            }
            Ok(Some(_)) => {
                self.status = RuntimeStatus::Paused;
                true
            }
            Err(failure) => {
                self.error = Some(failure.error);
                self.error_program = Some(failure.program);
                self.status = RuntimeStatus::Stopped;
                self.io.reset_outputs();
                false
            }
        }
    }
}

/// Parse and analyze every program on an engine-sized stack (see `ENGINE_STACK_BYTES`).
/// Every program is compiled even when others fail, so all errors come back at once:
/// a syntax error stops its own program at the first problem; semantic errors are all
/// collected. A program with a syntax error can't share its variables with the others.
fn load(programs: &[ProgramSource], mappings: &[Mapping]) -> (Option<Runtime>, CompileReport) {
    let compile = || {
        let mut errors: Vec<StErrors> = vec![StErrors::new(); programs.len()];
        let mut lines: Vec<Vec<usize>> = vec![Vec::new(); programs.len()];
        let mut variables: Vec<Vec<DeclaredVariable>> = vec![Vec::new(); programs.len()];
        let mut defined: Vec<Vec<FbSummary>> = vec![Vec::new(); programs.len()];
        let mut has_program = vec![false; programs.len()];
        let mut parsed = Vec::new();
        let mut index = Vec::new();
        // Every file's FUNCTION_BLOCKs, and the file each came from.
        let (mut fbs, mut fb_file) = (Vec::new(), Vec::new());
        for (i, program) in programs.iter().enumerate() {
            if programs[..i].iter().any(|p| p.name.eq_ignore_ascii_case(&program.name)) {
                errors[i].push(StError::general(format!("Program '{}' already exists.", program.name)));
            }
            match parse_file(&program.source) {
                Ok(file) => {
                    defined[i] = file.function_blocks.iter().map(FbSummary::of).collect();
                    fb_file.extend(std::iter::repeat_n(i, file.function_blocks.len()));
                    // Breakpoints can stop in block bodies too.
                    lines[i] = file.function_blocks.iter().flat_map(|fb| statement_lines(&fb.body)).collect();
                    fbs.extend(file.function_blocks.into_iter().map(|fb| FunctionBlockDef { file: program.name.clone(), ..fb }));
                    // A file with only FUNCTION_BLOCKs defines types; it has nothing to run.
                    let Some(ast) = file.program else { continue };
                    has_program[i] = true;
                    lines[i].extend(statement_lines(&ast.body));
                    lines[i].sort_unstable();
                    lines[i].dedup();
                    variables[i] = ast
                        .vars
                        .iter()
                        .map(|v| DeclaredVariable {
                            name: v.name.clone(),
                            data_type: match v.var_type {
                                VarType::Elementary(t) => t.name().to_string(),
                                VarType::FunctionBlock(kind) => kind.name().to_string(),
                                VarType::UserFb(_) => v.type_name.clone(),
                            },
                        })
                        .collect();
                    parsed.push((program.name.clone(), ast));
                    index.push(i);
                }
                Err(e) => errors[i].push(e),
            }
        }
        let (runtime, mapping) = match Runtime::build(parsed, fbs, mappings) {
            Ok(runtime) => (Some(runtime), StErrors::new()),
            Err(semantic) => {
                for (k, e) in semantic.programs.into_iter().enumerate() {
                    errors[index[k]].extend(e);
                }
                for (k, e) in semantic.function_blocks.into_iter().enumerate() {
                    errors[fb_file[k]].extend(e);
                }
                (None, semantic.mapping)
            }
        };
        let ok = errors.iter().all(Vec::is_empty);
        let programs = programs
            .iter()
            .zip(errors)
            .zip(lines)
            .zip(variables)
            .zip(defined)
            .zip(has_program)
            .map(|(((((p, errors), lines), variables), function_blocks), has_program)| CompileResult {
                name: p.name.clone(),
                errors,
                lines,
                variables,
                function_blocks,
                has_program,
            })
            .collect();
        (runtime.filter(|_| ok), CompileReport { programs, mapping })
    };
    let failed = |message: &str| CompileReport::failed(programs, message);
    thread::scope(|scope| {
        let loader = thread::Builder::new()
            .name("plc-load".into())
            .stack_size(ENGINE_STACK_BYTES)
            .spawn_scoped(scope, compile)
            .map_err(|_| (None, failed("Could not start the PLC engine.")));
        match loader {
            Ok(loader) => loader.join().unwrap_or_else(|_| (None, failed("The PLC engine failed while loading the program."))),
            Err(failure) => failure,
        }
    })
}

/// Compile every program (lexer → parser → semantic analysis) and check the I/O mappings,
/// without running anything. Touches no simulator state.
pub fn compile(programs: &[ProgramSource], mappings: &[Mapping]) -> CompileReport {
    load(programs, mappings).1
}

fn breakpoint_pairs(breakpoints: &[Breakpoint]) -> Vec<(String, usize)> {
    breakpoints.iter().map(|b| (b.program.clone(), b.line)).collect()
}

// A poisoned lock only means some earlier holder panicked; the data is still usable,
// and the IDE must keep working rather than propagate the panic.
fn lock(inner: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    inner.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Simulated PLC: owns the runtime and drives it with a background scan cycle.
pub struct Simulator {
    inner: Arc<Mutex<Inner>>,
}

impl Default for Simulator {
    fn default() -> Self {
        Self::new()
    }
}

impl Simulator {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                runtime: None,
                io: IoImage::new(&io::CONFIG),
                status: RuntimeStatus::Stopped,
                cycle_count: 0,
                scan_time_ms: DEFAULT_SCAN_MS,
                error: None,
                error_program: None,
                breakpoints: Vec::new(),
                generation: 0,
                clock_ms: 0,
            })),
        }
    }

    /// Compile every program and check the I/O mappings; if all is well, start scanning
    /// (or load paused, ready for STEP). With any error nothing runs and the full report
    /// is returned. Simulated inputs keep their values; outputs start from the safe state.
    pub fn start(
        &self,
        programs: &[ProgramSource],
        mappings: &[Mapping],
        scan_time_ms: Option<u64>,
        paused: bool,
    ) -> Result<RuntimeState, CompileReport> {
        let loaded = load(programs, mappings);

        let mut inner = lock(&self.inner);
        inner.generation += 1;
        inner.cycle_count = 0;
        inner.clock_ms = 0;
        inner.status = RuntimeStatus::Stopped;
        inner.error = None; // compile errors are returned, not shown as a scan failure
        inner.error_program = None;
        inner.io.reset_outputs();
        let mut runtime = match loaded {
            (Some(runtime), _) => runtime,
            (None, report) => {
                inner.runtime = None;
                return Err(report);
            }
        };
        runtime.set_breakpoints(&breakpoint_pairs(&inner.breakpoints));
        inner.runtime = Some(runtime);
        inner.scan_time_ms = scan_time_ms.unwrap_or(DEFAULT_SCAN_MS).clamp(MIN_SCAN_MS, MAX_SCAN_MS);
        inner.status = if paused { RuntimeStatus::Paused } else { RuntimeStatus::Running };
        let generation = inner.generation;
        let state = inner.snapshot();
        drop(inner);

        if let Err(err) = self.spawn_scan_loop(generation) {
            let mut inner = lock(&self.inner);
            inner.status = RuntimeStatus::Stopped;
            inner.error = Some(err.clone());
            return Err(CompileReport::failed(programs, &err.message));
        }
        Ok(state)
    }

    fn spawn_scan_loop(&self, generation: u64) -> StResult<()> {
        let inner = Arc::clone(&self.inner);
        let scan_loop = move || loop {
            // One scan = READ INPUTS → EXECUTE → UPDATE OUTPUTS. Inputs (set_input) and
            // outputs (get_runtime_state) share the runtime's variable store, and the
            // execute step holds the lock, so each scan sees a consistent input image.
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
        thread::Builder::new()
            .name("plc-scan".into())
            .stack_size(ENGINE_STACK_BYTES)
            .spawn(scan_loop)
            .map(|_| ())
            .map_err(|_| StError::general("Could not start the PLC scan cycle."))
    }

    /// Stop scanning and reset variables to their initial values and outputs to their safe
    /// state. Simulated inputs are kept. The loaded programs stay, so the monitor still
    /// lists their variables.
    pub fn stop(&self) -> RuntimeState {
        let mut inner = lock(&self.inner);
        inner.generation += 1;
        inner.status = RuntimeStatus::Stopped;
        inner.cycle_count = 0;
        inner.clock_ms = 0;
        inner.error = None;
        inner.error_program = None;
        inner.io.reset_outputs();
        if let Some(runtime) = inner.runtime.as_mut() {
            runtime.reset();
        }
        inner.snapshot()
    }

    /// Suspend scanning, keeping variables and cycle count.
    pub fn pause(&self) -> StResult<RuntimeState> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Running {
            return Err(StError::general("The program is not running."));
        }
        inner.status = RuntimeStatus::Paused;
        Ok(inner.snapshot())
    }

    pub fn resume(&self) -> StResult<RuntimeState> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Paused {
            return Err(StError::general("The program is not paused."));
        }
        inner.status = RuntimeStatus::Running;
        Ok(inner.snapshot())
    }

    /// STEP SCAN: finish the scan the debugger stopped (or run one complete scan), then
    /// stay paused. Breakpoints don't stop it: it always ends with a complete scan.
    pub fn step(&self) -> StResult<RuntimeState> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Paused {
            return Err(StError::general("STEP runs one scan while the PLC is paused."));
        }
        inner.scan(Stop::Never);
        Ok(inner.snapshot())
    }

    /// STEP STATEMENT: run the statement the debugger stopped at and stop before the next
    /// one, which may be in the next program. After the scan's last statement, the outputs
    /// are written and the next scan starts, stopping at its first statement.
    pub fn step_statement(&self) -> StResult<RuntimeState> {
        let mut inner = lock(&self.inner);
        if inner.status != RuntimeStatus::Paused {
            return Err(StError::general("STEP STATEMENT runs one statement while the PLC is paused."));
        }
        let still_scanning = inner.scan(Stop::NextStatement);
        if still_scanning && inner.runtime.as_ref().is_some_and(|rt| rt.location().is_none()) {
            inner.scan(Stop::NextStatement);
        }
        Ok(inner.snapshot())
    }

    /// Replace the breakpoints. They apply right away (and to later runs).
    pub fn set_breakpoints(&self, breakpoints: Vec<Breakpoint>) -> RuntimeState {
        let mut inner = lock(&self.inner);
        let pairs = breakpoint_pairs(&breakpoints);
        inner.breakpoints = breakpoints;
        if let Some(runtime) = inner.runtime.as_mut() {
            runtime.set_breakpoints(&pairs);
        }
        inner.snapshot()
    }

    /// Write a BOOL variable declared by `program`; the next scan reads the new value.
    pub fn set_input(&self, program: &str, name: &str, value: bool) -> StResult<RuntimeState> {
        let mut inner = lock(&self.inner);
        let runtime = inner
            .runtime
            .as_mut()
            .ok_or_else(|| StError::general("Press RUN to load the program before changing inputs."))?;
        runtime.set_in(program, name, Value::Bool(value))?;
        Ok(inner.snapshot())
    }

    /// Change the I/O mappings of the loaded programs right away (from the next scan).
    /// Returns the mapping errors (nothing changes then). Without loaded programs there is
    /// nothing to apply: the mappings are checked by the next compile or RUN.
    pub fn apply_mappings(&self, mappings: &[Mapping]) -> StErrors {
        let mut inner = lock(&self.inner);
        match inner.runtime.as_mut() {
            Some(runtime) => runtime.set_mappings(mappings),
            None => StErrors::new(),
        }
    }

    /// Set a simulated input (DI: ON/OFF, AI: a number), whether or not the PLC runs; the
    /// next scan reads it. Outputs are refused: only the PLC writes them.
    pub fn set_io_input(&self, address: &str, value: InputValue) -> StResult<RuntimeState> {
        let mut inner = lock(&self.inner);
        inner.io.set_input(address, value).map_err(StError::general)?;
        Ok(inner.snapshot())
    }

    pub fn state(&self) -> RuntimeState {
        lock(&self.inner).snapshot()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::st::error::ErrorKind;
    use crate::st::parser::parse;
    use crate::st::runtime::Location;

    fn sources(programs: &[(&str, &str)]) -> Vec<ProgramSource> {
        programs.iter().map(|(name, source)| ProgramSource { name: name.to_string(), source: source.to_string() }).collect()
    }

    /// Single-program helpers: the program is named after its PROGRAM header.
    impl Simulator {
        fn start_one(&self, source: &str, scan_time_ms: Option<u64>) -> Result<RuntimeState, StErrors> {
            let name = source.split_whitespace().nth(1).unwrap_or("main");
            self.start(&sources(&[(name, source)]), &[], scan_time_ms, false)
                .map_err(|report| report.programs.into_iter().flat_map(|r| r.errors).collect())
        }

        fn set(&self, name: &str, value: bool) -> StResult<RuntimeState> {
            let program = self.state().programs.first().cloned().unwrap_or_default();
            self.set_input(&program, name, value)
        }
    }

    fn check(source: &str) -> StErrors {
        compile(&sources(&[("main", source)]), &[]).programs.remove(0).errors
    }

    const MOTOR: &str = "PROGRAM main
VAR
    Start : BOOL := FALSE;
    Motor : BOOL := FALSE;
END_VAR

IF Start THEN
    Motor := TRUE;
ELSE
    Motor := FALSE;
END_IF;

END_PROGRAM";

    fn value(state: &RuntimeState, name: &str) -> bool {
        state.variables.iter().find(|v| v.name == name).unwrap().value.as_bool().unwrap()
    }

    /// Poll until `done` holds; the scan thread runs on its own clock.
    fn wait_until(sim: &Simulator, done: impl Fn(&RuntimeState) -> bool) -> RuntimeState {
        for _ in 0..400 {
            let state = sim.state();
            if done(&state) {
                return state;
            }
            thread::sleep(Duration::from_millis(5));
        }
        panic!("timed out waiting for simulator: {:?}", sim.state());
    }

    /// Wait for at least one complete scan after the current one.
    fn next_scan(sim: &Simulator) -> RuntimeState {
        let now = sim.state().cycle_count;
        wait_until(sim, |s| s.cycle_count > now + 1)
    }

    #[test]
    fn scan_cycle_runs_motor_program() {
        let sim = Simulator::new();
        let started = sim.start_one(MOTOR, Some(10)).unwrap();
        assert_eq!(started.status, RuntimeStatus::Running);
        assert!(!value(&started, "Start"));
        assert!(!value(&started, "Motor"));

        let state = wait_until(&sim, |s| s.cycle_count >= 3);
        assert!(!value(&state, "Motor"));

        sim.set("Start", true).unwrap();
        let state = next_scan(&sim);
        assert!(value(&state, "Start"));
        assert!(value(&state, "Motor"));

        sim.set("Start", false).unwrap();
        let state = next_scan(&sim);
        assert!(!value(&state, "Motor"));

        let stopped = sim.stop();
        assert_eq!(stopped.status, RuntimeStatus::Stopped);
        assert_eq!(stopped.cycle_count, 0);
    }

    #[test]
    fn pause_preserves_state_and_resume_continues() {
        let sim = Simulator::new();
        sim.start_one(MOTOR, Some(10)).unwrap();
        wait_until(&sim, |s| s.cycle_count >= 2);

        let paused = sim.pause().unwrap();
        assert_eq!(paused.status, RuntimeStatus::Paused);
        thread::sleep(Duration::from_millis(60));
        let still = sim.state();
        assert_eq!(still.cycle_count, paused.cycle_count, "no scans while paused");

        sim.resume().unwrap();
        wait_until(&sim, |s| s.cycle_count > paused.cycle_count);
        sim.stop();
    }

    #[test]
    fn semantic_errors_block_execution_and_are_all_returned() {
        let source = "PROGRAM main
VAR Counter : INT; Motor : BOOL; END_VAR
Unknown := 10;
Counter := TRUE;
Motor := 100;
END_PROGRAM";
        let sim = Simulator::new();
        let errors = sim.start_one(source, Some(10)).unwrap_err();
        assert_eq!(errors.len(), 3);
        thread::sleep(Duration::from_millis(50));
        let state = sim.state();
        assert_eq!((state.status, state.cycle_count), (RuntimeStatus::Stopped, 0), "the runtime never started");
        assert!(state.variables.is_empty());

        // The editor's live check reports the same errors and touches no simulator state.
        assert_eq!(check(source), errors);
        assert!(check(MOTOR).is_empty());
        assert_eq!(sim.state().status, RuntimeStatus::Stopped);
    }

    #[test]
    fn invalid_program_does_not_start() {
        let sim = Simulator::new();
        let err = sim.start_one("PROGRAM main\nIF TRUE THEN\nEND_PROGRAM", None).unwrap_err();
        assert_eq!(err.len(), 1, "a syntax error stops at the first problem");
        assert!(err[0].message.starts_with("Expected END_IF"));
        let state = sim.state();
        assert_eq!(state.status, RuntimeStatus::Stopped);
        assert!(state.variables.is_empty());
        assert!(sim.set("Start", true).is_err());
    }

    #[test]
    fn pause_and_resume_require_matching_status() {
        let sim = Simulator::new();
        assert!(sim.pause().is_err());
        assert!(sim.resume().is_err());
    }

    #[test]
    fn restart_uses_new_program_and_default_scan_time() {
        let sim = Simulator::new();
        sim.start_one(MOTOR, Some(10)).unwrap();
        let restarted = sim.start_one("PROGRAM other VAR x : BOOL; END_VAR END_PROGRAM", None).unwrap();
        assert_eq!(restarted.programs, ["other"]);
        assert_eq!(restarted.scan_time_ms, DEFAULT_SCAN_MS);
        sim.stop();
    }

    fn get(state: &RuntimeState, name: &str) -> Value {
        state.variables.iter().find(|v| v.name == name).unwrap().value
    }

    #[test]
    fn numeric_program_runs_in_scan_cycle() {
        let sim = Simulator::new();
        let started = sim
            .start_one(
                "PROGRAM main
                 VAR Counter : INT := 10; Speed : INT := 20; Result : INT := 0; Temperature : REAL := 24.5; END_VAR
                 Result := Counter + Speed;
                 END_PROGRAM",
                Some(10),
            )
            .unwrap();
        assert_eq!(get(&started, "Result"), Value::Int(0), "before the first scan");
        assert_eq!(get(&started, "Temperature"), Value::Real(24.5));

        let state = wait_until(&sim, |s| s.cycle_count >= 1);
        assert_eq!(get(&state, "Counter"), Value::Int(10));
        assert_eq!(get(&state, "Speed"), Value::Int(20));
        assert_eq!(get(&state, "Result"), Value::Int(30));
        sim.stop();
    }

    #[test]
    fn counter_advances_once_per_cycle() {
        let sim = Simulator::new();
        sim.start_one("PROGRAM p VAR Counter : INT := 10; END_VAR Counter := Counter + 1; END_PROGRAM", Some(10))
            .unwrap();
        let state = wait_until(&sim, |s| s.cycle_count >= 5);
        sim.pause().unwrap();
        let state_after = sim.state();
        // Read under one snapshot: Counter = 10 + number of completed scans.
        assert_eq!(get(&state_after, "Counter"), Value::Int(10 + state_after.cycle_count as i16));
        assert!(state_after.cycle_count >= state.cycle_count);
        sim.stop();
    }

    #[test]
    fn runtime_error_stops_the_plc_with_position() {
        let sim = Simulator::new();
        sim.start_one("PROGRAM p VAR a : INT := 5; b : INT; r : INT; END_VAR\nr := a / b;\nEND_PROGRAM", Some(10))
            .unwrap();
        let state = wait_until(&sim, |s| s.status == RuntimeStatus::Stopped);
        let err = state.error.expect("division by zero should be reported");
        assert_eq!((err.line, err.column, err.message.as_str()), (2, 8, "Division by zero"));
    }

    #[test]
    fn set_input_rejects_wrong_type() {
        let sim = Simulator::new();
        sim.start_one("PROGRAM p VAR Counter : INT; END_VAR END_PROGRAM", Some(10)).unwrap();
        let err = sim.set("Counter", true).unwrap_err();
        assert!(err.message.contains("'Counter' is INT"));
        sim.stop();
    }

    fn member(state: &RuntimeState, block: &str, member: &str) -> Value {
        let fb = state.function_blocks.iter().find(|b| b.name == block).unwrap();
        fb.members.iter().find(|m| m.name == member).unwrap().value
    }

    #[test]
    fn timers_advance_one_scan_interval_per_scan() {
        let sim = Simulator::new();
        sim.start_one(
            "PROGRAM p VAR t : TON; done : BOOL; END_VAR t(IN := TRUE, PT := T#150ms); done := t.Q; END_PROGRAM",
            Some(10),
        )
        .unwrap();
        // Every snapshot is consistent: ET is exactly 10 ms per completed scan, capped at PT.
        for _ in 0..30 {
            let state = sim.state();
            let expected = (state.cycle_count as i64 * 10).min(150);
            assert_eq!(member(&state, "t", "ET"), Value::Time(expected), "after {} scans", state.cycle_count);
            assert_eq!(value(&state, "done"), state.cycle_count >= 15);
            thread::sleep(Duration::from_millis(3));
        }
        sim.stop();
    }

    #[test]
    fn pause_freezes_timers() {
        let sim = Simulator::new();
        sim.start_one("PROGRAM p VAR t : TON; END_VAR t(IN := TRUE, PT := T#1S); END_PROGRAM", Some(10)).unwrap();
        wait_until(&sim, |s| s.cycle_count >= 3);
        let paused = sim.pause().unwrap();
        thread::sleep(Duration::from_millis(100));
        let later = sim.state();
        assert_eq!(member(&later, "t", "ET"), member(&paused, "t", "ET"), "no scans, no time");
        assert_eq!(member(&later, "t", "ET"), Value::Time(paused.cycle_count as i64 * 10));
        sim.stop();
    }

    #[test]
    fn stop_resets_function_blocks() {
        let sim = Simulator::new();
        sim.start_one("PROGRAM p VAR c : CTU; b : BOOL; END_VAR c(CU := b); END_PROGRAM", Some(10)).unwrap();
        sim.set("b", true).unwrap();
        wait_until(&sim, |s| member(s, "c", "CV") == Value::Int(1));
        let stopped = sim.stop();
        assert_eq!(member(&stopped, "c", "CV"), Value::Int(0));
    }

    /// The deepest programs the parser accepts, one per recursion shape.
    fn deepest_programs() -> Vec<String> {
        use crate::st::parser::MAX_NESTING;
        let shapes: [fn(usize) -> String; 8] = [
            |n| format!("x := {}TRUE{};", "(".repeat(n), ")".repeat(n)),
            |n| format!("x := {}TRUE;", "NOT ".repeat(n)),
            |n| format!("i := {}1;", "- ".repeat(n)),
            |n| format!("i := 1{};", " + 1".repeat(n)),
            |n| format!("{} x := TRUE; {}", "IF x THEN ".repeat(n), "END_IF; ".repeat(n)),
            // These execute every level, so the runtime recursion is as deep as the parse.
            |n| format!("{} x := TRUE; {}", "REPEAT ".repeat(n), "UNTIL TRUE END_REPEAT; ".repeat(n)),
            |n| format!("{} x := TRUE; {}", "CASE i OF 0: ".repeat(n), "END_CASE; ".repeat(n)),
            |n| format!("{} x := TRUE; {}", "WHILE x DO ".repeat(n), "END_WHILE; ".repeat(n)),
        ];
        shapes
            .iter()
            .map(|shape| {
                (1..=MAX_NESTING)
                    .rev()
                    .map(|n| format!("PROGRAM p VAR x : BOOL; i : DINT; END_VAR {} END_PROGRAM", shape(n)))
                    .find(|src| parse(src).is_ok())
                    .expect("some depth must be accepted")
            })
            .collect()
    }

    #[test]
    fn deepest_programs_run_when_started_from_a_1mb_main_thread() {
        // Finding the deepest accepted depth parses, so do it on an engine-sized stack.
        let programs = thread::Builder::new()
            .stack_size(ENGINE_STACK_BYTES)
            .spawn(deepest_programs)
            .unwrap()
            .join()
            .unwrap();
        // Stands in for Tauri calling start_program on the Windows main thread; only
        // `start` runs here, exactly as in production.
        thread::Builder::new()
            .stack_size(1024 * 1024)
            .spawn(move || {
                for src in programs {
                    let sim = Simulator::new();
                    sim.start_one(&src, Some(10)).unwrap();
                    let state = wait_until(&sim, |s| s.cycle_count >= 1 || s.error.is_some());
                    assert!(state.error.is_none(), "{:?}", state.error);
                    sim.stop();
                }
            })
            .unwrap()
            .join()
            .expect("deep programs must not overflow the stack");
    }

    #[test]
    fn engine_stack_has_4x_headroom() {
        thread::Builder::new()
            .stack_size(ENGINE_STACK_BYTES / 4)
            .spawn(|| {
                for src in deepest_programs() {
                    let mut runtime = Runtime::new(parse(&src).unwrap()).unwrap();
                    runtime.scan(ScanClock { now_ms: 0, cycle_ms: 0 }).unwrap();
                }
            })
            .unwrap()
            .join()
            .expect("deepest programs must fit in a quarter of the engine stack");
    }

    // --- multi-program projects ---

    const MAIN: &str = "PROGRAM Main\nVAR\n    Start : BOOL := FALSE;\nEND_VAR\nEND_PROGRAM";
    const MOTOR_CONTROL: &str = "PROGRAM MotorControl\nVAR\n    Motor : BOOL := FALSE;\nEND_VAR\nIF Start THEN\n    Motor := TRUE;\nELSE\n    Motor := FALSE;\nEND_IF;\nEND_PROGRAM";
    const ALARM_OK: &str = "PROGRAM AlarmLogic\nVAR\n    Alarm : BOOL := FALSE;\nEND_VAR\nAlarm := FALSE;\nEND_PROGRAM";
    const ALARM_BAD: &str = "PROGRAM AlarmLogic\nVAR\n    Alarm : BOOL := FALSE;\nEND_VAR\nUnknownVariable := TRUE;\nEND_PROGRAM";
    const TANK: &str = "PROGRAM TankControl\nVAR\n    TankLevel : REAL := 50.0;\nEND_VAR\nTankLevel := TankLevel + 0.1;\nEND_PROGRAM";

    fn spec_project(alarm: &str) -> Vec<ProgramSource> {
        sources(&[("Main", MAIN), ("MotorControl", MOTOR_CONTROL), ("AlarmLogic", alarm), ("TankControl", TANK)])
    }

    fn error_counts(results: &[CompileResult]) -> Vec<(&str, usize)> {
        results.iter().map(|r| (r.name.as_str(), r.errors.len())).collect()
    }

    #[test]
    fn compile_all_reports_each_program_and_blocks_the_runtime() {
        let broken = spec_project(ALARM_BAD);
        let expected = [("Main", 0), ("MotorControl", 0), ("AlarmLogic", 1), ("TankControl", 0)];
        assert_eq!(error_counts(&compile(&broken, &[]).programs), expected);

        let sim = Simulator::new();
        let results = sim.start(&broken, &[], Some(10), false).unwrap_err().programs;
        assert_eq!(error_counts(&results), expected);
        assert_eq!(results[2].errors[0].message, "Undeclared variable 'UnknownVariable'");
        thread::sleep(Duration::from_millis(40));
        let state = sim.state();
        assert_eq!((state.status, state.cycle_count), (RuntimeStatus::Stopped, 0), "runtime must not start");
        assert!(state.programs.is_empty() && state.variables.is_empty());

        // Fixed: every program compiles and the PLC runs all four.
        let fixed = spec_project(ALARM_OK);
        assert!(compile(&fixed, &[]).programs.iter().all(|r| r.errors.is_empty()));
        let started = sim.start(&fixed, &[], Some(10), false).unwrap();
        assert_eq!(started.programs, ["Main", "MotorControl", "AlarmLogic", "TankControl"]);
        sim.set_input("Main", "Start", true).unwrap();
        let state = next_scan(&sim);
        assert!(value(&state, "Motor"), "MotorControl reads Main's Start");
        sim.stop();
    }

    #[test]
    fn compile_all_keeps_going_past_syntax_errors_and_duplicate_names() {
        let results = compile(
            &sources(&[
                ("A", "PROGRAM A\nIF TRUE THEN\nEND_PROGRAM"),
                ("B", "PROGRAM B VAR x : INT; END_VAR x := TRUE; y := 1; END_PROGRAM"),
                ("b", "PROGRAM b END_PROGRAM"),
            ]),
            &[],
        )
        .programs;
        assert_eq!(error_counts(&results), [("A", 1), ("B", 2), ("b", 1)]);
        assert!(results[0].errors[0].message.starts_with("Expected END_IF"));
        assert_eq!(results[2].errors[0].message, "Program 'b' already exists.");
    }

    #[test]
    fn step_executes_one_complete_scan_of_every_program() {
        let sim = Simulator::new();
        let programs = sources(&[
            ("ProgramA", "PROGRAM ProgramA VAR Seq : DINT; Scans : DINT; END_VAR Seq := 1; Scans := Scans + 1; END_PROGRAM"),
            ("ProgramB", "PROGRAM ProgramB VAR END_VAR Seq := Seq * 10 + 2; END_PROGRAM"),
            ("ProgramC", "PROGRAM ProgramC VAR END_VAR Seq := Seq * 10 + 3; END_PROGRAM"),
        ]);
        let loaded = sim.start(&programs, &[], Some(10), true).unwrap();
        assert_eq!((loaded.status, loaded.cycle_count), (RuntimeStatus::Paused, 0), "loaded paused, nothing ran");
        thread::sleep(Duration::from_millis(40));
        assert_eq!(sim.state().cycle_count, 0);

        for n in 1..=3 {
            let state = sim.step().unwrap();
            assert_eq!(state.status, RuntimeStatus::Paused);
            assert_eq!(state.cycle_count, n);
            assert_eq!(get(&state, "Seq"), Value::DInt(123), "A → B → C, each exactly once");
            assert_eq!(get(&state, "Scans"), Value::DInt(n as i32));
        }
        sim.resume().unwrap();
        assert!(sim.step().is_err(), "STEP only while paused");
        sim.stop();
        assert!(sim.step().is_err());
    }

    #[test]
    fn runtime_error_names_the_failing_program() {
        let sim = Simulator::new();
        sim.start(
            &sources(&[("Ok", "PROGRAM Ok VAR d : INT; END_VAR END_PROGRAM"), ("Bad", "PROGRAM Bad VAR r : INT; END_VAR\nr := 1 / d;\nEND_PROGRAM")]),
            &[],
            Some(10),
            false,
        )
        .unwrap();
        let state = wait_until(&sim, |s| s.status == RuntimeStatus::Stopped);
        assert_eq!(state.error_program.as_deref(), Some("Bad"));
        assert_eq!(state.error.unwrap().line, 2);
        assert_eq!(sim.stop().error_program, None);
    }

    // --- simulated I/O ---

    fn map(pairs: &[(&str, &str)]) -> Vec<Mapping> {
        pairs.iter().map(|(variable, address)| Mapping { address: address.to_string(), variable: variable.to_string() }).collect()
    }

    fn io(state: &RuntimeState, address: &str) -> Value {
        state.io.iter().find(|p| p.address == address).unwrap().value
    }

    fn set_di(sim: &Simulator, address: &str, on: bool) {
        sim.set_io_input(address, InputValue::Bool(on)).unwrap();
    }

    fn set_ai(sim: &Simulator, address: &str, x: f64) {
        sim.set_io_input(address, InputValue::Number(x)).unwrap();
    }

    const MOTOR_IO: &str = "PROGRAM Main
VAR
    StartButton : BOOL;
    StopButton : BOOL;
    Motor : BOOL;
END_VAR

IF StartButton AND NOT StopButton THEN
    Motor := TRUE;
END_IF;

IF StopButton THEN
    Motor := FALSE;
END_IF;

END_PROGRAM";

    const TANK_IO: &str = "PROGRAM TankControl
VAR
    TankLevel : REAL;
    Setpoint : REAL := 80.0;
    FillValve : REAL;
END_VAR

TankLevel := AI0;

IF TankLevel < Setpoint THEN
    FillValve := 100.0;
ELSE
    FillValve := 0.0;
END_IF;

AO0 := FillValve;

END_PROGRAM";

    #[test]
    fn spec_motor_control_di_to_do() {
        let sim = Simulator::new();
        let mappings = map(&[("StartButton", "DI0"), ("StopButton", "DI1"), ("Motor", "DO0")]);
        let loaded = sim.start(&sources(&[("Main", MOTOR_IO)]), &mappings, Some(10), true).unwrap();
        assert_eq!((io(&loaded, "DI0"), io(&loaded, "DI1"), io(&loaded, "DO0")), (Value::Bool(false), Value::Bool(false), Value::Bool(false)));

        set_di(&sim, "DI0", true);
        assert_eq!(io(&sim.state(), "DO0"), Value::Bool(false), "inputs are read at the start of the next scan");
        let state = sim.step().unwrap();
        assert_eq!(io(&state, "DO0"), Value::Bool(true));
        let motor = state.variables.iter().find(|v| v.name == "Motor").unwrap();
        assert_eq!((motor.value, motor.io.as_deref()), (Value::Bool(true), Some("DO0")));

        set_di(&sim, "DI1", true);
        assert_eq!(io(&sim.step().unwrap(), "DO0"), Value::Bool(false));
        sim.stop();
    }

    #[test]
    fn spec_tank_level_ai_to_ao() {
        let sim = Simulator::new();
        let mappings = map(&[("TankLevel", "AI0"), ("FillValve", "AO0")]);
        sim.start(&sources(&[("TankControl", TANK_IO)]), &mappings, Some(10), true).unwrap();
        for (level, valve) in [(20.0, 100.0), (80.0, 0.0), (90.0, 0.0), (79.5, 100.0)] {
            set_ai(&sim, "AI0", level);
            let state = sim.step().unwrap();
            assert_eq!(io(&state, "AO0"), Value::Real(valve), "AI0 = {level}");
            assert_eq!(get(&state, "TankLevel"), Value::Real(level));
        }
        sim.stop();
    }

    #[test]
    fn complete_io_flow_across_programs_while_running() {
        let sim = Simulator::new();
        let programs = sources(&[
            ("Main", "PROGRAM Main VAR Start : BOOL; Stop : BOOL; Level : REAL; END_VAR END_PROGRAM"),
            (
                "Plant",
                "PROGRAM Plant VAR Motor : BOOL; Alarm : BOOL; Setpoint : REAL := 80.0; END_VAR
                 IF Start AND NOT Stop THEN Motor := TRUE; ELSIF Stop THEN Motor := FALSE; END_IF;
                 Alarm := Level > 95.0;
                 DO1 := Alarm;
                 END_PROGRAM",
            ),
        ]);
        let mappings = map(&[("Start", "DI0"), ("Stop", "DI1"), ("Level", "AI0"), ("Motor", "DO0"), ("Setpoint", "AO0")]);
        sim.start(&programs, &mappings, Some(10), false).unwrap();
        let state = wait_until(&sim, |s| s.cycle_count >= 1);
        assert_eq!(io(&state, "AO0"), Value::Real(80.0));
        assert_eq!(io(&state, "DO0"), Value::Bool(false));

        set_di(&sim, "DI0", true);
        wait_until(&sim, |s| io(s, "DO0") == Value::Bool(true));
        set_ai(&sim, "AI0", 97.0);
        wait_until(&sim, |s| io(s, "DO1") == Value::Bool(true));
        set_di(&sim, "DI1", true);
        let state = wait_until(&sim, |s| io(s, "DO0") == Value::Bool(false));
        assert_eq!(io(&state, "DO1"), Value::Bool(true), "still over 95 %");

        // STOP: programs stop, outputs go safe, the inputs the user set stay.
        let stopped = sim.stop();
        assert_eq!((io(&stopped, "DO1"), io(&stopped, "AO0")), (Value::Bool(false), Value::Real(0.0)));
        assert_eq!((io(&stopped, "DI0"), io(&stopped, "AI0")), (Value::Bool(true), Value::Real(97.0)));
        thread::sleep(Duration::from_millis(40));
        assert_eq!(io(&sim.state(), "DO1"), Value::Bool(false), "no scans while stopped");
        set_di(&sim, "DI1", false); // inputs can still be changed while stopped

        // RUN again: the first scan reads the current inputs (DI0 still ON).
        sim.start(&programs, &mappings, Some(10), false).unwrap();
        wait_until(&sim, |s| io(s, "DO0") == Value::Bool(true));
        sim.stop();
    }

    #[test]
    fn step_runs_one_complete_io_scan() {
        let sim = Simulator::new();
        let programs = sources(&[
            ("Main", "PROGRAM Main VAR Count : INT; END_VAR IF DI0 THEN Count := Count + 1; END_IF; END_PROGRAM"),
            ("Out", "PROGRAM Out VAR END_VAR AO0 := AI1; DO0 := DI0; END_PROGRAM"),
        ]);
        sim.start(&programs, &[], Some(10), true).unwrap();
        set_di(&sim, "DI0", true);
        set_ai(&sim, "AI1", 12.5);
        let state = sim.step().unwrap();
        assert_eq!(state.cycle_count, 1);
        assert_eq!(get(&state, "Count"), Value::Int(1), "Main saw DI0 in this scan");
        assert_eq!((io(&state, "DO0"), io(&state, "AO0")), (Value::Bool(true), Value::Real(12.5)), "outputs written after the scan");
        assert_eq!(get(&sim.step().unwrap(), "Count"), Value::Int(2), "one STEP, one scan");
        sim.stop();
    }

    #[test]
    fn outputs_belong_to_the_plc() {
        let sim = Simulator::new();
        set_di(&sim, "DI2", true); // no program loaded: inputs still work
        assert_eq!(io(&sim.state(), "DI2"), Value::Bool(true));
        let err = sim.set_io_input("DO0", InputValue::Bool(true)).unwrap_err();
        assert_eq!(err.message, "DO0 is an output: only the PLC program drives it.");
        assert!(sim.set_io_input("AO0", InputValue::Number(1.0)).is_err());
    }

    #[test]
    fn invalid_mappings_and_io_misuse_are_reported_and_block_run() {
        let programs = sources(&[("Main", "PROGRAM Main VAR Level : REAL; On : BOOL; T : TON; Count : INT; END_VAR END_PROGRAM")]);
        let report = compile(
            &programs,
            &map(&[
                ("Level", "DI0"),
                ("Count", "AI0"),
                ("T", "DO1"),
                ("Ghost", "DO2"),
                ("On", "XX7"),
                ("On", "DO3"),
                ("Level", "AO1"),
                ("Level", "AO1"),
                ("On", "DI2"),
                ("On", "DI3"),
            ]),
        );
        let messages: Vec<&str> = report.mapping.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "Cannot map DIGITAL INPUT DI0 to REAL variable 'Level'.",
                "Cannot map ANALOG INPUT AI0 to INT variable 'Count'.",
                "Cannot map DIGITAL OUTPUT DO1 to TON instance 'T'; map one of its inputs or outputs, e.g. T.Q.",
                "Cannot map DIGITAL OUTPUT DO2: Undeclared variable 'Ghost'.",
                "Unknown I/O address 'XX7' (mapped to 'On').",
                "AO1 is mapped more than once ('Level' and 'Level').",
                "'On' is mapped to two inputs, DI2 and DI3.",
            ]
        );
        assert!(report.mapping.iter().all(|e| e.kind == ErrorKind::InvalidIoMapping));
        assert!(report.programs[0].errors.is_empty(), "the program itself is fine");

        let sim = Simulator::new();
        assert!(sim.start(&programs, &map(&[("Level", "DI0")]), Some(10), false).is_err());
        assert_eq!(sim.state().status, RuntimeStatus::Stopped, "a bad mapping never runs");

        // In programs: inputs are read-only and addresses can't be redeclared.
        let report = compile(&sources(&[("Main", "PROGRAM Main VAR DO0 : BOOL; END_VAR\nDI0 := TRUE;\nAI0 := 1.0;\nDO1 := 5;\nEND_PROGRAM")]), &[]);
        let errors: Vec<(ErrorKind, usize, &str)> = report.programs[0].errors.iter().map(|e| (e.kind, e.line, e.message.as_str())).collect();
        assert_eq!(
            errors,
            [
                (ErrorKind::DuplicateDeclaration, 1, "'DO0' is a PLC I/O address and can't be declared as a variable; use it directly or map a variable to it"),
                (ErrorKind::InvalidAssignment, 2, "Can't assign to 'DI0': PLC inputs are read-only in programs"),
                (ErrorKind::InvalidAssignment, 3, "Can't assign to 'AI0': PLC inputs are read-only in programs"),
                (ErrorKind::TypeMismatch, 4, "Type mismatch: cannot assign integer literal to 'DO1' (BOOL)"),
            ]
        );
    }

    // --- debugger ---

    const DEBUG_PROGRAMS: [(&str, &str); 2] = [
        ("Main", "PROGRAM Main\nVAR\n    Counter : INT;\nEND_VAR\nCounter := Counter + 1;\nDO0 := TRUE;\nEND_PROGRAM"),
        ("MotorControl", "PROGRAM MotorControl\nVAR\n    Motor : BOOL;\nEND_VAR\nMotor := DI0;\nDO1 := Motor;\nEND_PROGRAM"),
    ];

    fn bp(program: &str, line: usize) -> Breakpoint {
        Breakpoint { program: program.to_string(), line }
    }

    fn at(state: &RuntimeState) -> Option<(&str, usize)> {
        state.location.as_ref().map(|l| (l.program.as_str(), l.line))
    }

    #[test]
    fn running_into_a_breakpoint_pauses_mid_scan_until_resume() {
        let sim = Simulator::new();
        sim.set_breakpoints(vec![bp("MotorControl", 6)]);
        set_di(&sim, "DI0", true);
        sim.start(&sources(&DEBUG_PROGRAMS), &[], Some(10), false).unwrap();
        let paused = wait_until(&sim, |s| s.status == RuntimeStatus::Paused);
        assert_eq!(at(&paused), Some(("MotorControl", 6)));
        assert_eq!(paused.cycle_count, 0, "the first scan isn't complete");
        assert_eq!(get(&paused, "Counter"), Value::Int(1), "Main already ran");
        assert_eq!(get(&paused, "Motor"), Value::Bool(true), "line 5 ran");
        assert_eq!(io(&paused, "DO0"), Value::Bool(false), "outputs are written only at the end of the scan");
        thread::sleep(Duration::from_millis(40));
        let still = sim.state();
        assert_eq!((still.status, at(&still), get(&still, "Counter")), (RuntimeStatus::Paused, Some(("MotorControl", 6)), Value::Int(1)));

        // Resume: the scan finishes (outputs written), and the next scan stops at the same line.
        sim.resume().unwrap();
        let again = wait_until(&sim, |s| s.status == RuntimeStatus::Paused && s.cycle_count == 1);
        assert_eq!(at(&again), Some(("MotorControl", 6)));
        assert_eq!(get(&again, "Counter"), Value::Int(2), "nothing was reset");
        assert_eq!((io(&again, "DO0"), io(&again, "DO1")), (Value::Bool(true), Value::Bool(true)));

        // Remove the breakpoint and resume: it runs freely.
        sim.set_breakpoints(Vec::new());
        sim.resume().unwrap();
        let free = wait_until(&sim, |s| s.cycle_count >= 4);
        assert_eq!((free.status, free.location.is_none()), (RuntimeStatus::Running, true));
        let stopped = sim.stop();
        assert!(stopped.location.is_none());
    }

    #[test]
    fn step_statement_and_step_scan_while_paused() {
        let sim = Simulator::new();
        sim.set_breakpoints(vec![bp("Main", 5)]);
        sim.start(&sources(&DEBUG_PROGRAMS), &[], Some(10), false).unwrap();
        wait_until(&sim, |s| at(s) == Some(("Main", 5)));

        let s1 = sim.step_statement().unwrap();
        assert_eq!((at(&s1), get(&s1, "Counter")), (Some(("Main", 6)), Value::Int(1)));
        let s2 = sim.step_statement().unwrap();
        assert_eq!(at(&s2), Some(("MotorControl", 5)), "into the next program, in order");
        sim.step_statement().unwrap();
        let s4 = sim.step_statement().unwrap();
        // That was the scan's last statement: outputs written, the next scan stops at its first statement.
        assert_eq!((s4.cycle_count, at(&s4), io(&s4, "DO0")), (1, Some(("Main", 5)), Value::Bool(true)));
        assert_eq!(get(&s4, "Counter"), Value::Int(1), "the new scan hasn't run line 5 yet");

        // STEP SCAN finishes the stopped scan without stopping at breakpoints.
        let scanned = sim.step().unwrap();
        assert_eq!((scanned.cycle_count, scanned.location.is_none(), get(&scanned, "Counter")), (2, true, Value::Int(2)));
        assert_eq!(scanned.status, RuntimeStatus::Paused);
        // Paused between scans, STEP STATEMENT starts a scan and stops at its first statement.
        assert_eq!(at(&sim.step_statement().unwrap()), Some(("Main", 5)));

        sim.stop();
        assert!(sim.step_statement().is_err(), "only while paused");
    }

    #[test]
    fn compile_reports_breakable_lines() {
        let report = compile(&sources(&DEBUG_PROGRAMS), &[]);
        assert_eq!(report.programs[0].lines, [5, 6]);
        let broken = compile(&sources(&[("Main", "PROGRAM Main\nIF TRUE THEN\nEND_PROGRAM")]), &[]);
        assert!(broken.programs[0].lines.is_empty());
    }

    #[test]
    fn mappings_can_name_the_program_and_apply_while_running() {
        let programs = sources(&[
            ("main", "PROGRAM main VAR Start : BOOL; Motor : BOOL; END_VAR Motor := Start; END_PROGRAM"),
            ("MotorControl", "PROGRAM MotorControl VAR Motor : BOOL; END_VAR Motor := NOT Start; END_PROGRAM"),
        ]);
        let report = compile(&programs, &map(&[("Motor", "DO0")]));
        assert_eq!(
            report.mapping[0].message,
            "Cannot map DIGITAL OUTPUT DO0: 'Motor' is declared in programs main, MotorControl; write main.Motor or MotorControl.Motor."
        );
        let report = compile(&programs, &map(&[("main.Nope", "DO0")]));
        assert_eq!(report.mapping[0].message, "Cannot map DIGITAL OUTPUT DO0: program 'main' has no variable 'Nope'.");

        let sim = Simulator::new();
        sim.start(&programs, &map(&[("Start", "DI0"), ("main.Motor", "DO0"), ("motorcontrol.motor", "DO1")]), Some(10), true).unwrap();
        set_di(&sim, "DI0", true);
        let state = sim.step().unwrap();
        assert_eq!((io(&state, "DO0"), io(&state, "DO1")), (Value::Bool(true), Value::Bool(false)), "each program's own Motor");

        // Re-map while loaded: applies from the next scan, no restart.
        assert!(sim.apply_mappings(&map(&[("Start", "DI0"), ("MotorControl.Motor", "DO0")])).is_empty());
        let state = sim.step().unwrap();
        assert_eq!((io(&state, "DO0"), io(&state, "DO1")), (Value::Bool(false), Value::Bool(false)), "DO1 no longer mapped: safe value");
        // A bad mapping is refused and the old one stays.
        assert_eq!(sim.apply_mappings(&map(&[("Motor", "DO2")])).len(), 1);
        set_di(&sim, "DI0", false);
        assert_eq!(io(&sim.step().unwrap(), "DO0"), Value::Bool(true), "MotorControl.Motor = NOT Start, still on DO0");
        sim.stop();
    }

    // --- user-defined function blocks ---

    const MOTOR_FB: &str = "FUNCTION_BLOCK MotorControl

VAR_INPUT
    Start : BOOL;
    Stop : BOOL;
END_VAR

VAR_OUTPUT
    Motor : BOOL;
END_VAR

IF Start AND NOT Stop THEN
    Motor := TRUE;
END_IF;

IF Stop THEN
    Motor := FALSE;
END_IF;

END_FUNCTION_BLOCK";

    const COUNTER_FB: &str = "FUNCTION_BLOCK CounterFB
VAR_INPUT
    Enable : BOOL;
END_VAR
VAR_OUTPUT
    Count : INT;
END_VAR
IF Enable THEN
    Count := Count + 1;
END_IF;
END_FUNCTION_BLOCK";

    fn fb_member(state: &RuntimeState, instance: &str, member: &str) -> Value {
        let fb = state.function_blocks.iter().find(|b| b.name == instance).unwrap();
        fb.members.iter().find(|m| m.name == member).unwrap().value
    }

    fn file_errors(report: &CompileReport) -> Vec<(&str, Vec<(usize, &str)>)> {
        report.programs.iter().map(|r| (r.name.as_str(), r.errors.iter().map(|e| (e.line, e.message.as_str())).collect())).collect()
    }

    #[test]
    fn spec_motor_control_fb_with_io() {
        let sim = Simulator::new();
        let main = "PROGRAM Main
VAR
    Motor1 : MotorControl;
END_VAR
Motor1(
    Start := DI0,
    Stop := DI1
);
DO0 := Motor1.Motor;
END_PROGRAM";
        let report = compile(&sources(&[("MotorControl", MOTOR_FB), ("Main", main)]), &[]);
        assert!(report.programs.iter().all(|r| r.errors.is_empty()), "{:?}", file_errors(&report));
        let fb = &report.programs[0].function_blocks[0];
        assert_eq!(fb.name, "MotorControl");
        let names = |vars: &[DeclaredVariable]| vars.iter().map(|v| format!("{} {}", v.name, v.data_type)).collect::<Vec<_>>();
        assert_eq!((names(&fb.inputs), names(&fb.outputs)), (vec!["Start BOOL".to_string(), "Stop BOOL".into()], vec!["Motor BOOL".to_string()]));
        assert_eq!(standard_function_blocks().iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["TON", "TOF", "TP", "CTU", "CTD", "R_TRIG", "F_TRIG"]);
        assert_eq!(names(&standard_function_blocks()[0].inputs), ["IN BOOL", "PT TIME"]);
        assert!(!report.programs[0].has_program && report.programs[1].has_program);

        let state = sim.start(&sources(&[("MotorControl", MOTOR_FB), ("Main", main)]), &[], Some(10), true).unwrap();
        assert_eq!(state.programs, ["Main"], "a file with only a FUNCTION_BLOCK has nothing to run");
        assert_eq!(io(&sim.step().unwrap(), "DO0"), Value::Bool(false));
        set_di(&sim, "DI0", true);
        let state = sim.step().unwrap();
        assert_eq!(io(&state, "DO0"), Value::Bool(true));
        assert_eq!((fb_member(&state, "Motor1", "Start"), fb_member(&state, "Motor1", "Motor")), (Value::Bool(true), Value::Bool(true)));
        let block = state.function_blocks.iter().find(|b| b.name == "Motor1").unwrap();
        assert_eq!(block.block_type, "MotorControl");
        assert_eq!(block.members.iter().map(|m| (m.name.as_str(), m.is_output)).collect::<Vec<_>>(), [("Start", false), ("Stop", false), ("Motor", true)]);
        set_di(&sim, "DI0", false);
        assert_eq!(io(&sim.step().unwrap(), "DO0"), Value::Bool(true), "latched inside the instance");
        set_di(&sim, "DI1", true);
        assert_eq!(io(&sim.step().unwrap(), "DO0"), Value::Bool(false));
        sim.stop();
    }

    #[test]
    fn fb_state_persists_and_instances_are_independent() {
        let sim = Simulator::new();
        let main = "PROGRAM Main VAR Counter1 : CounterFB; Counter2 : CounterFB; END_VAR
Counter1(Enable := DI0);
Counter2(Enable := DI1);
END_PROGRAM";
        sim.start(&sources(&[("CounterFB", COUNTER_FB), ("Main", main)]), &[], Some(10), true).unwrap();
        set_di(&sim, "DI0", true);
        for n in 1..=3 {
            let state = sim.step().unwrap();
            assert_eq!(fb_member(&state, "Counter1", "Count"), Value::Int(n), "scan {n}");
            assert_eq!(fb_member(&state, "Counter2", "Count"), Value::Int(0));
        }
        set_di(&sim, "DI0", false);
        set_di(&sim, "DI1", true);
        let mut state = sim.step().unwrap();
        state = if fb_member(&state, "Counter2", "Count") == Value::Int(1) { sim.step().unwrap() } else { state };
        assert_eq!(fb_member(&state, "Counter1", "Count"), Value::Int(3), "stopped, not reset");
        assert_eq!(fb_member(&state, "Counter2", "Count"), Value::Int(2));
        // STOP restores the instances' initial state.
        assert_eq!(fb_member(&sim.stop(), "Counter1", "Count"), Value::Int(0));
    }

    #[test]
    fn fb_internals_initial_values_nesting_and_builtins() {
        let fbs = "FUNCTION_BLOCK Blinker
VAR_INPUT Run : BOOL; END_VAR
VAR_OUTPUT Lamp : BOOL; Ticks : INT; END_VAR
VAR
    Counter : INT := 10;
    T : TON;
END_VAR
Counter := Counter + 1;
Ticks := Counter;
T(IN := Run AND NOT T.Q, PT := T#30ms);
IF T.Q THEN Lamp := NOT Lamp; END_IF;
END_FUNCTION_BLOCK

FUNCTION_BLOCK Station
VAR_INPUT Go : BOOL; END_VAR
VAR_OUTPUT Light : BOOL; END_VAR
VAR B : Blinker; END_VAR
B(Run := Go);
Light := B.Lamp;
END_FUNCTION_BLOCK";
        let main = "PROGRAM Main VAR S1 : Station; B2 : Blinker; Timer1 : TON; Out : BOOL; Ticks : INT; END_VAR
S1(Go := TRUE);
B2(Run := FALSE);
Timer1(IN := TRUE, PT := T#20ms);
Out := S1.Light;
Ticks := B2.Ticks;
END_PROGRAM";
        let sim = Simulator::new();
        sim.start(&sources(&[("Blocks", fbs), ("Main", main)]), &[], Some(10), true).unwrap();
        let mut lights = Vec::new();
        let mut state = sim.state();
        for _ in 0..8 {
            state = sim.step().unwrap();
            lights.push(get(&state, "Out") == Value::Bool(true));
        }
        // The nested TON (30 ms) inside Station's Blinker fires on scan 3, restarts on the
        // scan after (IN = NOT Q), and fires again on scan 7: the lamp toggles every 4 scans.
        assert_eq!(lights, [false, false, true, true, true, true, false, false]);
        assert_eq!(get(&state, "Ticks"), Value::Int(18), "Counter started at 10 and persisted (8 scans)");
        assert_eq!(fb_member(&state, "Timer1", "Q"), Value::Bool(true), "built-in blocks still work beside user blocks");
        sim.stop();
    }

    #[test]
    fn fb_semantic_errors_name_their_file() {
        let main = "PROGRAM Main
VAR M1 : MotorControl; Temp : TemperatureControl; x : BOOL; END_VAR
M1(Start := 100, Stop := TRUE);
M1(Start := DI0);
M1(Start := DI0, Stop := DI1, Emergency := DI2);
M1.Motor := TRUE;
M1.Start := TRUE;
x := M1;
END_PROGRAM";
        let report = compile(&sources(&[("MotorControl", MOTOR_FB), ("Main", main)]), &[]);
        assert_eq!(
            file_errors(&report),
            [
                ("MotorControl", vec![]),
                (
                    "Main",
                    vec![
                        (2, "Unknown data type or function block type 'TemperatureControl' for 'Temp'"),
                        (3, "Type mismatch: cannot assign integer literal to 'M1.Start' (BOOL)"),
                        (4, "Missing required input 'Stop' for Function Block 'MotorControl'."),
                        (5, "Unknown input 'Emergency' for Function Block 'MotorControl'."),
                        (6, "Cannot assign to Function Block output 'Motor'."),
                        (7, "Set MotorControl inputs in a call, e.g. M1(Start := …);"),
                        (8, "'M1' is a MotorControl instance; read one of its outputs, e.g. M1.Motor"),
                    ]
                ),
            ]
        );

        // An error inside a block is reported in the block's file, and nothing runs.
        let broken = MOTOR_FB.replace("Motor := TRUE;", "Motor := 1.5;");
        let user = "PROGRAM Main VAR M1 : MotorControl; END_VAR M1(Start := TRUE, Stop := FALSE); END_PROGRAM";
        let report = compile(&sources(&[("MotorControl", &broken), ("Main", user)]), &[]);
        assert_eq!(file_errors(&report)[0], ("MotorControl", vec![(13, "Type mismatch: cannot assign REAL literal to 'Motor' (BOOL)")]));
        assert!(report.programs[1].errors.is_empty());
        let sim = Simulator::new();
        assert!(sim.start(&sources(&[("MotorControl", &broken), ("Main", user)]), &[], Some(10), false).is_err());
        assert_eq!(sim.state().status, RuntimeStatus::Stopped);
    }

    #[test]
    fn fb_encapsulation_inputs_with_defaults_and_definition_rules() {
        let fbs = "FUNCTION_BLOCK Valve
VAR_INPUT Open : BOOL; Speed : REAL := 50.0; END_VAR
VAR_OUTPUT Pos : REAL; END_VAR
VAR Secret : INT; END_VAR
IF Open THEN Pos := Speed; ELSE Pos := 0.0; END_IF;
END_FUNCTION_BLOCK";
        let ok = "PROGRAM Main VAR V : Valve; p : REAL; END_VAR V(Open := TRUE); p := V.Pos; END_PROGRAM";
        let report = compile(&sources(&[("Valve", fbs), ("Main", ok)]), &[]);
        assert!(report.programs.iter().all(|r| r.errors.is_empty()), "inputs with an initial value are optional: {:?}", file_errors(&report));
        let sim = Simulator::new();
        sim.start(&sources(&[("Valve", fbs), ("Main", ok)]), &[], Some(10), true).unwrap();
        assert_eq!(get(&sim.step().unwrap(), "p"), Value::Real(50.0));
        sim.stop();

        let peek = "PROGRAM Main VAR V : Valve; i : INT; END_VAR V(Open := TRUE); i := V.Secret; END_PROGRAM";
        assert_eq!(
            file_errors(&compile(&sources(&[("Valve", fbs), ("Main", peek)]), &[]))[1].1,
            [(1, "'Secret' is internal to function block Valve; only its inputs and outputs are accessible")]
        );

        // Definitions: recursion, duplicate and reserved names, an instance as an input.
        let defs = "FUNCTION_BLOCK A VAR b : B; END_VAR END_FUNCTION_BLOCK
FUNCTION_BLOCK B VAR a : A; END_VAR END_FUNCTION_BLOCK
FUNCTION_BLOCK A END_FUNCTION_BLOCK
FUNCTION_BLOCK TON END_FUNCTION_BLOCK
FUNCTION_BLOCK C VAR_INPUT t : TON; END_VAR END_FUNCTION_BLOCK";
        let report = compile(&sources(&[("Defs", defs)]), &[]);
        let messages: Vec<&str> = report.programs[0].errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "Function block 'A' contains itself (A → B → A); recursive function blocks are not supported",
                "Function block 'B' contains itself (B → A → B); recursive function blocks are not supported",
                "Function block 'A' is defined more than once",
                "'TON' is a standard function block; give yours another name",
                "Function block inputs and outputs must be BOOL, INT, DINT, REAL or TIME; declare the TON instance 't' under VAR",
            ]
        );
    }

    #[test]
    fn fbs_are_shared_by_every_program_and_files_may_mix() {
        // One file with a block and a program; another program uses the same block.
        let first = format!("{COUNTER_FB}\n\nPROGRAM P1 VAR C : CounterFB; END_VAR C(Enable := TRUE); END_PROGRAM");
        let second = "PROGRAM P2 VAR C : CounterFB; END_VAR C(Enable := TRUE); C(Enable := TRUE); END_PROGRAM";
        let sim = Simulator::new();
        let state = sim.start(&sources(&[("P1", &first), ("P2", second)]), &[], Some(10), true).unwrap();
        assert_eq!(state.programs, ["P1", "P2"]);
        let state = sim.step().unwrap();
        let counts: Vec<Value> = state.function_blocks.iter().map(|b| b.members.iter().find(|m| m.name == "Count").unwrap().value).collect();
        assert_eq!(counts, [Value::Int(1), Value::Int(2)], "each program's C is its own instance");
        sim.stop();

        let two = "PROGRAM A END_PROGRAM PROGRAM B END_PROGRAM";
        assert!(compile(&sources(&[("X", two)]), &[]).programs[0].errors[0].message.starts_with("A file can contain only one PROGRAM"));
    }

    fn where_(state: &RuntimeState) -> Option<Location> {
        state.location.clone()
    }

    fn loc(program: &str, file: &str, line: usize, column: usize, fb: Option<&str>, instance: Option<&str>) -> Option<Location> {
        Some(Location {
            program: program.into(),
            file: file.into(),
            line,
            column,
            function_block: fb.map(Into::into),
            instance: instance.map(Into::into),
        })
    }

    fn at_line(state: &RuntimeState) -> Option<usize> {
        state.location.as_ref().map(|l| l.line)
    }

    /// Spec test program (section 27). Lines: 10 `Motor := TRUE;`, 13 `Counter := …`, 16 `Alarm := TRUE;`.
    const DEBUG_SPEC: &str = "PROGRAM Main
VAR
    Start : BOOL := TRUE;
    Motor : BOOL := FALSE;
    Counter : INT := 0;
    Alarm : BOOL := FALSE;
END_VAR

IF Start THEN
    Motor := TRUE;
END_IF;

Counter := Counter + 1;

IF Counter >= 5 THEN
    Alarm := TRUE;
END_IF;

END_PROGRAM";

    #[test]
    fn spec_debug_program_breakpoints_step_and_resume() {
        let sim = Simulator::new();
        sim.set_breakpoints(vec![bp("Main", 10), bp("Main", 13), bp("Main", 16)]);
        sim.start(&sources(&[("Main", DEBUG_SPEC)]), &[], Some(10), false).unwrap();

        let paused = wait_until(&sim, |s| s.status == RuntimeStatus::Paused);
        assert_eq!(at_line(&paused), Some(10));
        assert_eq!((get(&paused, "Motor"), get(&paused, "Counter")), (Value::Bool(false), Value::Int(0)), "stopped before line 10");

        let stepped = sim.step_statement().unwrap();
        assert_eq!((at_line(&stepped), get(&stepped, "Motor")), (Some(13), Value::Bool(true)), "line 10 ran, now before 13");
        assert_eq!(get(&stepped, "Counter"), Value::Int(0), "13 not yet");

        // Resume: line 13 runs; each later scan stops at 10 and 13 until Counter reaches 5,
        // where the Alarm breakpoint (16) is reached.
        let mut hits = Vec::new();
        for _ in 0..20 {
            sim.resume().unwrap();
            let state = wait_until(&sim, |s| s.status == RuntimeStatus::Paused);
            hits.push(at_line(&state).unwrap());
            if at_line(&state) == Some(16) {
                assert_eq!((get(&state, "Counter"), get(&state, "Alarm")), (Value::Int(5), Value::Bool(false)));
                break;
            }
        }
        assert_eq!(hits, [10, 13, 10, 13, 10, 13, 10, 13, 16]);
        sim.stop();
    }

    #[test]
    fn breakpoints_inside_a_function_block_name_the_instance() {
        // CounterFB lines: 8 `IF Enable THEN`, 9 `Count := Count + 1;`.
        let counter = "FUNCTION_BLOCK CounterFB
VAR_INPUT
    Enable : BOOL;
END_VAR
VAR_OUTPUT
    Count : INT;
END_VAR
IF Enable THEN
    Count := Count + 1;
END_IF;
END_FUNCTION_BLOCK";
        let main = "PROGRAM Main
VAR
    Counter1 : CounterFB;
    Counter2 : CounterFB;
END_VAR
Counter1(Enable := DI0);
Counter2(Enable := DI1);
END_PROGRAM";
        let report = compile(&sources(&[("CounterFB", counter), ("Main", main)]), &[]);
        assert_eq!(report.programs[0].lines, [8, 9], "a block file's statements are breakable");

        let sim = Simulator::new();
        sim.set_breakpoints(vec![bp("CounterFB", 9)]);
        set_di(&sim, "DI0", true);
        sim.start(&sources(&[("CounterFB", counter), ("Main", main)]), &[], Some(10), false).unwrap();
        let paused = wait_until(&sim, |s| s.status == RuntimeStatus::Paused);
        let expected = |instance: &str| loc("Main", "CounterFB", 9, 5, Some("CounterFB"), Some(instance));
        assert_eq!(where_(&paused), expected("Counter1"));
        assert_eq!(fb_member(&paused, "Counter1", "Count"), Value::Int(0), "before the increment");

        let stepped = sim.step_statement().unwrap();
        assert_eq!(fb_member(&stepped, "Counter1", "Count"), Value::Int(1));
        assert_eq!(fb_member(&stepped, "Counter2", "Count"), Value::Int(0), "the other instance is untouched");
        assert_eq!(where_(&stepped).map(|l| (l.program, l.line)), Some(("Main".into(), 7)), "next: back in Main, the second call");

        // Now only Counter2 is enabled: the same breakpoint stops in Counter2.
        set_di(&sim, "DI0", false);
        set_di(&sim, "DI1", true);
        sim.resume().unwrap();
        let paused = wait_until(&sim, |s| s.status == RuntimeStatus::Paused && s.location.as_ref().and_then(|l| l.instance.clone()) == Some("Counter2".into()));
        assert_eq!(where_(&paused), expected("Counter2"));
        assert_eq!(fb_member(&paused, "Counter1", "Count"), Value::Int(1));
        sim.stop();
    }

    #[test]
    fn stepping_enters_function_blocks_and_shows_their_internals() {
        let fb = "FUNCTION_BLOCK Acc
VAR_INPUT Add : INT; END_VAR
VAR_OUTPUT Total : INT; END_VAR
VAR Calls : INT := 100; END_VAR
Calls := Calls + 1;
Total := Total + Add;
END_FUNCTION_BLOCK";
        let main = "PROGRAM Main VAR A : Acc; END_VAR\nA(Add := 2);\nEND_PROGRAM";
        let sim = Simulator::new();
        sim.set_breakpoints(vec![bp("Main", 2)]);
        sim.start(&sources(&[("Acc", fb), ("Main", main)]), &[], Some(10), false).unwrap();
        wait_until(&sim, |s| s.status == RuntimeStatus::Paused);
        let inside = sim.step_statement().unwrap();
        assert_eq!(where_(&inside), loc("Main", "Acc", 5, 1, Some("Acc"), Some("A")), "stepped into the block");
        let block = inside.function_blocks.iter().find(|b| b.name == "A").unwrap();
        let calls = block.members.iter().find(|m| m.name == "Calls").unwrap();
        assert_eq!((calls.value, calls.is_internal, calls.is_output), (Value::Int(100), true, false));
        assert_eq!(fb_member(&sim.step_statement().unwrap(), "A", "Calls"), Value::Int(101));
        sim.stop();
    }

    #[test]
    fn debugger_with_io_outputs_are_written_at_the_end_of_the_scan() {
        let main = "PROGRAM Main\nIF DI0 THEN\n    DO0 := TRUE;\nELSE\n    DO0 := FALSE;\nEND_IF;\nEND_PROGRAM";
        let sim = Simulator::new();
        sim.set_breakpoints(vec![bp("Main", 3)]);
        set_di(&sim, "DI0", true);
        sim.start(&sources(&[("Main", main)]), &[], Some(10), false).unwrap();
        let paused = wait_until(&sim, |s| s.status == RuntimeStatus::Paused);
        assert_eq!((at_line(&paused), io(&paused, "DI0"), io(&paused, "DO0")), (Some(3), Value::Bool(true), Value::Bool(false)));
        // The statement runs; the output image is published when the scan completes.
        let stepped = sim.step_statement().unwrap();
        assert_eq!(stepped.cycle_count, 1, "that was the scan's last statement: outputs written");
        assert_eq!(io(&stepped, "DO0"), Value::Bool(true));
        sim.stop();
    }

    #[test]
    fn io_maps_to_function_block_inputs_and_outputs() {
        let motor = "FUNCTION_BLOCK MotorBlock
VAR_INPUT Start : BOOL; Stop : BOOL; END_VAR
VAR_OUTPUT Motor : BOOL; END_VAR
VAR Runs : INT; END_VAR
IF Start AND NOT Stop THEN Motor := TRUE; END_IF;
IF Stop THEN Motor := FALSE; END_IF;
END_FUNCTION_BLOCK";
        // Only the calls: the I/O tab does all the wiring (Start/Stop are required inputs).
        let main = "PROGRAM Main VAR Motor1 : MotorBlock; T1 : TON; END_VAR
Motor1();
T1(PT := T#20ms);
END_PROGRAM";
        let mappings = map(&[
            ("Motor1.Start", "DI0"),
            ("main.Motor1.Stop", "DI1"),
            ("Motor1.Motor", "DO0"),
            ("T1.IN", "DI2"),
            ("T1.Q", "DO1"),
        ]);
        let files = sources(&[("MotorBlock", motor), ("main", main)]);
        let report = compile(&files, &mappings);
        assert!(report.mapping.is_empty() && report.programs.iter().all(|r| r.errors.is_empty()), "{report:?}");

        let sim = Simulator::new();
        sim.start(&files, &mappings, Some(10), true).unwrap();
        set_di(&sim, "DI0", true);
        assert_eq!(io(&sim.step().unwrap(), "DO0"), Value::Bool(true), "DI0 → Motor1.Start, Motor1.Motor → DO0");
        set_di(&sim, "DI0", false);
        set_di(&sim, "DI1", true);
        assert_eq!(io(&sim.step().unwrap(), "DO0"), Value::Bool(false));
        set_di(&sim, "DI2", true);
        sim.step().unwrap();
        let state = sim.step().unwrap();
        assert_eq!(io(&state, "DO1"), Value::Bool(true), "DI2 → T1.IN (standard block), T1.Q → DO1");
        assert_eq!(fb_member(&state, "T1", "IN"), Value::Bool(true));
        sim.stop();

        // Mistakes: an output fed by an input, an internal variable, a wrong type, a missing
        // member, and an input both mapped and given in the call.
        let both = "PROGRAM Main VAR Motor1 : MotorBlock; END_VAR
Motor1(Start := TRUE);
END_PROGRAM";
        let bad = compile(
            &sources(&[("MotorBlock", motor), ("main", both)]),
            &map(&[("Motor1.Motor", "DI0"), ("Motor1.Runs", "DO0"), ("Motor1.Start", "AI0"), ("Motor1.Nope", "DO1"), ("Motor1.Start", "DI1"), ("Motor1.Stop", "DI2")]),
        );
        let messages: Vec<&str> = bad.mapping.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "Cannot map DIGITAL INPUT DI0 to Motor1.Motor: it is an output, written by the block.",
                "Cannot map DIGITAL OUTPUT DO0: 'Runs' is internal to function block MotorBlock; map one of its inputs or outputs.",
                "Cannot map ANALOG INPUT AI0 to BOOL input 'Motor1.Start'.",
                "Cannot map DIGITAL OUTPUT DO1: MotorBlock has no input or output 'Nope' (members: Start, Stop, Motor).",
            ]
        );
        assert_eq!(
            bad.programs[1].errors.iter().map(|e| (e.line, e.message.as_str())).collect::<Vec<_>>(),
            [(2, "'Motor1.Start' already comes from an I/O mapping; remove it from this call or from the I/O tab")]
        );
    }
}
