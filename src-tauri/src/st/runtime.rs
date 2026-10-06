use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::ast::{DataType, Expr, ExprKind, FunctionBlockDef, Program, Stmt, UnaryOp, VarType};
use super::checker::{check_project, key, with_io, Checked, CheckedFb, ProjectCheck};
use super::error::{ErrorKind, StError, StErrors, StResult};
use super::function_blocks::{FbInstance, ScanClock};
use super::functions::StdFunction;
use super::symbols::{FbInterface, Scope, SymbolTable};
use super::value::Value;
use crate::io::{self, IoImage, Mapping};

/// Upper bound on loop iterations (FOR, WHILE, REPEAT combined) in one scan. A PLC scan
/// must finish; past this the program stops with an error instead of freezing the IDE.
pub const MAX_LOOP_ITERATIONS_PER_SCAN: u64 = 100_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VariableSnapshot {
    /// The program that declares it.
    pub program: String,
    pub name: String,
    pub data_type: String,
    pub value: Value,
    /// Never assigned by the program, so its value comes from outside (the simulator UI).
    pub is_input: bool,
    /// The I/O address it is mapped to, if any.
    pub io: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FunctionBlockSnapshot {
    pub program: String,
    pub name: String,
    pub block_type: String,
    pub members: Vec<MemberSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemberSnapshot {
    pub name: String,
    pub data_type: String,
    pub value: Value,
    pub is_output: bool,
    /// A user block's internal variable (read-only, shown for debugging).
    pub is_internal: bool,
}

/// Everything the project remembers between scans: plain variables and the internal
/// state of function block instances, keyed by slot (see `slot`).
#[derive(Debug, Clone, Default)]
struct Memory {
    values: HashMap<String, Value>,
    instances: HashMap<String, FbInstance>,
}

/// Upper-cased name → memory slot, for every name one program can use.
type Slots = HashMap<String, String>;

/// Where program `index` keeps its variable `name`. Each program has its own slots, so
/// same-named variables (and FB instances) of different programs stay independent.
fn slot(index: usize, name: &str) -> String {
    format!("{index}.{}", key(name))
}

impl Memory {
    fn value(&self, slots: &Slots, name: &str) -> Option<Value> {
        self.values.get(slots.get(&key(name))?).copied()
    }

    fn value_mut(&mut self, slots: &Slots, name: &str) -> Option<&mut Value> {
        self.values.get_mut(slots.get(&key(name))?)
    }

    fn instance(&self, slots: &Slots, name: &str) -> Option<&FbInstance> {
        self.instances.get(slots.get(&key(name))?)
    }

    fn instance_mut(&mut self, slots: &Slots, name: &str) -> Option<&mut FbInstance> {
        self.instances.get_mut(slots.get(&key(name))?)
    }
}

/// One checked program, as the runtime executes it.
struct Unit {
    name: String,
    body: Vec<Stmt>,
    symbols: SymbolTable,
    /// Its own variables plus the ones it shares from other programs.
    slots: Slots,
}

/// The type-checked programs of a project plus their memory. Executes one scan at a time,
/// running every program once, in project order; the scan loop and PLC clock live in
/// the simulator.
pub struct Runtime {
    units: Vec<Unit>,
    initial: Memory,
    memory: Memory,
    /// Index of the I/O address table: its slots hold the process image during a scan.
    io_index: usize,
    /// Mapped inputs (I/O slot → target), copied before the programs run.
    inputs: Vec<(String, Target)>,
    /// Mapped outputs (source → I/O slot), copied after the programs ran.
    outputs: Vec<(Target, String)>,
    /// Variable slot → the address it is mapped to, for the monitor.
    mapped: HashMap<String, String>,
    /// Enabled breakpoints as (file id, line).
    breakpoints: HashSet<(usize, usize)>,
    /// Source file names; a statement's file id indexes this.
    files: Vec<String>,
    /// The scan in progress, if the debugger stopped it.
    suspended: Option<Suspended>,
    fbs: UserFbs,
}

/// The project's FUNCTION_BLOCKs as the runtime executes them. An instance's members live
/// in the shared memory under the instance's slot ("0.MOTOR1.START", …), so every instance
/// has its own state, which persists between scans like any variable.
#[derive(Default)]
struct UserFbs {
    /// Checked definitions (bodies lowered), indexed by `VarType::UserFb`.
    defs: Vec<FunctionBlockDef>,
    interfaces: Vec<FbInterface>,
    /// Instance slot → (block index, slots of its members by name).
    instances: HashMap<String, (usize, Slots)>,
    /// Each block's source file id (see `Runtime::files`).
    file_ids: Vec<usize>,
}

/// What a statement runs in, for the debugger: its source file, the program whose scan
/// is running, and the user block instance (if any) executing it.
#[derive(Debug, Clone)]
struct Ctx {
    file: usize,
    program: usize,
    fb: Option<usize>,
    instance: Option<String>,
}

/// Create the memory of an instance of user block `index` at `prefix` (its slot): each
/// member gets its initial value, nested blocks their own instances.
fn instantiate(index: usize, prefix: &str, fbs: &mut UserFbs, memory: &mut Memory) -> StResult<()> {
    let mut slots = Slots::new();
    let members: Vec<VarDeclInfo> = fbs.defs[index].members().map(VarDeclInfo::from).collect();
    for member in members {
        let member_slot = format!("{prefix}.{}", key(&member.name));
        slots.insert(key(&member.name), member_slot.clone());
        match member.var_type {
            VarType::Elementary(data_type) => {
                let value = match &member.init {
                    Some(init) => eval(init, &Memory::default(), &Slots::new())?,
                    None => Value::default_for(data_type),
                };
                memory.values.insert(member_slot, value);
            }
            VarType::FunctionBlock(kind) => {
                memory.instances.insert(member_slot, kind.instantiate());
            }
            VarType::UserFb(nested) => instantiate(nested, &member_slot, fbs, memory)?,
        }
    }
    fbs.instances.insert(prefix.to_string(), (index, slots));
    Ok(())
}

/// What `instantiate` needs of a declaration, owned so `fbs` can be updated meanwhile.
struct VarDeclInfo {
    name: String,
    var_type: VarType,
    init: Option<Expr>,
}

impl From<&super::ast::VarDecl> for VarDeclInfo {
    fn from(decl: &super::ast::VarDecl) -> Self {
        Self { name: decl.name.clone(), var_type: decl.var_type, init: decl.init.clone() }
    }
}

/// Where the debugger stopped: the statement about to run.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    /// The program whose scan is running (the caller, when inside a function block).
    pub program: String,
    /// The source file of the statement: the program's, or the function block's.
    pub file: String,
    pub line: usize,
    pub column: usize,
    /// Set when the statement is inside a user function block.
    pub function_block: Option<String>,
    /// The instance running it, e.g. "Counter1" (nested: "Station1.Blinker").
    pub instance: Option<String>,
}

/// When a debugged scan stops before a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Run the whole scan.
    Never,
    /// At an enabled breakpoint (once per line, so a line with several statements stops once).
    AtBreakpoint,
    /// Before the very next statement (STEP STATEMENT).
    NextStatement,
}

/// A scan stopped at a breakpoint or step, waiting to go on.
///
/// The interpreter isn't resumable mid-statement, so going on replays the scan from its
/// start: a scan is deterministic (same memory, inputs and clock give the same result),
/// so re-running the first `paused_at - 1` statements reproduces exactly the state the
/// user inspected, then execution carries on. PLC scans are short, so this costs little.
struct Suspended {
    /// Memory when the scan began (inputs already read).
    start: Memory,
    clock: ScanClock,
    /// Number of the statement it stopped before (1-based, counted across the scan).
    paused_at: u64,
    location: Location,
    /// Values the user set while stopped, with the statement they were set before; the
    /// replay applies each one when it reaches that statement again.
    edits: Edits,
}

/// (statement number, slot, value): see `Suspended::edits`.
type Edits = Vec<(u64, String, Value)>;

/// A scan failed in `program`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramError {
    pub program: String,
    pub error: StError,
}

/// Why a project can't run: each program's and each FUNCTION_BLOCK's errors, in order
/// (empty for those that are fine), and the I/O mapping errors.
#[derive(Debug, Clone, PartialEq)]
pub struct ProjectErrors {
    pub programs: Vec<StErrors>,
    pub function_blocks: Vec<StErrors>,
    pub mapping: StErrors,
}

impl Runtime {
    /// A single-program project. A program with any semantic error is refused here, so it
    /// can never execute.
    #[cfg(test)]
    pub fn new(program: Program) -> Result<Self, StErrors> {
        let name = program.name.clone();
        Self::project(vec![(name, program)], &[]).map_err(|mut errors| errors.programs.remove(0))
    }

    /// A project without FUNCTION_BLOCKs.
    #[cfg(test)]
    pub fn project(programs: Vec<(String, Program)>, mappings: &[Mapping]) -> Result<Self, ProjectErrors> {
        Self::build(programs, Vec::new(), mappings)
    }

