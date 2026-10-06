//! Compile orchestration: project sources → a `CompiledProject`, the representation the
//! runtime loads and executes, plus the per-file report the IDE shows (errors, breakable
//! lines, declarations, function blocks). Nothing here executes anything.

use serde::{Deserialize, Serialize};

use super::ast::{statement_lines, DataType, FunctionBlockDef, Program, VarDecl, VarType};
use super::checker::{check_project, Checked, CheckedFb, ProjectCheck};
use super::error::{StError, StErrors};
use super::function_blocks::{reference_source, FbKind};
use super::parser::parse_file;
use super::symbols::{FbInterface, Scope, SymbolTable};
use crate::io::mapping::{self, Mapping, ResolvedMapping};
use crate::io::IoConfig;

/// One source file of the project, as the IDE sends it: its name and current text.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgramSource {
    pub name: String,
    pub source: String,
}

/// The compilation result of one file: compiled if `errors` is empty.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredVariable {
    pub name: String,
    /// BOOL, INT, …, or the function block type (TON, CTU, …).
    pub data_type: String,
}

/// A function block's interface, for the IDE's function block library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    fn of(fb: &FunctionBlockDef) -> Self {
        let vars = |decls: &[VarDecl]| {
            decls.iter().map(|d| DeclaredVariable { name: d.name.clone(), data_type: d.type_name.clone() }).collect()
        };
        Self { name: fb.name.clone(), inputs: vars(&fb.inputs), outputs: vars(&fb.outputs), line: fb.line, source: None }
    }
}

/// The standard function blocks (TON, TOF, TP, CTU, CTD, R_TRIG, F_TRIG) with their inputs
/// and outputs, for the IDE's function block library.
pub fn standard_function_blocks() -> Vec<FbSummary> {
    let vars = |members: &[(&str, DataType)]| {
        members.iter().map(|(n, t)| DeclaredVariable { name: n.to_string(), data_type: t.name().to_string() }).collect()
    };
    FbKind::ALL
        .iter()
        .map(|kind| FbSummary {
            name: kind.name().to_string(),
            inputs: vars(kind.inputs()),
            outputs: vars(kind.outputs()),
            line: 1,
            source: Some(reference_source(*kind)),
        })
        .collect()
}

/// Compiling a project: one result per file, in order, and the I/O mapping errors.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompileReport {
    pub programs: Vec<CompileResult>,
    pub mapping: StErrors,
}

impl CompileReport {
    /// Every file failed with the same error (the engine itself couldn't run).
    pub fn failed(sources: &[ProgramSource], message: &str) -> Self {
        let programs = sources
            .iter()
            .map(|p| CompileResult {
                name: p.name.clone(),
                errors: vec![StError::general(message)],
                lines: Vec::new(),
                variables: Vec::new(),
                function_blocks: Vec::new(),
                has_program: false,
            })
            .collect();
        Self { programs, mapping: StErrors::new() }
    }
}

/// A checked PROGRAM, ready to run.
#[derive(Debug, Clone)]
pub struct CompiledProgram {
    pub name: String,
    /// Initial values typed, body lowered.
    pub program: Program,
    pub symbols: SymbolTable,
    /// Index of its source file (in the compiled sources).
    pub file: usize,
}

/// What the compiler knows about a compiled project, kept to check and resolve new I/O
/// mappings later without recompiling.
#[derive(Debug, Clone)]
pub struct ProjectContext {
    pub config: IoConfig,
    /// Every program's symbol table, then the I/O addresses' (with `names`).
    pub tables: Vec<SymbolTable>,
    pub names: Vec<String>,
    /// The project's FUNCTION_BLOCK types, indexed by `VarType::UserFb`.
    pub interfaces: Vec<FbInterface>,
}

impl ProjectContext {
    /// Check `mappings` and resolve them to what the runtime reads and writes.
    pub fn resolve_mappings(&self, mappings: &[Mapping]) -> Result<Vec<ResolvedMapping>, StErrors> {
        let io = self.tables.len() - 1;
        let scope = Scope::new(&self.tables, &self.names, io).with_fbs(&self.interfaces);
        let errors = mapping::validate(mappings, &self.config, &scope, &self.tables[io]);
        if errors.is_empty() {
            Ok(mapping::resolve_all(mappings, &self.config, &scope))
        } else {
            Err(errors)
        }
    }
}

