//! Compiler queries for the editor (diagnostics, the function block library). Nothing
//! runs here: the PLC itself is the backend, reached over a WebSocket.

use crate::compiler::ast::DataType;
use crate::compiler::project::{self, CompileReport, FbSummary, ProgramSource};
use crate::io::Mapping;

/// The standard function blocks with their inputs and outputs (for the library panel).
#[tauri::command]
pub fn standard_function_blocks() -> Vec<FbSummary> {
    project::standard_function_blocks()
}

/// Compile without running: every program's errors and the I/O mapping errors. `tags`:
/// the SCADA tags (name, type) the programs may read.
#[tauri::command]
pub fn compile_programs(programs: Vec<ProgramSource>, mappings: Vec<Mapping>, tags: Option<Vec<(String, DataType)>>) -> CompileReport {
    myplc_core::compile(&programs, &mappings, &tags.unwrap_or_default())
}
