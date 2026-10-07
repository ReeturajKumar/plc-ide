//! Tauri commands for the PLC. The runtime is reached through one transport command that
//! carries protocol messages unchanged (`runtime_request`); the compiler queries are
//! separate, since compiling runs nothing.

use tauri::State;

use crate::compiler::project::{self, CompileReport, FbSummary, ProgramSource};
use crate::io::Mapping;
use crate::runtime_client::RuntimeClient;
use myplc_core::protocol::{Request, Response};

/// A protocol request for `myplc-runtime`, answered with its protocol response.
#[tauri::command]
pub fn runtime_request(runtime: State<'_, RuntimeClient>, request: Request) -> Response {
    runtime.request(request)
}

/// The standard function blocks with their inputs and outputs (for the library panel).
#[tauri::command]
pub fn standard_function_blocks() -> Vec<FbSummary> {
    project::standard_function_blocks()
}

/// Compile without running: every program's errors and the I/O mapping errors.
#[tauri::command]
pub fn compile_programs(programs: Vec<ProgramSource>, mappings: Vec<Mapping>) -> CompileReport {
    myplc_core::compile(&programs, &mappings)
}