    /// Run semantic analysis on every FUNCTION_BLOCK and program, check the I/O `mappings`,
    /// and create the variables (and block instances) with their initial values. With any
    /// error nothing is created.
    pub fn build(programs: Vec<(String, Program)>, fbs: Vec<FunctionBlockDef>, mappings: &[Mapping]) -> Result<Self, ProjectErrors> {
        let names: Vec<String> = programs.iter().map(|(name, _)| name.clone()).collect();
        let ProjectCheck { programs: results, function_blocks, mapping, interfaces } = check_project(programs, fbs, mappings);
        if results.iter().any(Result::is_err) || function_blocks.iter().any(Result::is_err) || !mapping.is_empty() {
            let programs = results.into_iter().map(|r| r.err().unwrap_or_default()).collect();
            let function_blocks = function_blocks.into_iter().map(|r| r.err().unwrap_or_default()).collect();
            return Err(ProjectErrors { programs, function_blocks, mapping });
        }
        let defs: Vec<FunctionBlockDef> = function_blocks.into_iter().flatten().map(|CheckedFb { def, .. }| def).collect();
        // Source files: the programs' (named after their file), then the blocks' files.
        let mut files = names.clone();
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
        let mut fbs = UserFbs { defs, interfaces, instances: HashMap::new(), file_ids };
        let (programs, tables): (Vec<Program>, Vec<SymbolTable>) = results
            .into_iter()
            .flatten()
            .map(|Checked { program, symbols }| (program, symbols))
            .unzip();

        let mut initial = Memory::default();
        let mut errors = vec![StErrors::new(); programs.len()];
        for (i, program) in programs.iter().enumerate() {
            for decl in &program.vars {
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
            let function_blocks = vec![StErrors::new(); fbs.defs.len()];
            return Err(ProjectErrors { programs: errors, function_blocks, mapping: StErrors::new() });
        }

        // The I/O addresses are one more table (after the programs'), with their own slots.
        let (mut tables, names) = with_io(tables, names);
        let io_index = tables.len() - 1;
        for symbol in tables[io_index].iter() {
            let data_type = symbol.data_type().unwrap_or(DataType::Bool);
            initial.values.insert(slot(io_index, &symbol.name), Value::default_for(data_type));
        }
        let slots: Vec<Slots> = (0..programs.len())
            .map(|i| {
                let scope = Scope::new(&tables, &names, i);
                tables
                    .iter()
                    .flat_map(SymbolTable::iter)
                    .filter_map(|symbol| Some((key(&symbol.name), slot(scope.owner(&symbol.name)?, &symbol.name))))
                    .collect()
            })
            .collect();

        let (inputs, outputs, mapped) = resolve_mappings(mappings, &tables, &names, &fbs.interfaces);

        tables.pop();
        let units = programs
            .into_iter()
            .zip(tables)
            .zip(slots)
            .zip(names)
            .map(|(((program, symbols), slots), name)| Unit { name, body: program.body, symbols, slots })
            .collect();

        let memory = initial.clone();
        Ok(Self { units, initial, memory, io_index, inputs, outputs, mapped, breakpoints: HashSet::new(), files, suspended: None, fbs })
    }

    /// Replace the I/O mappings of the loaded project (they apply from the next scan).
    /// With any error nothing changes and the errors are returned.
    pub fn set_mappings(&mut self, mappings: &[Mapping]) -> StErrors {
        let tables: Vec<SymbolTable> = self.units.iter().map(|u| u.symbols.clone()).collect();
        let names: Vec<String> = self.units.iter().map(|u| u.name.clone()).collect();
        let (tables, names) = with_io(tables, names);
        let io_index = tables.len() - 1;
        let scope = Scope::new(&tables, &names, io_index).with_fbs(&self.fbs.interfaces);
        let errors = io::validate(mappings, &io::CONFIG, &scope, &tables[io_index]);
        if errors.is_empty() {
            (self.inputs, self.outputs, self.mapped) = resolve_mappings(mappings, &tables, &names, &self.fbs.interfaces);
            // An output no longer mapped goes back to its safe value, unless a program writes it.
            for symbol in tables[io_index].iter().filter(|s| !s.read_only) {
                let value = Value::default_for(symbol.data_type().unwrap_or(DataType::Bool));
                self.memory.values.insert(slot(io_index, &symbol.name), value);
            }
        }
        errors
    }

    /// READ INPUTS, the first step of a scan: take the input image, then copy it into the
    /// mapped variables. Programs see these values for the whole scan.
    pub fn read_inputs(&mut self, image: &IoImage) {
        for point in image.points().iter().filter(|p| p.kind.is_input()) {
            self.memory.values.insert(slot(self.io_index, &point.address), point.value);
        }
        for (io_slot, target) in &self.inputs {
            let Some(&value) = self.memory.values.get(io_slot) else { continue };
            match target {
                Target::Slot(slot) => {
                    self.memory.values.insert(slot.clone(), value);
                }
                Target::Builtin { instance, member } => {
                    if let Some(fb) = self.memory.instances.get_mut(instance) {
                        let _ = fb.set_input(member, value); // type checked by the mapping validation
                    }
                }
            }
        }
    }

    /// WRITE OUTPUTS, the last step of a scan: copy the mapped variables to their outputs,
    /// then publish every output to the image.
    pub fn write_outputs(&mut self, image: &mut IoImage) {
        for (source, io_slot) in &self.outputs {
            let value = match source {
                Target::Slot(slot) => self.memory.values.get(slot).copied(),
                Target::Builtin { instance, member } => self.memory.instances.get(instance).and_then(|fb| fb.get(member)),
            };
            if let Some(value) = value {
                self.memory.values.insert(io_slot.clone(), value);
            }
        }
        let outputs: Vec<String> = image.points().iter().filter(|p| !p.kind.is_input()).map(|p| p.address.clone()).collect();
        for address in outputs {
            if let Some(&value) = self.memory.values.get(&slot(self.io_index, &address)) {
                image.set_output(&address, value);
            }
        }
    }

    /// Program names in execution order.
    pub fn program_names(&self) -> Vec<String> {
        self.units.iter().map(|u| u.name.clone()).collect()
    }

    /// Restore every variable and function block to its initial state (and drop a
    /// stopped scan).
    pub fn reset(&mut self) {
        self.memory = self.initial.clone();
        self.suspended = None;
    }

    /// The EXECUTE step of a PLC scan: every program once, in project order. Timers measure
    /// against `clock`, which the simulator advances by one scan interval per scan. The
    /// loop budget is shared by the whole scan, since the whole scan must finish.
    #[cfg(test)]
    pub fn scan(&mut self, clock: ScanClock) -> Result<(), ProgramError> {
        self.run(clock, Stop::Never).map(|_| ())
    }

    /// Execute a scan under the debugger. Returns where it stopped, or None if the scan
    /// completed. A stopped scan is finished with `proceed`.
    pub fn run(&mut self, clock: ScanClock, stop: Stop) -> Result<Option<Location>, ProgramError> {
        self.suspended = None;
        // Only a scan that may stop needs its starting memory, to replay it.
        let start = if stop == Stop::Never { Memory::default() } else { self.memory.clone() };
        self.finish(start, clock, 0, stop, Edits::new())
    }

    /// Continue the stopped scan from the statement it stopped at (which now runs), until
    /// `stop` or the end of the scan. Nothing is reset: the replay rebuilds the paused state.
    pub fn proceed(&mut self, stop: Stop) -> Result<Option<Location>, ProgramError> {
        let Some(suspended) = self.suspended.take() else { return Ok(None) };
        self.memory = suspended.start.clone();
        self.finish(suspended.start, suspended.clock, suspended.paused_at, stop, suspended.edits)
    }

    fn finish(&mut self, start: Memory, clock: ScanClock, skip: u64, stop: Stop, edits: Edits) -> Result<Option<Location>, ProgramError> {
        let mut trace = Trace { breakpoints: &self.breakpoints, stop, skip, edits: &edits, executed: 0, last: None, hit: None };
        let mut iterations = 0;
        for (unit_index, unit) in self.units.iter().enumerate() {
            // A program's file id is its index: programs come first in `files`.
            let ctx = Ctx { file: unit_index, program: unit_index, fb: None, instance: None };
            let mut exec = Exec { memory: &mut self.memory, slots: &unit.slots, clock, iterations, ctx, trace: &mut trace, fbs: &self.fbs };
            let result = exec.block(&unit.body);
            iterations = exec.iterations;
            if let Some((ctx, line, column)) = trace.hit.take() {
                let location = Location {
                    program: self.units[ctx.program].name.clone(),
                    file: self.files[ctx.file].clone(),
                    line,
                    column,
                    function_block: ctx.fb.map(|fb| self.fbs.defs[fb].name.clone()),
                    instance: ctx.instance,
                };
                let paused_at = trace.executed;
                self.suspended = Some(Suspended { start, clock, paused_at, location: location.clone(), edits });
                return Ok(Some(location));
            }
            result.map_err(|error| ProgramError { program: unit.name.clone(), error })?;
        }
        Ok(None)
    }

    /// Where the debugger stopped the current scan.
    pub fn location(&self) -> Option<&Location> {
        self.suspended.as_ref().map(|s| &s.location)
    }

    /// Replace the breakpoints: (source file name, line). A breakpoint in a function block's
    /// file stops in whichever instance reaches it. Unknown files are ignored, and so are
    /// lines without a statement (execution never stops there).
    pub fn set_breakpoints(&mut self, breakpoints: &[(String, usize)]) {
        self.breakpoints = breakpoints
            .iter()
            .filter_map(|(file, line)| Some((self.files.iter().position(|f| f == file)?, *line)))
            .collect();
    }

    pub fn has_breakpoints(&self) -> bool {
        !self.breakpoints.is_empty()
    }

    #[cfg(test)]
    pub fn get(&self, name: &str) -> Option<Value> {
        self.memory.value(&self.units[0].slots, name)
    }

    /// Read a function block output or input of the first program, e.g. `member("RunTimer", "Q")`.
    #[cfg(test)]
    pub fn member(&self, instance: &str, member: &str) -> Option<Value> {
        self.memory.instance(&self.units[0].slots, instance).and_then(|fb| fb.get(member))
    }

    #[cfg(test)]
    pub fn set(&mut self, name: &str, value: Value) -> StResult<()> {
        let program = self.units[0].name.clone();
        self.set_in(&program, name, value)
    }

    /// Write a variable that `program` declares, from outside. The value must match its type.
    pub fn set_in(&mut self, program: &str, name: &str, value: Value) -> StResult<()> {
        let unit = self
            .units
            .iter()
            .position(|u| u.name == program)
            .ok_or_else(|| StError::general(format!("Unknown program '{program}'.")))?;
        let slot = self
            .memory
            .values
            .get_mut(&slot(unit, name))
            .ok_or_else(|| StError::general(format!("Unknown variable '{name}'.")))?;
        if slot.data_type() != value.data_type() {
            return Err(StError::general(format!(
                "Type mismatch: '{name}' is {}, cannot set it to a {} value.",
                slot.data_type().name(),
                value.data_type().name()
            )));
        }
        *slot = value;
        // While stopped mid-scan, going on replays the scan: redo the edit at this point.
        if let Some(suspended) = self.suspended.as_mut() {
            suspended.edits.push((suspended.paused_at, self::slot(unit, name), value));
        }
        Ok(())
    }

    /// Plain (non function block) variables, by program in order, for the monitor.
    pub fn variables(&self) -> Vec<VariableSnapshot> {
        let mut out = Vec::new();
        for (i, unit) in self.units.iter().enumerate() {
            for symbol in unit.symbols.iter() {
                let Some(data_type) = symbol.data_type() else { continue };
                out.push(VariableSnapshot {
                    program: unit.name.clone(),
                    name: symbol.name.clone(),
                    data_type: data_type.name().to_string(),
                    value: self.memory.values.get(&slot(i, &symbol.name)).copied().unwrap_or(Value::default_for(data_type)),
                    // Never written by any program, so its value comes from outside (the simulator UI).
                    is_input: !symbol.is_written(),
                    io: self.mapped.get(&slot(i, &symbol.name)).cloned(),
                });
            }
        }
        out
    }

    /// Function block instances, by program in order, with every member's value.
    pub fn function_blocks(&self) -> Vec<FunctionBlockSnapshot> {
        let mut out = Vec::new();
        for (i, unit) in self.units.iter().enumerate() {
            for symbol in unit.symbols.iter() {
                if let VarType::UserFb(index) = symbol.var_type {
                    out.push(self.user_instance(&unit.name, &symbol.name, &slot(i, &symbol.name), index));
                    continue;
                }
                let Some(kind) = symbol.block_type() else { continue };
                let fb = self.memory.instances.get(&slot(i, &symbol.name));
                out.push(FunctionBlockSnapshot {
                    program: unit.name.clone(),
                    name: symbol.name.clone(),
                    block_type: kind.name().to_string(),
                    members: kind
                        .members()
                        .map(|(member, data_type)| MemberSnapshot {
                            name: member.to_string(),
                            data_type: data_type.name().to_string(),
                            value: fb.and_then(|fb| fb.get(member)).unwrap_or(Value::default_for(data_type)),
                            is_output: kind.is_output(member),
                            is_internal: false,
                        })
                        .collect(),
                });
            }
        }
        out
    }

    /// A user block instance for the monitor: its inputs and outputs. Internal variables
    /// stay private to the block, as in the language.
    fn user_instance(&self, program: &str, name: &str, prefix: &str, index: usize) -> FunctionBlockSnapshot {
        let fb = &self.fbs.interfaces[index];
        let member = |member: &str, data_type: DataType, is_output: bool| MemberSnapshot {
            name: member.to_string(),
            data_type: data_type.name().to_string(),
            value: self.memory.values.get(&format!("{prefix}.{}", key(member))).copied().unwrap_or(Value::default_for(data_type)),
            is_output,
            is_internal: false,
        };
        // Internal variables are shown read-only (for debugging); programs still can't use them.
        let internals = self.fbs.defs[index].vars.iter().filter_map(|decl| match decl.var_type {
            VarType::Elementary(t) => Some(MemberSnapshot { is_internal: true, ..member(&decl.name, t, false) }),
            _ => None,
        });
        FunctionBlockSnapshot {
            program: program.to_string(),
            name: name.to_string(),
            block_type: fb.name.clone(),
            members: fb
                .inputs
                .iter()
                .map(|(n, t, _)| member(n, *t, false))
                .chain(fb.outputs.iter().map(|(n, t)| member(n, *t, true)))
                .chain(internals)
                .collect(),
        }
    }
}

/// Where a mapping reads or writes: a memory slot (a variable, or a user block's member),
/// or a member of a standard block instance (kept inside the instance).
#[derive(Debug, Clone)]
enum Target {
    Slot(String),
    Builtin { instance: String, member: String },
}

/// Mapped inputs (I/O slot → target), mapped outputs (source → I/O slot) and
/// variable slot → address, for validated `mappings`. `tables` ends with the I/O table.
type ResolvedMappings = (Vec<(String, Target)>, Vec<(Target, String)>, HashMap<String, String>);

fn resolve_mappings(mappings: &[Mapping], tables: &[SymbolTable], names: &[String], fbs: &[FbInterface]) -> ResolvedMappings {
    let io_index = tables.len() - 1;
    let scope = Scope::new(tables, names, io_index).with_fbs(fbs);
    let (mut inputs, mut outputs, mut mapped) = (Vec::new(), Vec::new(), HashMap::new());
    for m in mappings {
        let Some(kind) = io::CONFIG.kind_of(&m.address) else { continue };
        let target = match io::resolve(&m.variable, &scope) {
            Ok(io::Endpoint::Variable { owner, symbol }) => {
                let var_slot = slot(owner, &symbol.name);
                mapped.insert(var_slot.clone(), key(&m.address));
                Target::Slot(var_slot)
            }
            Ok(io::Endpoint::Member { owner, instance, member, user: true, .. }) => {
                Target::Slot(format!("{}.{}", slot(owner, &instance.name), key(&member)))
            }
            Ok(io::Endpoint::Member { owner, instance, member, user: false, .. }) => {
                Target::Builtin { instance: slot(owner, &instance.name), member }
            }
            _ => continue, // refused by the validation
        };
        let io_slot = slot(io_index, &m.address);
        if kind.is_input() {
            inputs.push((io_slot, target));
        } else {
            outputs.push((target, io_slot));
        }
    }
    (inputs, outputs, mapped)
}

// --- execution ---

/// Debugger bookkeeping for one scan: counts statements and decides where to stop.
struct Trace<'a> {
    breakpoints: &'a HashSet<(usize, usize)>,
    stop: Stop,
    /// Statements that run without stopping: those replayed up to and including the one
    /// the scan last stopped at.
    skip: u64,
    edits: &'a [(u64, String, Value)],
    executed: u64,
    /// (file, line) of the previous statement.
    last: Option<(usize, usize)>,
    /// Where it stopped.
    hit: Option<(Ctx, usize, usize)>,
}

