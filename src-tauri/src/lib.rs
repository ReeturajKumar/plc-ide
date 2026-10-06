mod commands;
mod fs;
mod io;
mod simulator;
mod st;

use commands::execution;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(simulator::Simulator::new())
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
            execution::start_program,
            execution::compile_programs,
            execution::standard_function_blocks,
            execution::step_program,
            execution::step_statement,
            execution::set_breakpoints,
            execution::stop_program,
            execution::pause_program,
            execution::resume_program,
            execution::get_runtime_state,
            execution::set_input,
            execution::set_io_input,
            execution::apply_io_mappings,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
