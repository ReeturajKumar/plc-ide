//! Tauri commands for the PLC simulator. Thin wrappers: all logic lives in `simulator`.

use tauri::State;

use crate::simulator::state::RuntimeState;
use crate::io::{InputValue, Mapping};
use crate::simulator::{self, Breakpoint, CompileReport, FbSummary, ProgramSource, Simulator};
use crate::st::error::{StError, StErrors};

/// Compile every program, check the I/O mappings and start the PLC (paused, ready for
/// STEP, if `paused`). With any error nothing runs, and the full report is returned.
#[tauri::command]
pub fn start_program(
    simulator: State<'_, Simulator>,
    programs: Vec<ProgramSource>,
    mappings: Vec<Mapping>,
    scan_time_ms: Option<u64>,
    paused: Option<bool>,
) -> Result<RuntimeState, CompileReport> {
    simulator.start(&programs, &mappings, scan_time_ms, paused.unwrap_or(false))
}

/// The standard function blocks with their inputs and outputs (for the library panel).
#[tauri::command]
pub fn standard_function_blocks() -> Vec<FbSummary> {
    simulator::standard_function_blocks()
}

/// Compile without running: every program's errors and the I/O mapping errors.
#[tauri::command]
pub fn compile_programs(programs: Vec<ProgramSource>, mappings: Vec<Mapping>) -> CompileReport {
    simulator::compile(&programs, &mappings)
}

/// Apply new I/O mappings to the loaded programs immediately; returns the mapping errors.
#[tauri::command]
pub fn apply_io_mappings(simulator: State<'_, Simulator>, mappings: Vec<Mapping>) -> StErrors {
    simulator.apply_mappings(&mappings)
}

/// Set a simulated input (DI: true/false, AI: a number). Outputs can't be set.
#[tauri::command]
pub fn set_io_input(simulator: State<'_, Simulator>, address: String, value: InputValue) -> Result<RuntimeState, StError> {
    simulator.set_io_input(&address, value)
}

#[tauri::command]
pub fn stop_program(simulator: State<'_, Simulator>) -> RuntimeState {
    simulator.stop()
}

#[tauri::command]
pub fn pause_program(simulator: State<'_, Simulator>) -> Result<RuntimeState, StError> {
    simulator.pause()
}

#[tauri::command]
pub fn resume_program(simulator: State<'_, Simulator>) -> Result<RuntimeState, StError> {
    simulator.resume()
}

/// Execute exactly one scan of every program while paused.
#[tauri::command]
pub fn step_program(simulator: State<'_, Simulator>) -> Result<RuntimeState, StError> {
    simulator.step()
}

/// Debugger: run the next statement while paused, then stop again.
#[tauri::command]
pub fn step_statement(simulator: State<'_, Simulator>) -> Result<RuntimeState, StError> {
    simulator.step_statement()
}

/// Debugger: replace every breakpoint (program + line).
#[tauri::command]
pub fn set_breakpoints(simulator: State<'_, Simulator>, breakpoints: Vec<Breakpoint>) -> RuntimeState {
    simulator.set_breakpoints(breakpoints)
}

#[tauri::command]
pub fn get_runtime_state(simulator: State<'_, Simulator>) -> RuntimeState {
    simulator.state()
}

#[tauri::command]
pub fn set_input(
    simulator: State<'_, Simulator>,
    program: String,
    name: String,
    value: bool,
) -> Result<RuntimeState, StError> {
    simulator.set_input(&program, &name, value)
}
