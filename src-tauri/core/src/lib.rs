//! MyPLC core: the Structured Text compiler, the I/O model, the PLC runtime and the UI ↔
//! runtime protocol. No UI, no Tauri, no process or transport: `myplc-runtime` hosts the
//! runtime and serves the protocol; the IDE uses the compiler and speaks the protocol.

pub mod compiler;
pub mod io;
pub mod protocol;
pub mod runtime;

use compiler::project::{compile_sources, CompileReport, ProgramSource, ProjectContext};
use io::{IoConfig, Mapping};
use std::thread;
use runtime::Runtime;

/// Stack for the threads that load and scan programs. The parser, checker and runtime
/// recurse over the program tree; at `MAX_NESTING` a debug build needs about 1 MB
/// (measured), which is a whole default Windows main thread.
/// 8 MB is reserved address space, not committed memory, and leaves ~8x headroom.
pub const ENGINE_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Source files → compiler → compiled project → runtime: what every host does to load a
/// project. Every file's errors come back at once; an initial value the runtime can't
/// create is reported as an error of its program. Run it on an `ENGINE_STACK_BYTES` stack.
pub fn load(programs: &[ProgramSource], mappings: &[Mapping], config: &IoConfig) -> (Option<(Runtime, ProjectContext)>, CompileReport) {
    let (project, mut report) = compile_sources(programs, mappings, config);
    let Some(project) = project else { return (None, report) };
    let context = project.context.clone();
    let files: Vec<usize> = project.programs.iter().map(|p| p.file).collect();
    match Runtime::load(project) {
        Ok(runtime) => (Some((runtime, context)), report),
        Err(errors) => {
            for (k, e) in errors.into_iter().enumerate() {
                report.programs[files[k]].errors.extend(e);
            }
            (None, report)
        }
    }
}

/// `load` with the standard I/O, on its own `ENGINE_STACK_BYTES` thread (so it is safe
/// to call from any thread, however small its stack).
pub fn load_on_engine_stack(programs: &[ProgramSource], mappings: &[Mapping]) -> (Option<(Runtime, ProjectContext)>, CompileReport) {
    let failed = |message: &str| CompileReport::failed(programs, message);
    thread::scope(|scope| {
        let loader = thread::Builder::new()
            .name("plc-load".into())
            .stack_size(ENGINE_STACK_BYTES)
            .spawn_scoped(scope, || load(programs, mappings, &io::CONFIG))
            .map_err(|_| (None, failed("Could not start the PLC engine.")));
        match loader {
            Ok(loader) => loader.join().unwrap_or_else(|_| (None, failed("The PLC engine failed while loading the program."))),
            Err(failure) => failure,
        }
    })
}

/// Compile every program (lexer → parser → semantic analysis) and check the I/O mappings,
/// without running anything.
pub fn compile(programs: &[ProgramSource], mappings: &[Mapping]) -> CompileReport {
    load_on_engine_stack(programs, mappings).1
}
