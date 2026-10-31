//! MyPLC core: the Structured Text compiler, the I/O model, the PLC runtime and the UI ↔
//! runtime protocol. No UI, no Tauri, no process or transport: `myplc-runtime` hosts the
//! runtime and serves the protocol; the IDE uses the compiler and speaks the protocol.

pub mod compiler;
pub mod io;
pub mod protocol;
pub mod runtime;

use compiler::ast::DataType;
use compiler::project::{compile_sources, CompileReport, ProgramSource, ProjectContext};
use io::{IoConfig, Mapping};
use std::thread;
use runtime::Runtime;

/// Stack for the threads that load and scan programs. The parser, checker and runtime
/// recurse over the program tree; at `MAX_NESTING` a debug build needs about 1 MB
/// (measured), which is a whole default Windows main thread.
/// 8 MB is reserved address space, not committed memory, and leaves ~8x headroom.
pub const ENGINE_STACK_BYTES: usize = 8 * 1024 * 1024;

/// Every SCADA tag `X` also gets a read-only BOOL `X_VALID`: TRUE once `X` has received a live
/// value since the PLC started, FALSE while it only holds the value restored from the last
/// snapshot. Programs guard actions with it, e.g. `IF TANK1_VALID AND TANK1 > 80 THEN …`.
pub const VALID_SUFFIX: &str = "_VALID";

/// The SCADA tags plus each one's `_VALID` flag (skipped if a real tag already has that name).
fn with_validity_flags(tags: &[(String, DataType)]) -> Vec<(String, DataType)> {
    let names: std::collections::HashSet<String> = tags.iter().map(|(n, _)| n.to_uppercase()).collect();
    let flags = tags
        .iter()
        .map(|(n, _)| format!("{n}{VALID_SUFFIX}"))
        .filter(|flag| !names.contains(&flag.to_uppercase()))
        .map(|flag| (flag, DataType::Bool));
    tags.iter().cloned().chain(flags).collect()
}

/// Source files → compiler → compiled project → runtime: what every host does to load a
/// project. Every file's errors come back at once; an initial value the runtime can't
/// create is reported as an error of its program. `tags` are the SCADA tags (name, type)
/// every program can read. Run it on an `ENGINE_STACK_BYTES` stack.
pub fn load(
    programs: &[ProgramSource],
    mappings: &[Mapping],
    config: &IoConfig,
    tags: &[(String, DataType)],
) -> (Option<(Runtime, ProjectContext)>, CompileReport) {
    let (project, mut report) = compile_sources(programs, mappings, config, &with_validity_flags(tags));
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
pub fn load_on_engine_stack(
    programs: &[ProgramSource],
    mappings: &[Mapping],
    tags: &[(String, DataType)],
) -> (Option<(Runtime, ProjectContext)>, CompileReport) {
    let failed = |message: &str| CompileReport::failed(programs, message);
    thread::scope(|scope| {
        let loader = thread::Builder::new()
            .name("plc-load".into())
            .stack_size(ENGINE_STACK_BYTES)
            .spawn_scoped(scope, || load(programs, mappings, &io::CONFIG, tags))
            .map_err(|_| (None, failed("Could not start the PLC engine.")));
        match loader {
            Ok(loader) => loader.join().unwrap_or_else(|_| (None, failed("The PLC engine failed while loading the program."))),
            Err(failure) => failure,
        }
    })
}

/// Compile every program (lexer → parser → semantic analysis) and check the I/O mappings,
/// without running anything.
pub fn compile(programs: &[ProgramSource], mappings: &[Mapping], tags: &[(String, DataType)]) -> CompileReport {
    load_on_engine_stack(programs, mappings, tags).1
}

#[cfg(test)]
mod tests {
    use super::*;
    use compiler::value::Value;
    use runtime::Stop;

    #[test]
    fn each_tag_gets_a_valid_flag_that_starts_false() {
        let source = "PROGRAM main VAR ok : BOOL; END_VAR ok := T_VALID; END_PROGRAM";
        let programs = vec![ProgramSource { name: "main".into(), source: source.into() }];
        let tags = vec![("T".to_string(), DataType::Real)];
        let (loaded, report) = load(&programs, &[], &io::CONFIG, &tags);
        let (mut runtime, _) = loaded.unwrap_or_else(|| panic!("T_VALID should compile: {report:?}"));
        let mut image = io::IoImage::new(&io::CONFIG);
        let ok = |r: &Runtime| r.variables().into_iter().find(|v| v.name == "ok").map(|v| v.value);

        runtime.scan_cycle(&mut image, 100, Stop::Never);
        assert_eq!(ok(&runtime), Some(Value::Bool(false))); // no live value yet

        runtime.set_tag("T_VALID", 1.0); // the host does this when a live value arrives
        runtime.scan_cycle(&mut image, 100, Stop::Never);
        assert_eq!(ok(&runtime), Some(Value::Bool(true)));
    }
}
