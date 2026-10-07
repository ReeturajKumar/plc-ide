//! myplc-runtime: the PLC runtime on its own, without the IDE.
//!
//! ```text
//! myplc-runtime <project-dir> [--scan-ms N] [--scans N] [--trace] [--io simulated]
//! myplc-runtime --serve
//! ```
//!
//! With `--serve` it is a service for one client (the IDE) speaking the MyPLC protocol over
//! stdin/stdout; see `serve`. Otherwise:
//!
//! Reads `<project-dir>/project.json` (the IDE's project file: its programs in execution
//! order and its I/O mappings), compiles the sources, loads the compiled project into the
//! runtime and scans it on simulated I/O until told to stop. Commands on stdin, one per line:
//!
//! ```text
//! DI0 1 | DI0 0 | AI0 12.5   set a simulated input; the next scan reads it
//! state                      print every variable, function block member and I/O point
//! stop                       stop cleanly and exit (end of input too, unless --scans is set)
//! ```
//!
//! Both modes run the project the same way, through `host::Host` (the one runtime host:
//! lifecycle, scan timing and the scan cycle); this file only parses arguments, reads
//! stdin and prints.

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc;
use std::{env, fs, io::BufRead, thread};

use host::Host;
use myplc_core::compiler::project::{CompileReport, ProgramSource};
use myplc_core::compiler::value::Value;
use myplc_core::io::{self, InputValue, IoKind, Mapping};
use myplc_core::protocol::{self, ErrorCode, ProtocolError, RuntimeState};
use myplc_core::ENGINE_STACK_BYTES;

mod host;
mod serve;

const USAGE: &str = "usage: myplc-runtime <project-dir> [--scan-ms N] [--scans N] [--trace] [--io simulated]
       myplc-runtime --serve";

/// Where the I/O comes from. Only the simulated I/O exists for now.
#[derive(Debug, Clone, Copy, PartialEq)]
enum IoMode {
    Simulated,
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeConfig {
    project_path: PathBuf,
    /// PLC scan interval; timers advance by exactly this per scan.
    scan_interval_ms: u64,
    /// Stop after this many scans (otherwise on `stop` or end of input).
    max_scans: Option<u64>,
    /// Print the variables and I/O after every scan.
    trace: bool,
    io: IoMode,
}

impl RuntimeConfig {
    fn from_args(mut args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut config = RuntimeConfig {
            project_path: PathBuf::new(),
            scan_interval_ms: 100,
            max_scans: None,
            trace: false,
            io: IoMode::Simulated,
        };
        let number = |flag: &str, value: Option<String>| -> Result<u64, String> {
            value.and_then(|v| v.parse().ok()).filter(|&n| n > 0).ok_or(format!("{flag} needs a positive number"))
        };
        let mut project = None;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--scan-ms" => config.scan_interval_ms = number("--scan-ms", args.next())?.clamp(1, 10_000),
                "--scans" => config.max_scans = Some(number("--scans", args.next())?),
                "--trace" => config.trace = true,
                "--io" => match args.next().as_deref() {
                    Some("simulated") => config.io = IoMode::Simulated,
                    other => return Err(format!("unsupported I/O mode {other:?}: only 'simulated' is available")),
                },
                flag if flag.starts_with("--") => return Err(format!("unknown option {flag}")),
                _ if project.is_none() => project = Some(PathBuf::from(arg)),
                _ => return Err(format!("unexpected argument {arg}")),
            }
        }
        config.project_path = project.ok_or("missing <project-dir>")?;
        Ok(config)
    }
}

/// A line of stdin, understood.
#[derive(Debug, PartialEq)]
enum Command {
    SetInput(String, InputValue),
    State,
    Stop,
}

