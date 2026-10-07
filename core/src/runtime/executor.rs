//! The ST interpreter: runtime memory, statement execution, expression evaluation, and
//! standard and user function block calls. One call of `execute_programs` is the EXECUTE
//! step of a scan: every program once, in project order.

use std::collections::HashMap;

use super::debugger::Trace;
use crate::compiler::ast::{Expr, ExprKind, FunctionBlockDef, Stmt, UnaryOp, VarDecl, VarType};
use crate::compiler::error::{ErrorKind, StError, StResult};
use crate::runtime::function_blocks::{FbInstance, ScanClock};
use crate::compiler::functions::StdFunction;
use crate::compiler::symbols::{key, FbInterface, SymbolTable};
use crate::compiler::value::Value;

/// Upper bound on loop iterations (FOR, WHILE, REPEAT combined) in one scan. A PLC scan
/// must finish; past this the program stops with an error instead of freezing the IDE.
pub const MAX_LOOP_ITERATIONS_PER_SCAN: u64 = 100_000;

/// Everything the project remembers between scans: plain variables and the internal
/// state of function block instances, keyed by slot (see `slot`).
#[derive(Debug, Clone, Default)]
pub(super) struct Memory {
    pub(super) values: HashMap<String, Value>,
    pub(super) instances: HashMap<String, FbInstance>,
}

/// Upper-cased name → memory slot, for every name one program can use.
pub(super) type Slots = HashMap<String, String>;

/// Where program `index` keeps its variable `name`. Each program has its own slots, so
/// same-named variables (and FB instances) of different programs stay independent. The
/// I/O addresses use the index after the last program.
pub(super) fn slot(index: usize, name: &str) -> String {
    format!("{index}.{}", key(name))
}

impl Memory {
    pub(super) fn value(&self, slots: &Slots, name: &str) -> Option<Value> {
        self.values.get(slots.get(&key(name))?).copied()
    }

    fn value_mut(&mut self, slots: &Slots, name: &str) -> Option<&mut Value> {
        self.values.get_mut(slots.get(&key(name))?)
    }

    pub(super) fn instance(&self, slots: &Slots, name: &str) -> Option<&FbInstance> {
        self.instances.get(slots.get(&key(name))?)
    }

    fn instance_mut(&mut self, slots: &Slots, name: &str) -> Option<&mut FbInstance> {
        self.instances.get_mut(slots.get(&key(name))?)
    }
}

/// One compiled program, as the runtime executes it.
pub(super) struct Unit {
    pub(super) name: String,
    pub(super) body: Vec<Stmt>,
    pub(super) symbols: SymbolTable,
    /// Its own variables plus the ones it shares from other programs and the I/O addresses.
    pub(super) slots: Slots,
}

/// The project's FUNCTION_BLOCKs as the runtime executes them. An instance's members live
/// in the shared memory under the instance's slot ("0.MOTOR1.START", …), so every instance
/// has its own state, which persists between scans like any variable.
#[derive(Default)]
pub(super) struct UserFbs {
    /// Checked definitions (bodies lowered), indexed by `VarType::UserFb`.
    pub(super) defs: Vec<FunctionBlockDef>,
    pub(super) interfaces: Vec<FbInterface>,
    /// Instance slot → (block index, slots of its members by name).
    pub(super) instances: HashMap<String, (usize, Slots)>,
    /// Each block's source file id (see `Runtime::files`).
    pub(super) file_ids: Vec<usize>,
}

/// What a statement runs in, for the debugger: its source file, the program whose scan
/// is running, and the user block instance (if any) executing it.
#[derive(Debug, Clone)]
pub(super) struct Ctx {
    pub(super) file: usize,
    pub(super) program: usize,
    pub(super) fb: Option<usize>,
    pub(super) instance: Option<String>,
}

/// Create the memory of an instance of user block `index` at `prefix` (its slot): each
/// member gets its initial value, nested blocks their own instances.
pub(super) fn instantiate(index: usize, prefix: &str, fbs: &mut UserFbs, memory: &mut Memory) -> StResult<()> {
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

impl From<&VarDecl> for VarDeclInfo {
    fn from(decl: &VarDecl) -> Self {
        Self { name: decl.name.clone(), var_type: decl.var_type, init: decl.init.clone() }
    }
}

/// A scan failed in `program`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgramError {
    pub program: String,
    pub error: StError,
}

/// The EXECUTE step of a scan: every program once, in project order. Stops early, with
/// `trace.hit` set, when the debugger stops before a statement. The loop budget is shared
/// by the whole scan, since the whole scan must finish.
pub(super) fn execute_programs(
    units: &[Unit],
    memory: &mut Memory,
    fbs: &UserFbs,
    clock: ScanClock,
    trace: &mut Trace,
) -> Result<(), ProgramError> {
    let mut iterations = 0;
    for (index, unit) in units.iter().enumerate() {
        // A program's file id is its index: programs come first in `Runtime::files`.
        let ctx = Ctx { file: index, program: index, fb: None, instance: None };
        let result = {
            let mut exec = Exec { memory: &mut *memory, slots: &unit.slots, clock, iterations, ctx, trace: &mut *trace, fbs };
            let result = exec.block(&unit.body);
            iterations = exec.iterations;
            result
        };
        if trace.hit.is_some() {
            return Ok(());
        }
        result.map_err(|error| ProgramError { program: unit.name.clone(), error })?;
    }
    Ok(())
}

/// Executes statements, loops (within the iteration budget) and block calls.
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

pub(super) fn eval(expr: &Expr, memory: &Memory, slots: &Slots) -> StResult<Value> {
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
