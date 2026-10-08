//! The MyPLC desktop app (frontend shell): project files, dialogs, and compiling for the
//! editor's diagnostics. It runs no PLC: the UI talks to the PLC backend (`myplc-runtime
//! --listen`) directly over a WebSocket (see `ui/src/utils/runtimeSocket.ts`).

mod commands;
mod fs;

use myplc_core::{compiler, io};

use commands::execution;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            fs::create_project,
            fs::read_project,
            fs::write_project_json,
            fs::read_file,
            fs::write_file,
            fs::rename_file,
            fs::read_tree,
            fs::create_file,
            fs::create_dir,
            fs::delete_entry,
            execution::compile_programs,
            execution::standard_function_blocks,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