fn parse_command(line: &str) -> Result<Option<Command>, String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    match words.as_slice() {
        [] => Ok(None),
        ["stop" | "quit" | "exit"] => Ok(Some(Command::Stop)),
        ["state"] => Ok(Some(Command::State)),
        [address, value] => {
            let (kind, _) = io::CONFIG.locate(address).ok_or(format!("unknown I/O address '{address}'"))?;
            let value = if kind == IoKind::DigitalInput {
                match value.to_ascii_lowercase().as_str() {
                    "1" | "true" | "on" => InputValue::Bool(true),
                    "0" | "false" | "off" => InputValue::Bool(false),
                    _ => return Err(format!("{address} is digital: use 1/0, true/false or on/off")),
                }
            } else {
                InputValue::Number(value.parse().map_err(|_| format!("{address} needs a number"))?)
            };
            Ok(Some(Command::SetInput(address.to_string(), value)))
        }
        _ => Err(format!("unknown command '{line}' (try: DI0 1, AI0 12.5, state, stop)")),
    }
}

/// The IDE's project file: programs (in execution order) and I/O mappings.
fn read_project(dir: &Path) -> Result<(Vec<ProgramSource>, Vec<Mapping>), String> {
    let file = dir.join("project.json");
    let text = fs::read_to_string(&file).map_err(|e| format!("can't read {}: {e}", file.display()))?;
    let json: serde_json::Value = serde_json::from_str(&text).map_err(|e| format!("{} is not valid JSON: {e}", file.display()))?;
    let programs = json["programs"]
        .as_array()
        .ok_or("project.json has no programs")?
        .iter()
        .map(|p| {
            let (Some(name), Some(path)) = (p["name"].as_str(), p["path"].as_str()) else {
                return Err("every program needs a name and a path".to_string());
            };
            let source = fs::read_to_string(dir.join(path)).map_err(|e| format!("can't read {path}: {e}"))?;
            Ok(ProgramSource { name: name.to_string(), source })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mappings = match json.pointer("/io/mappings") {
        Some(m) => serde_json::from_value(m.clone()).map_err(|e| format!("invalid I/O mappings: {e}"))?,
        None => Vec::new(),
    };
    Ok((programs, mappings))
}

fn show(value: Value) -> String {
    match value {
        Value::Bool(b) => if b { "TRUE" } else { "FALSE" }.to_string(),
        Value::Int(n) => n.to_string(),
        Value::DInt(n) => n.to_string(),
        Value::Real(x) => x.to_string(),
        Value::Time(ms) => format!("T#{ms}ms"),
    }
}

/// One line: every variable, then every I/O point.
fn trace_line(state: &RuntimeState) -> String {
    let variables = state.variables.iter().map(|v| format!("{}.{}={}", v.program, v.name, show(v.value)));
    let points = state.io.iter().map(|p| format!("{}={}", p.address, show(p.value)));
    variables.chain(points).collect::<Vec<_>>().join(" ")
}

fn print_state(state: &RuntimeState) {
    println!("[state] programs: {}  scans: {}", state.programs.join(" -> "), state.cycle_count);
    for v in &state.variables {
        let mapped = v.io_address.as_ref().map(|a| format!("  ({a})")).unwrap_or_default();
        println!("  {}.{} : {} = {}{mapped}", v.program, v.name, v.data_type.name(), show(v.value));
    }
    for fb in &state.function_blocks {
        let members = fb.inputs.iter().chain(&fb.outputs).chain(&fb.internals);
        let members: Vec<_> = members.map(|m| format!("{}={}", m.name, show(m.value))).collect();
        println!("  {}.{} : {} [{}]", fb.program, fb.instance, fb.type_name, members.join(" "));
    }
    let points: Vec<_> = state.io.iter().map(|p| format!("{}={}", p.address, show(p.value))).collect();
    println!("  I/O: {}", points.join(" "));
}

fn report_errors(report: &CompileReport) {
    for program in &report.programs {
        for e in &program.errors {
            eprintln!("  {} line {}, column {}: {}", program.name, e.line, e.column, e.message);
        }
    }
    for e in &report.mapping {
        eprintln!("  I/O mapping: {}", e.message);
    }
}

/// What the run waits for.
enum Event {
    Line(Command),
    EndOfInput,
    /// `--scans` reached, or a scan failed.
    ScansDone,
}

/// LOAD → RUNNING → STOPPING → STOPPED, all carried out by the host.
fn run(config: RuntimeConfig, programs: Vec<ProgramSource>, mappings: Vec<Mapping>) -> ExitCode {
    let host = Host::new();
    let load = protocol::Command::LoadProject { programs, mappings, scan_time_ms: Some(config.scan_interval_ms) };
    let loaded = match host.execute(load) {
        Ok(state) => state,
        Err(ProtocolError { code: ErrorCode::CompilationError(report), .. }) => {
            eprintln!("[runtime] compilation failed:");
            report_errors(&report);
            return ExitCode::FAILURE;
        }
        Err(e) => {
            eprintln!("[runtime] {}", e.message);
            return ExitCode::FAILURE;
        }
    };
    println!("[runtime] LOADED {} program(s): {}", loaded.programs.len(), loaded.programs.join(" -> "));
    let IoMode::Simulated = config.io;

    let (events, next_event) = mpsc::channel();
    // Each finished scan: trace it; end the run after --scans, or when a scan failed.
    let (trace, max_scans, scans_done) = (config.trace, config.max_scans, events.clone());
    host.on_scan(move |state| {
        if trace {
            println!("scan {}: {}", state.cycle_count, trace_line(state));
        }
        let done = state.error.is_some() || max_scans.is_some_and(|max| state.cycle_count >= max);
        if done {
            let _ = scans_done.send(Event::ScansDone);
        }
        !done
    });
    // Commands on stdin, one per line.
    thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            match parse_command(&line) {
                Ok(Some(command)) => {
                    if events.send(Event::Line(command)).is_err() {
                        return; // the run is over
                    }
                }
                Ok(None) => {}
                Err(e) => eprintln!("[runtime] {e}"),
            }
        }
        let _ = events.send(Event::EndOfInput);
    });

    match host.execute(protocol::Command::Run(None)) {
        Ok(running) => println!("[runtime] RUNNING: scan every {} ms, simulated I/O", running.scan_time_ms),
        Err(e) => {
            eprintln!("[runtime] {}", e.message);
            return ExitCode::FAILURE;
        }
    }
    // Requests reach the host between scans; the next READ INPUTS sees them.
    while let Ok(event) = next_event.recv() {
        match event {
            Event::Line(Command::SetInput(address, value)) => {
                if let Err(e) = host.execute(protocol::Command::SetInput { address, value }) {
                    eprintln!("[runtime] {}", e.message);
                }
            }
            Event::Line(Command::State) => print_state(&host.state()),
            Event::Line(Command::Stop) | Event::ScansDone => break,
            Event::EndOfInput if config.max_scans.is_none() => break,
            Event::EndOfInput => {} // keep scanning until --scans
        }
    }

    // Freeze the final state (no-op if the host already stopped scanning), report, stop.
    let _ = host.execute(protocol::Command::Pause);
    let last = host.state();
    let mut code = ExitCode::SUCCESS;
    if let Some(failure) = &last.error {
        eprintln!("[runtime] scan failed in {} line {}: {}", failure.program, failure.error.line, failure.error.message);
        code = ExitCode::FAILURE;
    }
    println!("[runtime] STOPPING after {} scan(s)", last.cycle_count);
    print_state(&last);
    let _ = host.execute(protocol::Command::Stop); // every output to its safe state
    println!("[runtime] STOPPED");
    code
}

fn main() -> ExitCode {
    if env::args().nth(1).as_deref() == Some("--serve") {
        // Commands like STEP run scans on the serving thread: give it the engine's stack.
        let server = thread::Builder::new().name("plc-serve".into()).stack_size(ENGINE_STACK_BYTES).spawn(serve::serve);
        return server.ok().and_then(|s| s.join().ok()).unwrap_or(ExitCode::FAILURE);
    }
    let config = match RuntimeConfig::from_args(env::args().skip(1)) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("myplc-runtime: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    println!("[runtime] STARTING: {}", config.project_path.display());
    match read_project(&config.project_path) {
        Ok((programs, mappings)) => run(config, programs, mappings),
        Err(e) => {
            eprintln!("[runtime] {e}");
            ExitCode::FAILURE
        }
    }
}