impl Trace<'_> {
    /// Called before every statement; an error unwinds the scan to stop there.
    fn enter(&mut self, ctx: &Ctx, (line, column): (usize, usize)) -> StResult<()> {
        self.executed += 1;
        let new_line = self.last != Some((ctx.file, line));
        self.last = Some((ctx.file, line));
        let stop = self.executed > self.skip
            && match self.stop {
                Stop::Never => false,
                Stop::NextStatement => true,
                Stop::AtBreakpoint => new_line && self.breakpoints.contains(&(ctx.file, line)),
            };
        if stop {
            self.hit = Some((ctx.clone(), line, column));
            // Not a failure: `finish` sees `hit` and suspends the scan.
            return Err(StError::general("debugger stop"));
        }
        Ok(())
    }
}

/// Executes one scan: statements, loops (within the iteration budget) and block calls.
struct Exec<'a, 't> {
    memory: &'a mut Memory,
    slots: &'a Slots,
    ctx: Ctx,
    trace: &'a mut Trace<'t>,
    fbs: &'a UserFbs,
    clock: ScanClock,
    iterations: u64,
}

impl Exec<'_, '_> {
    fn block(&mut self, body: &[Stmt]) -> StResult<()> {
        body.iter().try_for_each(|stmt| self.stmt(stmt))
    }

    /// Count one loop iteration against the scan's budget.
    fn iteration(&mut self, line: usize, column: usize) -> StResult<()> {
        self.iterations += 1;
        if self.iterations > MAX_LOOP_ITERATIONS_PER_SCAN {
            return Err(StError::new(ErrorKind::Runtime, 
                line,
                column,
                format!(
                    "Loop limit exceeded: more than {MAX_LOOP_ITERATIONS_PER_SCAN} loop iterations in one scan (infinite loop?)"
                ),
            ));
        }
        Ok(())
    }

    fn bool(&self, expr: &Expr) -> StResult<bool> {
        eval(expr, self.memory, self.slots)?
            .as_bool()
            .ok_or_else(|| StError::new(ErrorKind::Runtime, expr.line, expr.column, "Condition must be BOOL"))
    }

    fn integer(&self, expr: &Expr) -> StResult<i64> {
        eval(expr, self.memory, self.slots)?
            .as_integer()
            .ok_or_else(|| StError::new(ErrorKind::Runtime, expr.line, expr.column, "Expected an INT or DINT value"))
    }

    fn assign(&mut self, target: &str, value: Value, line: usize, column: usize) -> StResult<()> {
        match self.memory.value_mut(self.slots, target) {
            Some(slot) => {
                *slot = value;
                Ok(())
            }
            None => Err(StError::new(ErrorKind::Runtime, line, column, format!("Undeclared variable '{target}'"))),
        }
    }

    fn stmt(&mut self, stmt: &Stmt) -> StResult<()> {
        self.trace.enter(&self.ctx, stmt.location())?;
        let edits = self.trace.edits;
        for (_, slot, value) in edits.iter().filter(|(at, _, _)| *at == self.trace.executed) {
            self.memory.values.insert(slot.clone(), *value);
        }
        match stmt {
            Stmt::Assign { target, value, line, column } => {
                let result = eval(value, self.memory, self.slots)?;
                self.assign(target, result, *line, *column)
            }
            Stmt::If { branches, else_branch, .. } => {
                for branch in branches {
                    if self.bool(&branch.condition)? {
                        return self.block(&branch.body);
                    }
                }
                self.block(else_branch)
            }
            Stmt::For { var, start, end, step, body, line, column } => {
                let var_type = self
                    .memory
                    .value(self.slots, var)
                    .map(|v| v.data_type())
                    .ok_or_else(|| StError::new(ErrorKind::Runtime, *line, *column, format!("Undeclared variable '{var}'")))?;
                // Bounds and step are evaluated once, when the loop starts.
                let mut i = self.integer(start)?;
                let end = self.integer(end)?;
                let step = match step {
                    Some(step) => self.integer(step)?,
                    None => 1,
                };
                if step == 0 {
                    return Err(StError::new(ErrorKind::Runtime, *line, *column, "FOR step is 0: the loop would never end"));
                }
                loop {
                    // Store before testing, so after the loop the variable holds the first
                    // value past the end (FOR i := 1 TO 5 leaves i = 6), as in IEC.
                    let value = Value::integer(i, var_type)
                        .map_err(|e| StError::new(ErrorKind::Runtime, *line, *column, format!("FOR loop variable '{var}': {e}")))?;
                    self.assign(var, value, *line, *column)?;
                    if (step > 0 && i > end) || (step < 0 && i < end) {
                        return Ok(());
                    }
                    self.iteration(*line, *column)?;
                    self.block(body)?;
                    i += step;
                }
            }
            Stmt::While { condition, body, line, column } => {
                while self.bool(condition)? {
                    self.iteration(*line, *column)?;
                    self.block(body)?;
                }
                Ok(())
            }
            Stmt::Repeat { body, condition, line, column } => loop {
                self.iteration(*line, *column)?;
                self.block(body)?;
                if self.bool(condition)? {
                    return Ok(());
                }
            },
            Stmt::Case { selector, branches, else_branch, .. } => {
                let value = self.integer(selector)?;
                let body = branches
                    .iter()
                    .find(|b| b.labels.iter().any(|l| l.value == value))
                    .map_or(else_branch, |b| &b.body);
                self.block(body)
            }
            Stmt::Call { instance, args, line, column } => {
                // Evaluate every argument before touching the instance, as IEC specifies.
                let inputs = args
                    .iter()
                    .map(|arg| Ok((arg, eval(&arg.value, self.memory, self.slots)?)))
                    .collect::<StResult<Vec<_>>>()?;
                if let Some(fb) = self.memory.instance_mut(self.slots, instance) {
                    for (arg, value) in inputs {
                        fb.set_input(&arg.name, value).map_err(|m| StError::new(ErrorKind::Runtime, arg.line, arg.column, m))?;
                    }
                    fb.execute(self.clock);
                    return Ok(());
                }
                // A user block: bind the inputs, then run its body on the instance's own
                // members, inside this scan, and come back here.
                let fbs = self.fbs;
                let (index, slots) = self.slots.get(&key(instance)).and_then(|prefix| fbs.instances.get(prefix)).ok_or_else(|| {
                    StError::new(ErrorKind::Runtime, *line, *column, format!("'{instance}' is not a function block instance"))
                })?;
                for (arg, value) in inputs {
                    if let Some(input) = slots.get(&key(&arg.name)) {
                        self.memory.values.insert(input.clone(), value);
                    }
                }
                let def = &fbs.defs[*index];
                let path = match &self.ctx.instance {
                    Some(outer) => format!("{outer}.{instance}"),
                    None => instance.clone(),
                };
                let ctx = Ctx { file: fbs.file_ids[*index], program: self.ctx.program, fb: Some(*index), instance: Some(path) };
                let mut body = Exec {
                    memory: &mut *self.memory,
                    slots,
                    ctx,
                    trace: &mut *self.trace,
                    clock: self.clock,
                    iterations: self.iterations,
                    fbs,
                };
                let result = body.block(&def.body);
                self.iterations = body.iterations;
                result.map_err(|e| StError { message: format!("{} (in function block {})", e.message, def.name), ..e })
            }
            // The checker rejects these; reaching one means it was skipped.
            Stmt::AssignMember { line, column, .. } => {
                Err(StError::new(ErrorKind::Runtime, *line, *column, "Function block members can't be assigned"))
            }
        }
    }
}

