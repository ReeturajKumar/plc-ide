//! The PLC runtime: executes a compiled project. It owns the runtime memory (variables,
//! timers, counters, function block instances), runs the scan cycle, reads inputs from and
//! writes outputs to an `IoProvider`, and holds the debugger state. It knows nothing about
//! the UI, the editor or the Tauri layer: a host (`myplc-runtime`'s) drives it and reports
//! its state.
//!
//! - `executor`: the ST interpreter and runtime memory
//! - `scan_cycle`: read inputs → programs → write outputs, on the PLC clock
//! - `debugger`: breakpoints, stopping and stepping
//! - `state`: the engine state the host can report

mod debugger;
mod executor;
pub mod function_blocks;
mod scan_cycle;
mod state;

use std::collections::HashMap;

pub use debugger::{Location, Stop};
pub use executor::ProgramError;
pub use scan_cycle::ScanOutcome;
pub use state::{EngineState, FunctionBlockState, MemberState, VariableState};

use debugger::DebuggerState;
use executor::{eval, instantiate, slot, Memory, Slots, Unit, UserFbs};
use scan_cycle::ScanCycle;

use crate::io::{IoConfig, MappingTarget, ResolvedMapping};
use crate::compiler::ast::VarType;
use crate::compiler::error::{StError, StErrors, StResult};
use crate::compiler::project::CompiledProject;
use crate::compiler::symbols::{key, Scope, SymbolTable};
use crate::compiler::value::Value;

/// A loaded, compiled project and its memory. Executes one scan at a time, running every
/// program once, in project order; when to scan is up to the host.
pub struct Runtime {
    units: Vec<Unit>,
    initial: Memory,
    memory: Memory,
    io_config: IoConfig,
    /// Index of the I/O address table: its slots hold the process image during a scan.
    io_index: usize,
    /// Mapped inputs (I/O slot → target), copied before the programs run.
    inputs: Vec<(String, Target)>,
    /// Mapped outputs (source → I/O slot), copied after the programs ran.
    outputs: Vec<(Target, String)>,
    /// Variable slot → the address it is mapped to.
    mapped: HashMap<String, String>,
    /// Source file names; a statement's file id indexes this.
    files: Vec<String>,
    fbs: UserFbs,
    debugger: DebuggerState,
    cycle: ScanCycle,
}

/// Where a mapping reads or writes: a memory slot (a variable, or a project block's
/// member), or a member of a standard block instance (kept inside the instance).
#[derive(Debug, Clone)]
enum Target {
    Slot(String),
    Builtin { instance: String, member: String },
}

impl Runtime {
    /// Load a compiled project: create every variable and function block instance with its
    /// initial value. An initial value that can't be evaluated is an error of its program:
    /// the result is then each program's errors, in order (empty for those that are fine).
    pub fn load(project: CompiledProject) -> Result<Self, Vec<StErrors>> {
        let CompiledProject { programs, function_blocks: defs, mappings, context } = project;
        // Source files: the programs' (named after their file), then the blocks' files.
        let mut files: Vec<String> = programs.iter().map(|p| p.name.clone()).collect();
        let file_ids = defs
            .iter()
            .map(|def| {
                let file = if def.file.is_empty() { def.name.clone() } else { def.file.clone() };
                files.iter().position(|f| *f == file).unwrap_or_else(|| {
                    files.push(file);
                    files.len() - 1
                })
            })
            .collect();
        let mut fbs = UserFbs { defs, interfaces: context.interfaces.clone(), instances: HashMap::new(), file_ids };

        let mut initial = Memory::default();
        let mut errors = vec![StErrors::new(); programs.len()];
        for (i, compiled) in programs.iter().enumerate() {
            for decl in &compiled.program.vars {
                match decl.var_type {
                    VarType::Elementary(data_type) => {
                        let value = match &decl.init {
                            // Initial values are constant expressions, so they need no memory.
                            Some(init) => match eval(init, &Memory::default(), &Slots::new()) {
                                Ok(value) => value,
                                Err(e) => {
                                    errors[i].push(e);
                                    continue;
                                }
                            },
                            None => Value::default_for(data_type),
                        };
                        initial.values.insert(slot(i, &decl.name), value);
                    }
                    VarType::FunctionBlock(kind) => {
                        initial.instances.insert(slot(i, &decl.name), kind.instantiate());
                    }
                    VarType::UserFb(index) => {
                        if let Err(e) = instantiate(index, &slot(i, &decl.name), &mut fbs, &mut initial) {
                            errors[i].push(e);
                        }
                    }
                }
            }
        }
        if errors.iter().any(|e| !e.is_empty()) {
            return Err(errors);
        }

        // The I/O addresses come after the programs, with their own slots.
        let io_index = programs.len();
        for (address, kind) in context.config.points() {
            initial.values.insert(slot(io_index, &address), Value::default_for(kind.data_type()));
        }
        // What each name in each program refers to: its own variable, another program's,
        // or an I/O address (the context's tables end with the I/O table).
        let slots: Vec<Slots> = (0..programs.len())
            .map(|i| {
                let scope = Scope::new(&context.tables, &context.names, i);
                context
                    .tables
                    .iter()
                    .flat_map(SymbolTable::iter)
                    .filter_map(|symbol| Some((key(&symbol.name), slot(scope.owner(&symbol.name)?, &symbol.name))))
                    .collect()
            })
            .collect();
        let units = programs
            .into_iter()
            .zip(slots)
            .map(|(compiled, slots)| Unit { name: compiled.name, body: compiled.program.body, symbols: compiled.symbols, slots })
            .collect();

        let mut runtime = Self {
            units,
            memory: initial.clone(),
            initial,
            io_config: context.config,
            io_index,
            inputs: Vec::new(),
            outputs: Vec::new(),
            mapped: HashMap::new(),
            files,
            fbs,
            debugger: DebuggerState::default(),
            cycle: ScanCycle::default(),
        };
        runtime.wire(&mappings);
        Ok(runtime)
    }

