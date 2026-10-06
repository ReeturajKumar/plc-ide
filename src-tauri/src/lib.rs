mod commands;
mod fs;
mod runtime_client;

use myplc_core::{compiler, io};

use commands::execution;
use tauri::{Emitter, Manager};

/// Pushed with the protocol `RuntimeState` whenever the PLC state changes.
const STATE_UPDATE_EVENT: &str = "runtime://state-update";
/// Pushed with `{ code: "UNAVAILABLE", message }` when the runtime stops unexpectedly.
const UNAVAILABLE_EVENT: &str = "runtime://unavailable";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // The runtime's state changes go to the UI as they happen (no polling).
            let ui = app.handle().clone();
            let runtime = runtime_client::RuntimeClient::new(runtime_client::resolve_runtime_executable()).with_events(move |event| {
                let sent = match event {
                    runtime_client::RuntimeEvent::State(state) => ui.emit(STATE_UPDATE_EVENT, state),
                    runtime_client::RuntimeEvent::Lost(error) => ui.emit(UNAVAILABLE_EVENT, error),
                };
                if let Err(e) = sent {
                    eprintln!("[runtime] can't notify the UI: {e}");
                }
            });
            app.manage(runtime);
            Ok(())
        })
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
            execution::runtime_request,
            execution::compile_programs,
            execution::standard_function_blocks,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|app, event| {
            // Closing the IDE stops the PLC and ends the runtime process.
            if let tauri::RunEvent::Exit = event {
                app.state::<runtime_client::RuntimeClient>().shutdown();
            }
        });
}