fn eval(expr: &Expr, memory: &Memory, slots: &Slots) -> StResult<Value> {
    let at = |message: String| StError::new(ErrorKind::Runtime, expr.line, expr.column, message);
    match &expr.kind {
        ExprKind::Const(v) => Ok(*v),
        ExprKind::Variable(name) => memory
            .value(slots, name)
            .ok_or_else(|| at(format!("Undeclared variable '{name}'"))),
        ExprKind::Member { instance, member } => memory
            .instance(slots, instance)
            .and_then(|fb| fb.get(member))
            // A user block instance: its members are slots under the instance's slot.
            .or_else(|| {
                let prefix = slots.get(&key(instance))?;
                memory.values.get(&format!("{prefix}.{}", key(member))).copied()
            })
            .ok_or_else(|| at(format!("Unknown member '{instance}.{member}'"))),
        ExprKind::Convert { to, operand } => eval(operand, memory, slots)?.convert(*to).map_err(at),
        ExprKind::Call { name, args } => {
            let function = StdFunction::from_name(name).ok_or_else(|| at(format!("Unknown function '{name}'")))?;
            let values = args.iter().map(|a| eval(a, memory, slots)).collect::<StResult<Vec<_>>>()?;
            function.call(&values).map_err(at)
        }
        ExprKind::Unary { op: UnaryOp::Not, operand } => eval(operand, memory, slots)?.not().map_err(at),
        ExprKind::Unary { op: UnaryOp::Neg, operand } => eval(operand, memory, slots)?.negate().map_err(at),
        ExprKind::Binary { op, left, right } => {
            let (l, r) = (eval(left, memory, slots)?, eval(right, memory, slots)?);
            Value::binary(*op, l, r).map_err(at)
        }
        // The checker replaces every numeric literal; reaching one means it was skipped.
        ExprKind::IntLiteral(_) | ExprKind::RealLiteral(_) => {
            Err(at("Internal error: untyped literal reached the runtime".to_string()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::st::parser::parse;

    /// For programs without timers, where PLC time is irrelevant.
    const NO_TIME: ScanClock = ScanClock { now_ms: 0, cycle_ms: 0 };

    /// A 100 ms scan ending at `now_ms`.
    fn at(now_ms: i64) -> ScanClock {
        ScanClock { now_ms, cycle_ms: 100 }
    }

    const MOTOR: &str = "PROGRAM main
VAR
    Start : BOOL := FALSE;
    Motor : BOOL := FALSE;
END_VAR

IF Start THEN
    Motor := TRUE;
ELSE
    Motor := FALSE;
END_IF;

END_PROGRAM";

    fn load(src: &str) -> Runtime {
        Runtime::new(parse(src).unwrap()).unwrap()
    }

    fn get(rt: &Runtime, name: &str) -> Value {
        rt.get(name).unwrap()
    }

    fn flag(rt: &Runtime, name: &str) -> bool {
        get(rt, name).as_bool().unwrap()
    }

    /// Run `body` once against the acceptance-test variables and return Result.
    fn result_of(body: &str) -> Value {
        let mut rt = load(&format!(
            "PROGRAM main VAR Counter : INT := 10; Speed : INT := 20; Result : INT := 0; END_VAR {body} END_PROGRAM"
        ));
        rt.scan(NO_TIME).unwrap();
        get(&rt, "Result")
    }

    /// Evaluate a single expression by assigning it to a variable of `ty`.
    fn eval_as(ty: &str, expr: &str) -> StResult<Value> {
        let program = parse(&format!("PROGRAM p VAR x : {ty}; END_VAR x := {expr}; END_PROGRAM"))?;
        let mut rt = Runtime::new(program).map_err(|mut errors| errors.remove(0))?;
        rt.scan(NO_TIME).map_err(|e| e.error)?;
        Ok(get(&rt, "x"))
    }

    // --- BOOL ---

    #[test]
    fn motor_program_follows_start_input() {
        let mut rt = load(MOTOR);
        assert!(!flag(&rt, "Start"));
        assert!(!flag(&rt, "Motor"));

        rt.set("Start", Value::Bool(true)).unwrap();
        rt.scan(NO_TIME).unwrap();
        assert!(flag(&rt, "Motor"));

        rt.set("Start", Value::Bool(false)).unwrap();
        rt.scan(NO_TIME).unwrap();
        assert!(!flag(&rt, "Motor"));
    }

    #[test]
    fn applies_initial_values_and_reset() {
        let mut rt = load("PROGRAM p VAR a : BOOL := TRUE; b : BOOL; END_VAR b := a; END_PROGRAM");
        assert!(flag(&rt, "a"));
        assert!(!flag(&rt, "b"));
        rt.scan(NO_TIME).unwrap();
        assert!(flag(&rt, "b"));
        rt.reset();
        assert!(!flag(&rt, "b"));
    }

    #[test]
    fn evaluates_boolean_expressions() {
        let mut rt = load(
            "PROGRAM p VAR a : BOOL := TRUE; b : BOOL := FALSE;
               r1 : BOOL; r2 : BOOL; r3 : BOOL; r4 : BOOL; r5 : BOOL; END_VAR
             r1 := a AND NOT b;
             r2 := a AND b OR NOT a;
             r3 := a = b;
             r4 := a <> b;
             r5 := NOT (a OR b);
             END_PROGRAM",
        );
        rt.scan(NO_TIME).unwrap();
        assert!(flag(&rt, "r1"));
        assert!(!flag(&rt, "r2"));
        assert!(!flag(&rt, "r3"));
        assert!(flag(&rt, "r4"));
        assert!(!flag(&rt, "r5"));
    }

    #[test]
    fn identifiers_are_case_insensitive() {
        let mut rt = load("PROGRAM p VAR Start : BOOL := TRUE; Motor : BOOL; END_VAR motor := START; END_PROGRAM");
        rt.scan(NO_TIME).unwrap();
        assert!(flag(&rt, "MOTOR"));
    }

    #[test]
    fn marks_unassigned_variables_as_inputs() {
        let vars = load(MOTOR).variables();
        assert_eq!(vars.len(), 2);
        assert!(vars[0].is_input, "Start is only read");
        assert!(!vars[1].is_input, "Motor is assigned");
    }

    #[test]
    fn elsif_runs_only_the_first_true_branch() {
        let src = |a: bool, b: bool| {
            format!(
                "PROGRAM p VAR a : BOOL := {a}; b : BOOL := {b}; r : INT; END_VAR
                 IF a THEN r := 1; ELSIF b THEN r := 2; ELSIF TRUE THEN r := 3; ELSE r := 4; END_IF;
                 END_PROGRAM"
            )
        };
        for (a, b, expected) in [(true, true, 1), (false, true, 2), (false, false, 3)] {
            let mut rt = load(&src(a, b));
            rt.scan(NO_TIME).unwrap();
            assert_eq!(get(&rt, "r"), Value::Int(expected), "a={a} b={b}");
        }
        let mut rt = load("PROGRAM p VAR r : INT; END_VAR IF FALSE THEN r := 1; ELSIF FALSE THEN r := 2; ELSE r := 4; END_IF; END_PROGRAM");
        rt.scan(NO_TIME).unwrap();
        assert_eq!(get(&rt, "r"), Value::Int(4));
    }

    // --- numeric types ---

    #[test]
    fn initializes_int_dint_real() {
        let rt = load(
            "PROGRAM p VAR i : INT := 10; n : INT := -5; d : DINT := 100000; r : REAL := 24.5;
             z : INT; zr : REAL; e : DINT := 2 * (3 + 4); END_VAR END_PROGRAM",
        );
        assert_eq!(get(&rt, "i"), Value::Int(10));
        assert_eq!(get(&rt, "n"), Value::Int(-5));
        assert_eq!(get(&rt, "d"), Value::DInt(100_000));
        assert_eq!(get(&rt, "r"), Value::Real(24.5));
        assert_eq!(get(&rt, "z"), Value::Int(0));
        assert_eq!(get(&rt, "zr"), Value::Real(0.0));
        assert_eq!(get(&rt, "e"), Value::DInt(14));
    }

    #[test]
    fn acceptance_examples() {
        assert_eq!(result_of("Result := Counter + Speed;"), Value::Int(30));
        assert_eq!(result_of("Result := Counter * Speed;"), Value::Int(200));
        assert_eq!(result_of("Result := Speed - Counter;"), Value::Int(10));
        assert_eq!(
            result_of("IF Counter < Speed THEN Result := 100; ELSE Result := 0; END_IF;"),
            Value::Int(100)
        );
        assert_eq!(
            result_of("IF Counter > Speed THEN Result := 100; ELSE Result := 0; END_IF;"),
            Value::Int(0)
        );
    }

    #[test]
    fn arithmetic_per_type() {
        assert_eq!(eval_as("INT", "7 + 3"), Ok(Value::Int(10)));
        assert_eq!(eval_as("INT", "7 - 10"), Ok(Value::Int(-3)));
        assert_eq!(eval_as("DINT", "70000 * 3"), Ok(Value::DInt(210_000)));
        // Integer division truncates toward zero.
        assert_eq!(eval_as("INT", "7 / 2"), Ok(Value::Int(3)));
        assert_eq!(eval_as("INT", "-7 / 2"), Ok(Value::Int(-3)));
        assert_eq!(eval_as("REAL", "7.0 / 2.0"), Ok(Value::Real(3.5)));
        assert_eq!(eval_as("REAL", "1.5 + 2"), Ok(Value::Real(3.5)));
        assert_eq!(eval_as("REAL", "-(2.5 * 2.0)"), Ok(Value::Real(-5.0)));
    }

    #[test]
    fn precedence_and_parentheses() {
        assert_eq!(eval_as("INT", "10 + 5 * 2"), Ok(Value::Int(20)));
        assert_eq!(eval_as("INT", "(10 + 5) * 2"), Ok(Value::Int(30)));
        assert_eq!(eval_as("INT", "20 - 5 - 3"), Ok(Value::Int(12)));
        assert_eq!(eval_as("INT", "100 / 10 / 2"), Ok(Value::Int(5)));
    }

    #[test]
    fn numeric_comparisons() {
        for (expr, expected) in [
            ("3 < 5", true), ("5 < 3", false), ("5 > 3", true), ("3 >= 3", true),
            ("2 <= 1", false), ("4 = 4", true), ("4 <> 4", false), ("2.5 > 2", true),
            ("1 + 1 = 2 AND 3 > 2", true),
        ] {
            assert_eq!(eval_as("BOOL", expr), Ok(Value::Bool(expected)), "{expr}");
        }
    }

    #[test]
    fn division_by_zero_is_a_positioned_error() {
        let err = eval_as("INT", "10 / 0").unwrap_err();
        // Points at the '/' operator in `x := 10 / 0;`.
        assert_eq!((err.line, err.column, err.message.as_str()), (1, 40, "Division by zero"));
        assert_eq!(eval_as("REAL", "1.0 / 0.0").unwrap_err().message, "Division by zero");
        // A variable divisor is only known at run time.
        let mut rt = load("PROGRAM p VAR a : INT := 5; b : INT; r : INT; END_VAR r := a / b; END_PROGRAM");
        assert_eq!(rt.scan(NO_TIME).unwrap_err().error.message, "Division by zero");
    }

    #[test]
    fn integer_overflow_is_an_error() {
        let mut rt = load("PROGRAM p VAR c : INT := 32767; END_VAR c := c + 1; END_PROGRAM");
        assert!(rt.scan(NO_TIME).unwrap_err().error.message.starts_with("INT overflow"));

        // Squaring 1e10 repeatedly passes f64's range on the 5th scan (1e320).
        let mut rt = load("PROGRAM p VAR r : REAL := 10000000000.0; END_VAR r := r * r; END_PROGRAM");
        let err = (0..5).find_map(|_| rt.scan(NO_TIME).err()).expect("REAL should overflow");
        assert!(err.error.message.starts_with("REAL overflow"));
    }

    #[test]
    fn invalid_type_combinations_do_not_start() {
        assert!(eval_as("INT", "TRUE + 10").unwrap_err().message.starts_with("Type error"));
        assert!(eval_as("INT", "10.5").unwrap_err().message.starts_with("Type mismatch"));
    }

    #[test]
    fn counter_increments_every_scan() {
        let mut rt = load("PROGRAM p VAR Counter : INT := 10; END_VAR Counter := Counter + 1; END_PROGRAM");
        for expected in [11, 12, 13] {
            rt.scan(NO_TIME).unwrap();
            assert_eq!(get(&rt, "Counter"), Value::Int(expected));
        }
        rt.reset();
        assert_eq!(get(&rt, "Counter"), Value::Int(10));
    }

    #[test]
    fn set_checks_name_and_type() {
        let mut rt = load("PROGRAM p VAR b : BOOL; i : INT; END_VAR END_PROGRAM");
        assert!(rt.set("Nope", Value::Bool(true)).is_err());
        let err = rt.set("i", Value::Bool(true)).unwrap_err();
        assert!(err.message.contains("'i' is INT"));
        assert!(rt.set("b", Value::Bool(true)).is_ok());
    }

    // --- TIME and function blocks ---

    #[test]
    fn time_arithmetic_and_comparison() {
        assert_eq!(eval_as("TIME", "T#1m30s + T#500ms - T#1s"), Ok(Value::Time(89_500)));
        assert_eq!(eval_as("BOOL", "T#2s > T#1500ms"), Ok(Value::Bool(true)));
        let rt = load("PROGRAM p VAR d : TIME := T#2s; z : TIME; END_VAR END_PROGRAM");
        assert_eq!(get(&rt, "d"), Value::Time(2_000));
        assert_eq!(get(&rt, "z"), Value::Time(0));
    }

    #[test]
    fn ton_in_a_program_uses_the_scan_clock() {
        let mut rt = load(
            "PROGRAM p VAR run : BOOL; t : TON; done : BOOL; left : TIME; END_VAR
             t(IN := run, PT := T#2s);
             done := t.Q;
             left := t.PT - t.ET;
             END_PROGRAM",
        );
        rt.set("run", Value::Bool(true)).unwrap();
        rt.scan(at(10_000)).unwrap(); // IN seen in the scan from 9.9 s to 10 s
        rt.scan(at(11_800)).unwrap();
        assert!(!flag(&rt, "done"));
        assert_eq!(get(&rt, "left"), Value::Time(100));
        rt.scan(at(11_900)).unwrap();
        assert!(flag(&rt, "done"));
        assert_eq!(rt.member("t", "ET"), Some(Value::Time(2_000)));

        rt.reset();
        assert_eq!(rt.member("t", "Q"), Some(Value::Bool(false)), "reset clears timer state");
    }

    #[test]
    fn function_blocks_are_monitored_separately() {
        let rt = load("PROGRAM p VAR x : BOOL; t : TON; c : CTU; END_VAR END_PROGRAM");
        let vars: Vec<_> = rt.variables().iter().map(|v| v.name.clone()).collect();
        assert_eq!(vars, vec!["x"]);
        let summary: Vec<_> = rt
            .function_blocks()
            .iter()
            .map(|b| {
                let members: Vec<_> =
                    b.members.iter().map(|m| format!("{}{}", m.name, if m.is_output { "*" } else { "" })).collect();
                format!("{} {} [{}]", b.name, b.block_type, members.join(" "))
            })
            .collect();
        assert_eq!(summary, vec!["t TON [IN PT Q* ET*]", "c CTU [CU R PV Q* CV*]"]);
    }

    /// The program from the user's report, which used ELSIF, TON, CTU and T#2S.
    const TANK: &str = "PROGRAM Main
VAR
  Start      : BOOL := FALSE;   (* DI0 - start pushbutton *)
  Stop       : BOOL := FALSE;   (* DI1 - stop pushbutton *)
  FaultReset : BOOL := FALSE;   (* DI2 - fault reset *)
  Motor      : BOOL := FALSE;   (* DO0 - motor contactor *)
  FillValve  : BOOL := FALSE;   (* DO1 - tank fill valve *)
  Fault      : BOOL := FALSE;
  Setpoint   : REAL := 80.0;    (* AO0 - tank level setpoint % *)
  TankLevel  : REAL := 20.0;    (* AI0 - tank level sensor % *)
  RunCycles  : INT := 0;
  RunTimer   : TON;
  Overfill   : CTU;
END_VAR

(* --- Motor start/stop seal-in logic --- *)
IF FaultReset THEN
  Fault := FALSE;
END_IF;

IF TankLevel >= 98.0 THEN
  Fault := TRUE;
END_IF;

IF Start AND NOT Stop AND NOT Fault THEN
  Motor := TRUE;
ELSIF Stop OR Fault THEN
  Motor := FALSE;
END_IF;

(* --- On-delay confirmation timer + run cycle counter --- *)
RunTimer(IN := Motor, PT := T#2S);
IF RunTimer.Q THEN
  RunCycles := RunCycles + 1;
END_IF;

(* --- Tank level process simulation --- *)
FillValve := Motor AND (TankLevel < Setpoint);
IF FillValve THEN
  TankLevel := TankLevel + 0.4;
ELSE
  TankLevel := TankLevel - 0.15;
END_IF;
IF TankLevel > 100.0 THEN TankLevel := 100.0; END_IF;
IF TankLevel < 0.0 THEN TankLevel := 0.0; END_IF;

Overfill(CU := TankLevel >= 95.0, R := FaultReset, PV := 3);

END_PROGRAM";

    fn real(rt: &Runtime, name: &str) -> f64 {
        match get(rt, name) {
            Value::Real(x) => x,
            other => panic!("{name} is {other:?}"),
        }
    }

    #[test]
    fn tank_program_from_the_report_runs() {
        let mut rt = load(TANK);
        let mut now = 0;
        let mut scan = |rt: &mut Runtime| {
            now += 100; // 100 ms scan
            rt.scan(at(now)).unwrap();
        };

        // Press and release Start: the seal-in keeps the motor running.
        rt.set("Start", Value::Bool(true)).unwrap();
        scan(&mut rt); // scan 1: RunTimer.IN rises, ET = 100 ms
        rt.set("Start", Value::Bool(false)).unwrap();
        for _ in 0..18 {
            scan(&mut rt); // scans 2..19
        }
        assert!(flag(&rt, "Motor"));
        assert_eq!(rt.member("RunTimer", "ET"), Some(Value::Time(1_900)));
        assert_eq!(get(&rt, "RunCycles"), Value::Int(0), "timer not done yet");

        scan(&mut rt); // scan 20: ET reaches PT = 2 s
        assert_eq!(rt.member("RunTimer", "Q"), Some(Value::Bool(true)));
        assert_eq!(get(&rt, "RunCycles"), Value::Int(1));
        scan(&mut rt);
        assert_eq!(get(&rt, "RunCycles"), Value::Int(2));

        // The fill valve holds the level near the 80 % setpoint.
        for _ in 0..400 {
            scan(&mut rt);
        }
        let level = real(&rt, "TankLevel");
        assert!((79.5..80.5).contains(&level), "level {level}");
        assert!(!flag(&rt, "Fault"));
        assert_eq!(rt.member("Overfill", "CV"), Some(Value::Int(0)));

        // A level spike trips the fault (ELSIF branch) and counts one overfill edge.
        rt.set("TankLevel", Value::Real(99.0)).unwrap();
        scan(&mut rt);
        assert!(flag(&rt, "Fault"));
        assert!(!flag(&rt, "Motor"));
        assert_eq!(rt.member("Overfill", "CV"), Some(Value::Int(1)));
        scan(&mut rt);
        assert_eq!(rt.member("Overfill", "CV"), Some(Value::Int(1)), "CU still TRUE: no new edge");
        assert_eq!(rt.member("RunTimer", "Q"), Some(Value::Bool(false)), "motor off resets the timer");

        // Stop and FaultReset.
        rt.set("Stop", Value::Bool(true)).unwrap();
        rt.set("FaultReset", Value::Bool(true)).unwrap();
        for _ in 0..10 {
            scan(&mut rt); // level drains below 98 %, so the reset sticks
        }
        assert!(!flag(&rt, "Fault"));
        assert!(!flag(&rt, "Motor"));
        assert_eq!(rt.member("Overfill", "CV"), Some(Value::Int(0)), "R resets the counter");
    }

    // --- MOD, XOR, widening ---

    #[test]
    fn mod_xor_and_widening() {
        assert_eq!(eval_as("INT", "17 MOD 5"), Ok(Value::Int(2)));
        assert_eq!(eval_as("INT", "-7 MOD 3"), Ok(Value::Int(-1)), "sign follows the dividend");
        assert_eq!(eval_as("INT", "2 + 7 MOD 4 * 2"), Ok(Value::Int(8)), "MOD binds like * and /");
        assert_eq!(eval_as("INT", "5 MOD 0").unwrap_err().message, "Division by zero");
        assert_eq!(eval_as("BOOL", "TRUE XOR FALSE"), Ok(Value::Bool(true)));
        assert_eq!(eval_as("BOOL", "TRUE XOR TRUE"), Ok(Value::Bool(false)));

        // The operation's type comes from its operands; only the result is widened.
        let mut rt = load(
            "PROGRAM p VAR i : INT := 30000; d : DINT := 5; r : REAL; big : DINT; END_VAR
             r := i / 7;       (* INT division = 4285, then widened to REAL *)
             big := d + i;     (* DINT addition: i widens to DINT first *)
             END_PROGRAM",
        );
        rt.scan(NO_TIME).unwrap();
        assert_eq!(get(&rt, "r"), Value::Real(4_285.0));
        assert_eq!(get(&rt, "big"), Value::DInt(30_005));

        let mut rt = load("PROGRAM p VAR i : INT := 30000; d : DINT; END_VAR d := i + i; END_PROGRAM");
        assert!(
            rt.scan(NO_TIME).unwrap_err().error.message.starts_with("INT overflow"),
            "INT + INT overflows as INT even when stored in a DINT"
        );
    }

    // --- control flow ---

    #[test]
    fn for_loops() {
        let run = |body: &str| {
            let mut rt = load(&format!("PROGRAM p VAR i : INT; s : INT; log : DINT; END_VAR {body} END_PROGRAM"));
            rt.scan(NO_TIME).unwrap();
            (get(&rt, "i"), get(&rt, "s"), get(&rt, "log"))
        };
        assert_eq!(run("FOR i := 1 TO 5 DO s := s + i; END_FOR;"), (Value::Int(6), Value::Int(15), Value::DInt(0)));
        assert_eq!(run("FOR i := 0 TO 10 BY 3 DO s := s + 1; END_FOR;"), (Value::Int(12), Value::Int(4), Value::DInt(0)));
        // Reverse loop, recording the visiting order as digits: 5, 4, 3, 2, 1.
        assert_eq!(
            run("FOR i := 5 TO 1 BY -1 DO log := log * 10 + i; END_FOR;"),
            (Value::Int(0), Value::Int(0), Value::DInt(54_321))
        );
        // An empty range runs zero times but still assigns the start value.
        assert_eq!(run("FOR i := 5 TO 1 DO s := 99; END_FOR;"), (Value::Int(5), Value::Int(0), Value::DInt(0)));
    }

    #[test]
    fn for_loop_runtime_errors() {
        let mut rt = load("PROGRAM p VAR i : INT; z : INT; END_VAR FOR i := 1 TO 5 BY z DO END_FOR; END_PROGRAM");
        assert!(rt.scan(NO_TIME).unwrap_err().error.message.contains("FOR step is 0"));
        // Reaching past INT's range when stepping beyond the end is an overflow, not a wrap.
        let mut rt = load("PROGRAM p VAR i : INT; END_VAR FOR i := 32766 TO 32767 DO END_FOR; END_PROGRAM");
        assert!(rt.scan(NO_TIME).unwrap_err().error.message.contains("INT overflow"));
    }

    #[test]
    fn while_repeat_and_case() {
        let mut rt = load("PROGRAM p VAR c : INT; END_VAR WHILE c < 5 DO c := c + 1; END_WHILE; END_PROGRAM");
        rt.scan(NO_TIME).unwrap();
        assert_eq!(get(&rt, "c"), Value::Int(5));
        rt.scan(NO_TIME).unwrap();
        assert_eq!(get(&rt, "c"), Value::Int(5), "condition already FALSE: body doesn't run");

        let mut rt = load("PROGRAM p VAR c : INT := 10; END_VAR REPEAT c := c + 1; UNTIL c >= 5 END_REPEAT; END_PROGRAM");
        rt.scan(NO_TIME).unwrap();
        assert_eq!(get(&rt, "c"), Value::Int(11), "REPEAT runs its body at least once");

        let case = |mode: i16| {
            let mut rt = load(&format!(
                "PROGRAM p VAR mode : INT := {mode}; r : INT; END_VAR
                 CASE mode OF 0: r := 10; 1, 2: r := 20; -1: r := 30; ELSE r := 99; END_CASE;
                 END_PROGRAM"
            ));
            rt.scan(NO_TIME).unwrap();
            get(&rt, "r")
        };
        assert_eq!(
            [0, 1, 2, -1, 7].map(case),
            [Value::Int(10), Value::Int(20), Value::Int(20), Value::Int(30), Value::Int(99)]
        );
        let mut rt = load("PROGRAM p VAR m : INT := 5; r : INT := 1; END_VAR CASE m OF 0: r := 2; END_CASE; END_PROGRAM");
        rt.scan(NO_TIME).unwrap();
        assert_eq!(get(&rt, "r"), Value::Int(1), "no match and no ELSE: nothing runs");
    }

    #[test]
    fn infinite_loops_are_stopped() {
        for (body, line) in [
            ("\nWHILE TRUE DO x := x; END_WHILE;", 2),
            ("\n\nREPEAT x := x; UNTIL FALSE END_REPEAT;", 3),
            // Nested loops share one budget: 1000 × 1000 exceeds it.
            ("FOR i := 1 TO 1000 DO\nFOR j := 1 TO 1000 DO END_FOR; END_FOR;", 2),
        ] {
            let mut rt = load(&format!("PROGRAM p VAR x : BOOL; i : INT; j : INT; END_VAR {body} END_PROGRAM"));
            let err = rt.scan(NO_TIME).unwrap_err();
            assert!(err.error.message.starts_with("Loop limit exceeded"), "{body}: {}", err.error.message);
            assert_eq!(err.error.line, line, "{body}");
        }
        // The budget is per scan, not per program run.
        let mut rt = load("PROGRAM p VAR i : INT; END_VAR FOR i := 1 TO 30000 DO END_FOR; END_PROGRAM");
        for _ in 0..5 {
            rt.scan(NO_TIME).unwrap();
        }
    }

    // --- the spec's acceptance programs ---

    fn run_once(src: &str) -> Runtime {
        let mut rt = load(src);
        rt.scan(at(100)).unwrap();
        rt
    }

    #[test]
    fn test_program_1_numeric() {
        let rt = run_once(
            "PROGRAM NumericTest
             VAR A : INT := 10; B : INT := 20; Result : INT := 0; END_VAR
             Result := (A + B) * 2;
             END_PROGRAM",
        );
        assert_eq!((get(&rt, "A"), get(&rt, "B"), get(&rt, "Result")), (Value::Int(10), Value::Int(20), Value::Int(60)));

        let rt = run_once(
            "PROGRAM R VAR A : REAL := 10.5; B : REAL := 2.5; Result : REAL := 0.0; END_VAR
             Result := A + B;
             END_PROGRAM",
        );
        assert_eq!(get(&rt, "Result"), Value::Real(13.0));
    }

    #[test]
    fn test_programs_2_to_5_control_flow() {
        let rt = run_once(
            "PROGRAM ControlTest
             VAR Counter : INT := 0; Sum : INT := 0; END_VAR
             FOR Counter := 1 TO 5 DO
                 Sum := Sum + Counter;
             END_FOR;
             END_PROGRAM",
        );
        assert_eq!((get(&rt, "Counter"), get(&rt, "Sum")), (Value::Int(6), Value::Int(15)));

        let rt = run_once(
            "PROGRAM WhileTest
             VAR Counter : INT := 0; END_VAR
             WHILE Counter < 5 DO
                 Counter := Counter + 1;
             END_WHILE;
             END_PROGRAM",
        );
        assert_eq!(get(&rt, "Counter"), Value::Int(5));

        let rt = run_once(
            "PROGRAM RepeatTest
             VAR Counter : INT := 0; END_VAR
             REPEAT
                 Counter := Counter + 1;
             UNTIL Counter >= 5
             END_REPEAT;
             END_PROGRAM",
        );
        assert_eq!(get(&rt, "Counter"), Value::Int(5));

        let rt = run_once(
            "PROGRAM CaseTest
             VAR Mode : INT := 1; Motor : BOOL := FALSE; END_VAR
             CASE Mode OF

                 0:
                     Motor := FALSE;

                 1:
                     Motor := TRUE;

             ELSE
                 Motor := FALSE;

             END_CASE;
             END_PROGRAM",
        );
        assert!(flag(&rt, "Motor"));
    }

    /// Run `scans` 100 ms scans starting after `*now`, returning `probe` after each.
    fn scans<T>(rt: &mut Runtime, now: &mut i64, count: usize, probe: impl Fn(&Runtime) -> T) -> Vec<T> {
        (0..count)
            .map(|_| {
                *now += 100;
                rt.scan(at(*now)).unwrap();
                probe(rt)
            })
            .collect()
    }

    #[test]
    fn test_program_6_ton() {
        let mut rt = load(
            "PROGRAM TimerTest
             VAR Start : BOOL := TRUE; Motor : BOOL := FALSE; RunTimer : TON; END_VAR
             RunTimer(
                 IN := Start,
                 PT := T#2S
             );
             IF RunTimer.Q THEN
                 Motor := TRUE;
             ELSE
                 Motor := FALSE;
             END_IF;
             END_PROGRAM",
        );
        let mut now = 0;
        let observed = scans(&mut rt, &mut now, 21, |rt| (rt.member("RunTimer", "ET").unwrap(), flag(rt, "Motor")));
        for (scan, (et, motor)) in observed.iter().enumerate().map(|(i, o)| (i + 1, o)) {
            let expected_et = (scan as i64 * 100).min(2_000);
            assert_eq!(*et, Value::Time(expected_et), "scan {scan}");
            assert_eq!(*motor, scan >= 20, "scan {scan}: Motor follows RunTimer.Q from scan 20");
        }
    }

    #[test]
    fn test_program_7_tof() {
        let mut rt = load(
            "PROGRAM TOFTest
             VAR Input : BOOL := TRUE; Output : BOOL := FALSE; Timer : TOF; END_VAR
             Timer(
                 IN := Input,
                 PT := T#1S
             );
             Output := Timer.Q;
             END_PROGRAM",
        );
        let mut now = 0;
        assert_eq!(scans(&mut rt, &mut now, 3, |rt| flag(rt, "Output")), vec![true; 3], "Input TRUE → Q TRUE");
        rt.set("Input", Value::Bool(false)).unwrap();
        let after = scans(&mut rt, &mut now, 12, |rt| flag(rt, "Output"));
        assert_eq!(after[..9], [true; 9], "Q holds for 1 s after Input falls");
        assert_eq!(after[9..], [false; 3], "then drops");
    }

    #[test]
    fn test_program_8_tp() {
        let mut rt = load(
            "PROGRAM TPTest
             VAR Trigger : BOOL := FALSE; PulseOutput : BOOL := FALSE; Pulse : TP; END_VAR
             Pulse(
                 IN := Trigger,
                 PT := T#1S
             );
             PulseOutput := Pulse.Q;
             END_PROGRAM",
        );
        let mut now = 0;
        assert_eq!(scans(&mut rt, &mut now, 2, |rt| flag(rt, "PulseOutput")), vec![false; 2]);
        rt.set("Trigger", Value::Bool(true)).unwrap();
        let pulse = scans(&mut rt, &mut now, 12, |rt| flag(rt, "PulseOutput"));
        assert_eq!(pulse[..9], [true; 9], "rising edge → Q TRUE for 1 s");
        assert_eq!(pulse[9..], [false; 3], "then FALSE while Trigger stays TRUE");
        rt.set("Trigger", Value::Bool(false)).unwrap();
        scans(&mut rt, &mut now, 1, |_| ());
        rt.set("Trigger", Value::Bool(true)).unwrap();
        assert_eq!(scans(&mut rt, &mut now, 1, |rt| flag(rt, "PulseOutput")), vec![true], "a new edge, a new pulse");
    }

    #[test]
    fn timer_state_persists_and_reset_clears_it() {
        let mut rt = load("PROGRAM p VAR t : TON; END_VAR t(IN := TRUE, PT := T#1S); END_PROGRAM");
        let mut now = 0;
        scans(&mut rt, &mut now, 4, |_| ());
        assert_eq!(rt.member("t", "ET"), Some(Value::Time(400)));
        rt.reset();
        assert_eq!(rt.member("t", "ET"), Some(Value::Time(0)));
        assert_eq!(rt.member("t", "PT"), Some(Value::Time(0)), "inputs reset too");
    }

    // --- counters, edge triggers and standard functions (spec test programs) ---

    /// Set inputs, then run one scan.
    fn step(rt: &mut Runtime, inputs: &[(&str, bool)]) {
        for (name, value) in inputs {
            rt.set(name, Value::Bool(*value)).unwrap();
        }
        rt.scan(NO_TIME).unwrap();
    }

    fn cv_q(rt: &Runtime, fb: &str) -> (Value, Value) {
        (rt.member(fb, "CV").unwrap(), rt.member(fb, "Q").unwrap())
    }

    #[test]
    fn test_program_1_ctu() {
        let mut rt = load(
            "PROGRAM CounterUpTest
             VAR Pulse : BOOL := FALSE; Reset : BOOL := FALSE; Counter : CTU; END_VAR
             Counter(
                 CU := Pulse,
                 R := Reset,
                 PV := 3
             );
             END_PROGRAM",
        );
        step(&mut rt, &[]); // RUN
        assert_eq!(cv_q(&rt, "Counter"), (Value::Int(0), Value::Bool(false)));
        // FALSE→TRUE, TRUE→FALSE, FALSE→TRUE, TRUE→FALSE, FALSE→TRUE, with extra scans in between.
        for pulse in [true, true, false, true, false, false, true, true] {
            step(&mut rt, &[("Pulse", pulse)]);
        }
        assert_eq!(cv_q(&rt, "Counter"), (Value::Int(3), Value::Bool(true)));
        step(&mut rt, &[("Reset", true)]);
        assert_eq!(cv_q(&rt, "Counter"), (Value::Int(0), Value::Bool(false)));
    }

    #[test]
    fn test_program_2_ctd() {
        let mut rt = load(
            "PROGRAM CounterDownTest
             VAR Pulse : BOOL := FALSE; Load : BOOL := TRUE; Counter : CTD; END_VAR
             Counter(
                 CD := Pulse,
                 LD := Load,
                 PV := 3
             );
             END_PROGRAM",
        );
        step(&mut rt, &[]);
        assert_eq!(cv_q(&rt, "Counter"), (Value::Int(3), Value::Bool(false)), "loaded");
        step(&mut rt, &[("Load", false)]);
        for expected in [2, 1, 0] {
            step(&mut rt, &[("Pulse", true)]);
            step(&mut rt, &[("Pulse", false)]);
            assert_eq!(rt.member("Counter", "CV"), Some(Value::Int(expected)));
        }
        assert_eq!(rt.member("Counter", "Q"), Some(Value::Bool(true)), "Q at CV = 0");
    }

    #[test]
    fn test_program_3_r_trig() {
        let mut rt = load(
            "PROGRAM RisingEdgeTest
             VAR Start : BOOL := FALSE; Rising : R_TRIG; Count : INT := 0; END_VAR
             Rising(
                 CLK := Start
             );
             IF Rising.Q THEN
                 Count := Count + 1;
             END_IF;
             END_PROGRAM",
        );
        step(&mut rt, &[]);
        step(&mut rt, &[("Start", true)]);
        assert_eq!(get(&rt, "Count"), Value::Int(1));
        for _ in 0..5 {
            step(&mut rt, &[]); // Start held TRUE
        }
        assert_eq!(get(&rt, "Count"), Value::Int(1), "no repeat while held");
        step(&mut rt, &[("Start", false)]);
        step(&mut rt, &[("Start", true)]);
        assert_eq!(get(&rt, "Count"), Value::Int(2));
    }

    #[test]
    fn test_program_4_f_trig() {
        let mut rt = load(
            "PROGRAM FallingEdgeTest
             VAR Start : BOOL := TRUE; Falling : F_TRIG; Count : INT := 0; END_VAR
             Falling(
                 CLK := Start
             );
             IF Falling.Q THEN
                 Count := Count + 1;
             END_IF;
             END_PROGRAM",
        );
        step(&mut rt, &[]);
        assert_eq!(get(&rt, "Count"), Value::Int(0));
        step(&mut rt, &[("Start", false)]);
        assert_eq!(get(&rt, "Count"), Value::Int(1));
        for _ in 0..5 {
            step(&mut rt, &[]); // Start held FALSE
        }
        assert_eq!(get(&rt, "Count"), Value::Int(1));
        step(&mut rt, &[("Start", true)]);
        step(&mut rt, &[("Start", false)]);
        assert_eq!(get(&rt, "Count"), Value::Int(2));
    }

    #[test]
    fn test_program_5_functions() {
        let rt = run_once(
            "PROGRAM FunctionTest
             VAR
                 A : REAL := -25.5; B : REAL := 10.0;
                 Result1 : REAL := 0.0; Result2 : REAL := 0.0; Result3 : REAL := 0.0;
                 T : DINT; R : DINT; S : REAL; C : REAL;
             END_VAR
             Result1 := ABS(A);
             Result2 := MAX(A, B);
             Result3 := SQRT(25.0);
             T := TRUNC(10.75);
             R := ROUND(10.75);
             S := SIN(0.0);
             C := COS(0.0);
             END_PROGRAM",
        );
        assert_eq!(get(&rt, "Result1"), Value::Real(25.5));
        assert_eq!(get(&rt, "Result2"), Value::Real(10.0));
        assert_eq!(get(&rt, "Result3"), Value::Real(5.0));
        assert_eq!(get(&rt, "T"), Value::DInt(10));
        assert_eq!(get(&rt, "R"), Value::DInt(11));
        assert_eq!(get(&rt, "S"), Value::Real(0.0));
        assert_eq!(get(&rt, "C"), Value::Real(1.0));
    }

    #[test]
    fn functions_inside_expressions_and_conversions() {
        assert_eq!(eval_as("INT", "ABS(-10)"), Ok(Value::Int(10)));
        assert_eq!(eval_as("INT", "MIN(4, 9) + MAX(2, -7) * 3"), Ok(Value::Int(10)));
        assert_eq!(eval_as("REAL", "SQRT(16)"), Ok(Value::Real(4.0)), "integer literal becomes REAL");
        assert_eq!(eval_as("DINT", "ROUND(SQRT(2.0) * 10.0)"), Ok(Value::DInt(14)));
        let rt = run_once("PROGRAM p VAR i : INT := -3; r : REAL; END_VAR r := SQRT(ABS(i) * 3); END_PROGRAM");
        assert_eq!(get(&rt, "r"), Value::Real(3.0), "INT result widened to REAL for SQRT");
    }

    #[test]
    fn function_runtime_errors_are_positioned() {
        let mut rt = load("PROGRAM p VAR x : REAL := -4.0; r : REAL; END_VAR\nr := SQRT(x);\nEND_PROGRAM");
        let err = rt.scan(NO_TIME).unwrap_err();
        assert_eq!((err.error.line, err.error.column, err.error.message.as_str()), (2, 6, "SQRT of a negative number (-4)"));
        let mut rt = load("PROGRAM p VAR i : INT := -32768; END_VAR i := ABS(i); END_PROGRAM");
        assert!(rt.scan(NO_TIME).unwrap_err().error.message.starts_with("ABS overflow"));
    }

    #[test]
    fn instances_keep_independent_state() {
        let mut rt = load(
            "PROGRAM p
             VAR a : BOOL; b : BOOL; Counter1 : CTU; Counter2 : CTU; Rising1 : R_TRIG; Rising2 : R_TRIG; END_VAR
             Counter1(CU := a, PV := 10); Counter2(CU := b, PV := 10);
             Rising1(CLK := a); Rising2(CLK := b);
             END_PROGRAM",
        );
        step(&mut rt, &[("a", true)]);
        assert_eq!(rt.member("Rising1", "Q"), Some(Value::Bool(true)));
        assert_eq!(rt.member("Rising2", "Q"), Some(Value::Bool(false)));
        step(&mut rt, &[("a", false), ("b", true)]);
        step(&mut rt, &[("a", true)]);
        assert_eq!(rt.member("Counter1", "CV"), Some(Value::Int(2)));
        assert_eq!(rt.member("Counter2", "CV"), Some(Value::Int(1)));
        assert_eq!(rt.member("Rising2", "Q"), Some(Value::Bool(false)), "b held TRUE: no new edge");
        rt.reset();
        assert_eq!(rt.member("Counter1", "CV"), Some(Value::Int(0)), "STOP resets every instance");
    }

    // --- multi-program projects ---

    fn project(programs: &[(&str, &str)]) -> Result<Runtime, Vec<StErrors>> {
        Runtime::project(programs.iter().map(|(name, src)| (name.to_string(), parse(src).unwrap())).collect(), &[])
            .map_err(|e| e.programs)
    }

    /// `name` as program `program` sees it.
    fn value_in(rt: &Runtime, program: &str, name: &str) -> Value {
        rt.variables().into_iter().find(|v| v.program == program && v.name == name).unwrap().value
    }

    const SPEC_PROJECT: [(&str, &str); 4] = [
        ("Main", "PROGRAM Main\nVAR\n    Start : BOOL := FALSE;\nEND_VAR\nEND_PROGRAM"),
        (
            "MotorControl",
            "PROGRAM MotorControl\nVAR\n    Motor : BOOL := FALSE;\nEND_VAR\nIF Start THEN\n    Motor := TRUE;\nELSE\n    Motor := FALSE;\nEND_IF;\nEND_PROGRAM",
        ),
        ("AlarmLogic", "PROGRAM AlarmLogic\nVAR\n    Alarm : BOOL := FALSE;\nEND_VAR\nAlarm := FALSE;\nEND_PROGRAM"),
        ("TankControl", "PROGRAM TankControl\nVAR\n    TankLevel : REAL := 50.0;\nEND_VAR\nTankLevel := TankLevel + 0.1;\nEND_PROGRAM"),
    ];

    #[test]
    fn spec_project_runs_every_program_each_scan_with_shared_state() {
        let mut rt = project(&SPEC_PROJECT).unwrap();
        assert_eq!(rt.program_names(), ["Main", "MotorControl", "AlarmLogic", "TankControl"]);
        let programs: Vec<_> = rt.variables().into_iter().map(|v| (v.program, v.name, v.is_input)).collect();
        assert_eq!(
            programs,
            [
                ("Main".into(), "Start".into(), true), // read by MotorControl, written by nobody
                ("MotorControl".into(), "Motor".into(), false),
                ("AlarmLogic".into(), "Alarm".into(), false),
                ("TankControl".into(), "TankLevel".into(), false),
            ]
        );

        rt.scan(NO_TIME).unwrap();
        assert_eq!(value_in(&rt, "MotorControl", "Motor"), Value::Bool(false));
        // Main's Start drives MotorControl's Motor in the very next scan.
        rt.set_in("Main", "Start", Value::Bool(true)).unwrap();
        rt.scan(NO_TIME).unwrap();
        assert_eq!(value_in(&rt, "MotorControl", "Motor"), Value::Bool(true));
        let Value::Real(level) = value_in(&rt, "TankControl", "TankLevel") else { panic!() };
        assert!((level - 50.2).abs() < 1e-4, "TankControl ran in both scans: {level}");

        rt.reset();
        assert_eq!(value_in(&rt, "Main", "Start"), Value::Bool(false));
        assert_eq!(value_in(&rt, "TankControl", "TankLevel"), Value::Real(50.0));
        assert!(rt.set_in("MotorControl", "Start", Value::Bool(true)).is_err(), "inputs are set where declared");
    }

    #[test]
    fn programs_run_in_project_order_every_scan() {
        let mut rt = project(&[
            ("ProgramA", "PROGRAM ProgramA VAR Seq : DINT; Scans : DINT; END_VAR Seq := 1; Scans := Scans + 1; END_PROGRAM"),
            ("ProgramB", "PROGRAM ProgramB VAR SawA : BOOL; END_VAR SawA := Seq = 1; Seq := Seq * 10 + 2; END_PROGRAM"),
            ("ProgramC", "PROGRAM ProgramC VAR SawB : BOOL; END_VAR SawB := Seq = 12; Seq := Seq * 10 + 3; END_PROGRAM"),
        ])
        .unwrap();
        for scan in 1..=5 {
            rt.scan(NO_TIME).unwrap();
            assert_eq!(value_in(&rt, "ProgramA", "Seq"), Value::DInt(123), "A → B → C in scan {scan}");
            assert_eq!(value_in(&rt, "ProgramA", "Scans"), Value::DInt(scan), "each program once per scan");
            assert_eq!(value_in(&rt, "ProgramB", "SawA"), Value::Bool(true));
            assert_eq!(value_in(&rt, "ProgramC", "SawB"), Value::Bool(true));
        }
    }

    #[test]
    fn function_blocks_persist_per_program_and_stay_independent() {
        // Same instance name in two programs: two independent timers and counters.
        let fbs = "VAR T : TON; C : CTU; Pulse : BOOL; END_VAR T(IN := TRUE, PT := T#300ms); Pulse := NOT Pulse; C(CU := Pulse);";
        let mut rt = project(&[
            ("Fast", &format!("PROGRAM Fast {fbs} END_PROGRAM")),
            ("Slow", "PROGRAM Slow VAR T : TON; END_VAR T(IN := FALSE, PT := T#300ms); END_PROGRAM"),
        ])
        .unwrap();
        let member = |rt: &Runtime, program: &str, fb: &str, m: &str| {
            let block = rt.function_blocks().into_iter().find(|b| b.program == program && b.name == fb).unwrap();
            block.members.into_iter().find(|x| x.name == m).unwrap().value
        };
        let mut now = 0;
        for _ in 0..4 {
            now += 100;
            rt.scan(at(now)).unwrap();
        }
        assert_eq!(member(&rt, "Fast", "T", "ET"), Value::Time(300), "the timer kept its state across scans");
        assert_eq!(member(&rt, "Fast", "T", "Q"), Value::Bool(true));
        assert_eq!(member(&rt, "Fast", "C", "CV"), Value::Int(2), "two rising edges in four scans");
        assert_eq!(member(&rt, "Slow", "T", "ET"), Value::Time(0), "Slow's T is a different instance");
        rt.reset();
        assert_eq!(member(&rt, "Fast", "C", "CV"), Value::Int(0));
    }

    #[test]
    fn shared_names_resolve_locally_first_then_to_the_one_declaring_program() {
        let mut rt = project(&[
            ("A", "PROGRAM A VAR i : INT; Total : INT; END_VAR FOR i := 1 TO 3 DO Total := Total + i; END_FOR; END_PROGRAM"),
            // B has its own i; Total is A's.
            ("B", "PROGRAM B VAR i : INT := 100; Copy : INT; END_VAR Copy := Total + i; END_PROGRAM"),
        ])
        .unwrap();
        rt.scan(NO_TIME).unwrap();
        assert_eq!(value_in(&rt, "A", "i"), Value::Int(4));
        assert_eq!(value_in(&rt, "B", "i"), Value::Int(100), "B's i is untouched by A's loop");
        assert_eq!(value_in(&rt, "B", "Copy"), Value::Int(106));
    }

    #[test]
    fn every_programs_errors_are_reported_and_nothing_is_created() {
        let mut programs = SPEC_PROJECT;
        programs[2].1 = "PROGRAM AlarmLogic\nVAR\n    Alarm : BOOL := FALSE;\nEND_VAR\nUnknownVariable := TRUE;\nEND_PROGRAM";
        programs[1].1 = "PROGRAM MotorControl\nVAR\n    Speed : INT;\nEND_VAR\nSpeed := Start;\nEND_PROGRAM";
        let errors = project(&programs).err().unwrap();
        let summary: Vec<Vec<(ErrorKind, usize, String)>> =
            errors.iter().map(|e| e.iter().map(|e| (e.kind, e.line, e.message.clone())).collect()).collect();
        assert_eq!(
            summary,
            [
                vec![],
                vec![(ErrorKind::TypeMismatch, 5, "Type mismatch: cannot assign BOOL to 'Speed' (INT)".into())],
                vec![(ErrorKind::UndeclaredVariable, 5, "Undeclared variable 'UnknownVariable'".into())],
                vec![],
            ]
        );
        assert!(project(&SPEC_PROJECT).is_ok(), "fixed: the project compiles");
    }

    #[test]
    fn ambiguous_shared_names_are_errors() {
        let errors = project(&[
            ("A", "PROGRAM A VAR Level : INT; END_VAR END_PROGRAM"),
            ("B", "PROGRAM B VAR Level : INT; END_VAR END_PROGRAM"),
            ("C", "PROGRAM C VAR x : INT; END_VAR x := Level; END_PROGRAM"),
        ])
        .err()
        .unwrap();
        assert!(errors[0].is_empty() && errors[1].is_empty(), "the same local name in two programs is fine");
        assert_eq!(
            errors[2][0].message,
            "Ambiguous variable 'Level': declared in programs A, B; declare it in this program or give it a unique name"
        );
    }

    #[test]
    fn runtime_errors_name_their_program() {
        let mut rt = project(&[
            ("Ok", "PROGRAM Ok VAR d : INT; END_VAR END_PROGRAM"),
            ("Bad", "PROGRAM Bad VAR r : INT; END_VAR\nr := 1 / d;\nEND_PROGRAM"),
        ])
        .unwrap();
        let failure = rt.scan(NO_TIME).unwrap_err();
        assert_eq!(failure.program, "Bad");
        assert_eq!((failure.error.line, failure.error.message.as_str()), (2, "Division by zero"));
    }

    // --- debugger ---

    /// Spec example: lines 10-12 are three statements; line 8 is an IF.
    const DEBUG_MAIN: &str = "PROGRAM Main
VAR
    Start : BOOL := TRUE;
    Motor : BOOL;
    Counter : INT;
    Alarm : BOOL := TRUE;
END_VAR
IF Start THEN
    Motor := TRUE;
Motor := TRUE;
Counter := Counter + 1;
Alarm := FALSE;
END_IF;
END_PROGRAM";

    fn at_line(location: Option<Location>) -> Option<(String, usize)> {
        location.map(|l| (l.program, l.line))
    }

    fn debug_project(programs: &[(&str, &str)], breakpoints: &[(&str, usize)]) -> Runtime {
        let mut rt = project(programs).unwrap();
        rt.set_breakpoints(&breakpoints.iter().map(|(p, l)| (p.to_string(), *l)).collect::<Vec<_>>());
        rt
    }

    #[test]
    fn statement_lines_are_the_breakable_lines() {
        let program = parse(DEBUG_MAIN).unwrap();
        assert_eq!(crate::st::ast::statement_lines(&program.body), [8, 9, 10, 11, 12]);
    }

    #[test]
    fn breakpoint_pauses_before_its_statement_and_resume_finishes_the_scan() {
        let mut rt = debug_project(&[("Main", DEBUG_MAIN)], &[("Main", 11)]);
        let paused = rt.run(NO_TIME, Stop::AtBreakpoint).unwrap();
        assert_eq!(at_line(paused), Some(("Main".into(), 11)));
        assert_eq!(rt.location().map(|l| l.column), Some(1));
        // State at the pause point: lines 9-10 ran, 11 (the breakpoint) didn't.
        assert_eq!((get(&rt, "Motor"), get(&rt, "Counter"), get(&rt, "Alarm")), (Value::Bool(true), Value::Int(0), Value::Bool(true)));

        assert_eq!(rt.proceed(Stop::AtBreakpoint).unwrap(), None, "the rest of the scan runs; the same stop doesn't repeat");
        assert_eq!((get(&rt, "Counter"), get(&rt, "Alarm")), (Value::Int(1), Value::Bool(false)));
        assert!(rt.location().is_none());

        // The next scan stops there again, with nothing reset in between.
        assert!(rt.run(NO_TIME, Stop::AtBreakpoint).unwrap().is_some());
        assert_eq!(get(&rt, "Counter"), Value::Int(1));
        rt.proceed(Stop::AtBreakpoint).unwrap();
        assert_eq!(get(&rt, "Counter"), Value::Int(2));
    }

    #[test]
    fn step_statement_runs_exactly_one_statement() {
        let mut rt = debug_project(&[("Main", DEBUG_MAIN)], &[("Main", 10)]);
        assert_eq!(at_line(rt.run(NO_TIME, Stop::AtBreakpoint).unwrap()), Some(("Main".into(), 10)));
        assert_eq!(at_line(rt.proceed(Stop::NextStatement).unwrap()), Some(("Main".into(), 11)), "line 10 ran");
        assert_eq!(get(&rt, "Counter"), Value::Int(0));
        assert_eq!(at_line(rt.proceed(Stop::NextStatement).unwrap()), Some(("Main".into(), 12)));
        assert_eq!((get(&rt, "Counter"), get(&rt, "Alarm")), (Value::Int(1), Value::Bool(true)), "line 11 ran, 12 not yet");
        assert_eq!(rt.proceed(Stop::NextStatement).unwrap(), None, "line 12 was the last statement: the scan is complete");
        assert_eq!(get(&rt, "Alarm"), Value::Bool(false));

        // From the first statement of a scan, stepping goes into the IF body.
        assert_eq!(at_line(rt.run(NO_TIME, Stop::NextStatement).unwrap()), Some(("Main".into(), 8)));
        assert_eq!(at_line(rt.proceed(Stop::NextStatement).unwrap()), Some(("Main".into(), 9)));
    }

    #[test]
    fn debugging_spans_programs_in_order_and_breakpoints_belong_to_their_program() {
        let a = "PROGRAM A VAR Seq : DINT; END_VAR\nSeq := 1;\nSeq := Seq * 10 + 2;\nEND_PROGRAM";
        let b = "PROGRAM B VAR END_VAR\nSeq := Seq * 10 + 3;\nSeq := Seq * 10 + 4;\nEND_PROGRAM";
        let mut rt = debug_project(&[("A", a), ("B", b)], &[("B", 3)]);
        // Line 3 of A has no breakpoint; line 3 of B does.
        assert_eq!(at_line(rt.run(NO_TIME, Stop::AtBreakpoint).unwrap()), Some(("B".into(), 3)));
        assert_eq!(get(&rt, "Seq"), Value::DInt(123), "A ran completely, then B up to its line 3");
        // Stepping past A's last statement lands in B.
        let mut rt = debug_project(&[("A", a), ("B", b)], &[("A", 3)]);
        rt.run(NO_TIME, Stop::AtBreakpoint).unwrap();
        assert_eq!(at_line(rt.proceed(Stop::NextStatement).unwrap()), Some(("B".into(), 2)));
        assert_eq!(rt.proceed(Stop::Never).unwrap(), None);
        assert_eq!(get(&rt, "Seq"), Value::DInt(1234));
    }

    #[test]
    fn timers_counters_and_edits_survive_a_pause() {
        let src = "PROGRAM Main VAR T : TON; C : CTU; Pulse : BOOL; Mark : BOOL; END_VAR
T(IN := TRUE, PT := T#2S);
Pulse := NOT Pulse;
C(CU := Pulse, PV := 5);
Mark := TRUE;
END_PROGRAM";
        let mut rt = debug_project(&[("Main", src)], &[("Main", 5)]);
        let mut now = 0;
        for _ in 0..5 {
            now += 200;
            assert!(rt.run(at(now), Stop::AtBreakpoint).unwrap().is_some());
            rt.proceed(Stop::AtBreakpoint).unwrap();
        }
        now += 200;
        rt.run(at(now), Stop::AtBreakpoint).unwrap();
        // Paused in scan 6: T ran at 1.2 s (ET counts from the scan interval IN was first seen in), C counted 3 edges.
        assert_eq!(rt.member("T", "ET"), Some(Value::Time(1_100)));
        assert_eq!(rt.member("C", "CV"), Some(Value::Int(3)));
        // A value changed while paused is kept when execution goes on.
        assert_eq!(get(&rt, "Pulse"), Value::Bool(false));
        rt.set("Mark", Value::Bool(false)).unwrap();
        rt.set("Pulse", Value::Bool(true)).unwrap(); // line 3 (Pulse := NOT Pulse) already ran this scan
        assert_eq!(rt.proceed(Stop::NextStatement).unwrap(), None);
        assert_eq!(rt.member("T", "ET"), Some(Value::Time(1_100)), "replay didn't advance the timer twice");
        assert_eq!(get(&rt, "Mark"), Value::Bool(true), "line 5 ran after the edit");
        assert_eq!(get(&rt, "Pulse"), Value::Bool(true), "the edit survived the replay, not re-derived by line 3");
        assert_eq!(rt.member("C", "CV"), Some(Value::Int(3)), "the counter didn't count twice");
        rt.reset();
        assert!(rt.location().is_none(), "STOP drops the stopped scan");
    }

    #[test]
    fn a_line_with_several_statements_stops_once() {
        let mut rt = debug_project(&[("Main", "PROGRAM Main VAR a : INT; END_VAR\na := 1; a := a + 1; a := a + 1;\nEND_PROGRAM")], &[("Main", 2)]);
        rt.run(NO_TIME, Stop::AtBreakpoint).unwrap();
        assert_eq!(rt.proceed(Stop::AtBreakpoint).unwrap(), None);
        assert_eq!(get(&rt, "a"), Value::Int(3));
    }

    #[test]
    fn an_edit_while_paused_is_not_recomputed_by_the_replay() {
        let src = "PROGRAM Main VAR Counter : INT; Done : BOOL; END_VAR\nCounter := Counter + 1;\nDone := TRUE;\nDone := FALSE;\nEND_PROGRAM";
        let mut rt = debug_project(&[("Main", src)], &[("Main", 3), ("Main", 4)]);
        rt.run(NO_TIME, Stop::AtBreakpoint).unwrap(); // before line 3, Counter = 1
        rt.set("Counter", Value::Int(10)).unwrap();
        assert_eq!(at_line(rt.proceed(Stop::AtBreakpoint).unwrap()), Some(("Main".into(), 4)));
        assert_eq!(get(&rt, "Counter"), Value::Int(10), "not 11: line 2 isn't applied to the edit again");
        rt.proceed(Stop::AtBreakpoint).unwrap(); // a second replay re-applies it too
        assert_eq!(get(&rt, "Counter"), Value::Int(10));
        rt.run(NO_TIME, Stop::Never).unwrap();
        assert_eq!(get(&rt, "Counter"), Value::Int(11), "the next scan starts from the edited value");
    }
}