    /// Replace the I/O mappings (already checked by the compiler); they apply from the next
    /// scan. An output no longer mapped goes back to its safe value, unless a program writes it.
    pub fn set_mappings(&mut self, mappings: &[ResolvedMapping]) {
        self.wire(mappings);
        for (address, kind) in self.io_config.points().into_iter().filter(|(_, kind)| !kind.is_input()) {
            self.memory.values.insert(slot(self.io_index, &address), Value::default_for(kind.data_type()));
        }
    }

    fn wire(&mut self, mappings: &[ResolvedMapping]) {
        let (mut inputs, mut outputs, mut mapped) = (Vec::new(), Vec::new(), HashMap::new());
        for m in mappings {
            let address = m.kind.address(m.index);
            let target = match &m.target {
                MappingTarget::Variable { program, name } => {
                    let var_slot = slot(*program, name);
                    mapped.insert(var_slot.clone(), address.clone());
                    Target::Slot(var_slot)
                }
                MappingTarget::BlockMember { program, instance, member, standard: false } => {
                    Target::Slot(format!("{}.{}", slot(*program, instance), key(member)))
                }
                MappingTarget::BlockMember { program, instance, member, standard: true } => {
                    Target::Builtin { instance: slot(*program, instance), member: member.clone() }
                }
            };
            let io_slot = slot(self.io_index, &address);
            if m.kind.is_input() {
                inputs.push((io_slot, target));
            } else {
                outputs.push((target, io_slot));
            }
        }
        (self.inputs, self.outputs, self.mapped) = (inputs, outputs, mapped);
    }

    /// Program names in execution order.
    pub fn program_names(&self) -> Vec<String> {
        self.units.iter().map(|u| u.name.clone()).collect()
    }

    /// Restore every variable and function block to its initial state, drop a stopped scan,
    /// and restart the PLC clock and cycle count.
    pub fn reset(&mut self) {
        self.memory = self.initial.clone();
        self.debugger.suspended = None;
        self.reset_cycle();
    }

    /// Write a variable that `program` declares, from outside. The value must match its type.
    pub fn set_in(&mut self, program: &str, name: &str, value: Value) -> StResult<()> {
        let unit = self
            .units
            .iter()
            .position(|u| u.name == program)
            .ok_or_else(|| StError::general(format!("Unknown program '{program}'.")))?;
        let var_slot = slot(unit, name);
        let current = self
            .memory
            .values
            .get_mut(&var_slot)
            .ok_or_else(|| StError::general(format!("Unknown variable '{name}'.")))?;
        if current.data_type() != value.data_type() {
            return Err(StError::general(format!(
                "Type mismatch: '{name}' is {}, cannot set it to a {} value.",
                current.data_type().name(),
                value.data_type().name()
            )));
        }
        *current = value;
        self.record_edit(var_slot, value);
        Ok(())
    }
}