/// A compiled project: what the runtime loads and executes.
#[derive(Debug, Clone)]
pub struct CompiledProject {
    /// In execution order.
    pub programs: Vec<CompiledProgram>,
    /// Checked FUNCTION_BLOCKs, indexed by `VarType::UserFb`.
    pub function_blocks: Vec<FunctionBlockDef>,
    pub mappings: Vec<ResolvedMapping>,
    pub context: ProjectContext,
}

/// Why a project doesn't compile: each program's and each FUNCTION_BLOCK's errors, in
/// order (empty for those that are fine), and the I/O mapping errors.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectErrors {
    pub programs: Vec<StErrors>,
    pub function_blocks: Vec<StErrors>,
    pub mapping: StErrors,
}

/// Semantic analysis of parsed programs and FUNCTION_BLOCKs plus the I/O `mappings`, for
/// a PLC with the I/O `config`. Every error is collected.
pub fn compile_project(
    programs: Vec<(String, Program)>,
    fbs: Vec<FunctionBlockDef>,
    mappings: &[Mapping],
    config: &IoConfig,
) -> Result<CompiledProject, ProjectErrors> {
    let mapped_inputs = mapping::mapped_block_inputs(mappings, config);
    let ProjectCheck { programs: results, function_blocks, interfaces, tables, names } =
        check_project(programs, fbs, &config.symbols(), &mapped_inputs);
    let context = ProjectContext { config: *config, tables, names, interfaces };
    let resolved = context.resolve_mappings(mappings);
    let failed = results.iter().any(Result::is_err) || function_blocks.iter().any(Result::is_err) || resolved.is_err();
    if failed {
        return Err(ProjectErrors {
            programs: results.into_iter().map(|r| r.err().unwrap_or_default()).collect(),
            function_blocks: function_blocks.into_iter().map(|r| r.err().unwrap_or_default()).collect(),
            mapping: resolved.err().unwrap_or_default(),
        });
    }
    let programs = results
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(file, Checked { program, symbols })| CompiledProgram { name: context.names[file].clone(), program, symbols, file })
        .collect();
    let function_blocks = function_blocks.into_iter().flatten().map(|CheckedFb { def }| def).collect();
    Ok(CompiledProject { programs, function_blocks, mappings: resolved.unwrap_or_default(), context })
}

/// Compile project source files: parse each (FUNCTION_BLOCKs and at most one PROGRAM), then
/// analyze them together. Every file is compiled even when others fail, so all errors come
/// back at once; a file with a syntax error can't share its variables with the others.
/// Returns the compiled project if everything compiled, and the report either way.
pub fn compile_sources(sources: &[ProgramSource], mappings: &[Mapping], config: &IoConfig) -> (Option<CompiledProject>, CompileReport) {
    let n = sources.len();
    let mut errors: Vec<StErrors> = vec![StErrors::new(); n];
    let mut lines: Vec<Vec<usize>> = vec![Vec::new(); n];
    let mut variables: Vec<Vec<DeclaredVariable>> = vec![Vec::new(); n];
    let mut defined: Vec<Vec<FbSummary>> = vec![Vec::new(); n];
    let mut has_program = vec![false; n];
    let mut parsed = Vec::new();
    let mut index = Vec::new();
    // Every file's FUNCTION_BLOCKs, and the file each came from.
    let (mut fbs, mut fb_file) = (Vec::new(), Vec::new());
    for (i, source) in sources.iter().enumerate() {
        if sources[..i].iter().any(|p| p.name.eq_ignore_ascii_case(&source.name)) {
            errors[i].push(StError::general(format!("Program '{}' already exists.", source.name)));
        }
        match parse_file(&source.source) {
            Ok(file) => {
                defined[i] = file.function_blocks.iter().map(FbSummary::of).collect();
                fb_file.extend(std::iter::repeat_n(i, file.function_blocks.len()));
                // Breakpoints can stop in block bodies too.
                lines[i] = file.function_blocks.iter().flat_map(|fb| statement_lines(&fb.body)).collect();
                fbs.extend(file.function_blocks.into_iter().map(|fb| FunctionBlockDef { file: source.name.clone(), ..fb }));
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
                parsed.push((source.name.clone(), ast));
                index.push(i);
            }
            Err(e) => errors[i].push(e),
        }
    }
    let (project, mapping) = match compile_project(parsed, fbs, mappings, config) {
        Ok(mut project) => {
            for program in &mut project.programs {
                program.file = index[program.file]; // compiled-program index → source file
            }
            (Some(project), StErrors::new())
        }
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
    let programs = sources
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
    (project.filter(|_| ok), CompileReport { programs, mapping })
}
