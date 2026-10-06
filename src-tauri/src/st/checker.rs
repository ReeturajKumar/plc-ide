//! Static checking between parsing and execution.
//!
//! Validates names and types once, before any scan runs, and lowers the AST so every
//! numeric literal becomes a concrete, range-checked `Value` and every implicit widening
//! becomes an explicit `Convert` node. After this pass the runtime only ever combines
//! values of the same type.
//!
//! Conversion rules (IEC 61131-3 Ed. 3 "safe" implicit conversions):
//! - INT widens to DINT and to REAL where the context needs it; both are exact.
//! - Nothing narrows (DINT → INT, REAL → INT/DINT) and DINT → REAL is not implicit
//!   because it can lose precision; these are type errors.
//! - An operation's type comes from its operands: INT + INT is an INT addition (and can
//!   overflow as INT) even when the result is stored in a DINT; INT + DINT widens the INT
//!   operand and adds as DINT.
//! - Numeric literals take the type their context needs, range-checked
//!   (`Counter + 1` is INT + INT). An integer literal may become INT, DINT or REAL;
//!   a REAL literal only REAL.
//! - BOOL and TIME never convert to or from anything.
//!
//! Error collection: each statement is checked independently and every problem found is
//! returned, in source order, so one run reports them all. Within a single expression only
//! the first problem is reported, since later ones are usually consequences of it.

use std::collections::HashSet;

use super::ast::{FunctionBlockDef, VarDecl, 
    BinaryOp, CallArg, CaseBranch, DataType, Expr, ExprKind, IfBranch, Program, Stmt, UnaryOp, VarType,
};
use super::error::{ErrorKind, StError, StErrors, StResult};
use super::function_blocks::FbKind;
use super::functions::StdFunction;
pub use super::symbols::key;
use super::symbols::{FbInterface, Scope, SymbolTable};
use crate::io::{self, Mapping};
use super::value::Value;

fn err(kind: ErrorKind, line: usize, column: usize, message: impl Into<String>) -> StError {
    StError::new(kind, line, column, message)
}

/// Static type of an expression. Numeric literals are untyped until context decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Known(DataType),
    AnyInt,
    AnyReal,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Known(t) => t.name(),
            Ty::AnyInt => "integer literal",
            Ty::AnyReal => "REAL literal",
        }
    }

    fn is_numeric(self) -> bool {
        match self {
            Ty::Known(t) => t.is_numeric(),
            Ty::AnyInt | Ty::AnyReal => true,
        }
    }

    fn is_integer(self) -> bool {
        match self {
            Ty::Known(t) => t.is_integer(),
            Ty::AnyInt => true,
            Ty::AnyReal => false,
        }
    }

    /// The concrete type used when nothing else constrains a literal (e.g. `1 < 2`).
    fn concrete(self) -> DataType {
        match self {
            Ty::Known(t) => t,
            Ty::AnyInt => DataType::DInt,
            Ty::AnyReal => DataType::Real,
        }
    }
}

/// The common type two operands can share, or None if they're incompatible.
fn unify(a: Ty, b: Ty) -> Option<Ty> {
    match (a, b) {
        _ if a == b => Some(a),
        (Ty::Known(x), Ty::Known(y)) if x.widens_to(y) => Some(b),
        (Ty::Known(x), Ty::Known(y)) if y.widens_to(x) => Some(a),
        (Ty::AnyInt, Ty::Known(t)) | (Ty::Known(t), Ty::AnyInt) if t.is_numeric() => Some(Ty::Known(t)),
        (Ty::AnyReal, Ty::Known(t)) | (Ty::Known(t), Ty::AnyReal) if t.widens_to(DataType::Real) => {
            Some(Ty::Known(DataType::Real))
        }
        (Ty::AnyInt, Ty::AnyReal) | (Ty::AnyReal, Ty::AnyInt) => Some(Ty::AnyReal),
        _ => None,
    }
}

/// A program that passed semantic analysis, ready for the runtime.
pub struct Checked {
    /// Literals typed and widenings made explicit.
    pub program: Program,
    pub symbols: SymbolTable,
}

/// Semantic analysis of a single program: build the symbol table, then check every
/// declaration and statement. Returns all errors found (in source order) if there are any.
#[cfg(test)]
pub fn check(program: Program) -> Result<Checked, StErrors> {
    let name = program.name.clone();
    check_project(vec![(name, program)], Vec::new(), &[]).programs.remove(0)
}

/// A FUNCTION_BLOCK that passed semantic analysis.
pub struct CheckedFb {
    /// Initial values typed, body lowered.
    pub def: FunctionBlockDef,
}

/// The analysis of a whole project: one result per program and per FUNCTION_BLOCK, the
/// I/O mapping errors, and the function block types (indexed by `VarType::UserFb`).
pub struct ProjectCheck {
    pub programs: Vec<Result<Checked, StErrors>>,
    pub function_blocks: Vec<Result<CheckedFb, StErrors>>,
    pub mapping: StErrors,
    pub interfaces: Vec<FbInterface>,
}

/// Name of the I/O address table in error messages.
pub const IO_SCOPE_NAME: &str = "I/O";

/// Every symbol table of a project: the programs' in order, then the I/O addresses, which
/// every program can use (`IF DI0 THEN`, `AO0 := x;`).
pub fn with_io(mut tables: Vec<SymbolTable>, mut names: Vec<String>) -> (Vec<SymbolTable>, Vec<String>) {
    tables.push(io::CONFIG.symbols());
    names.push(IO_SCOPE_NAME.to_string());
    (tables, names)
}

/// Semantic analysis of a project. The FUNCTION_BLOCKs are registered first, as types the
/// programs (and other blocks) can instantiate; each block is checked on its own, seeing
/// only its members. Then every program, in order, gets one result with all of its errors;
/// a program can use the variables another program declares, as long as exactly one
/// program declares that name, and the PLC I/O addresses. Last, the I/O `mappings`.
pub fn check_project(programs: Vec<(String, Program)>, mut fbs: Vec<FunctionBlockDef>, mappings: &[Mapping]) -> ProjectCheck {
    // Block inputs that I/O mappings feed (only programs' instances can be mapped).
    let mapped_inputs = io::mapped_block_inputs(mappings, &io::CONFIG);
    let no_mapped_inputs = HashSet::new();

    // --- function blocks: registry, members, recursion, bodies ---
    let registry: Vec<String> = fbs.iter().map(|fb| fb.name.clone()).collect();
    let mut fb_errors = vec![StErrors::new(); fbs.len()];
    for (i, fb) in fbs.iter().enumerate() {
        let at = |message: String| err(ErrorKind::DuplicateDeclaration, fb.line, fb.column, message);
        if FbKind::from_name(&fb.name).is_some() {
            fb_errors[i].push(at(format!("'{}' is a standard function block; give yours another name", fb.name)));
        } else if registry[..i].iter().any(|n| n.eq_ignore_ascii_case(&fb.name)) {
            fb_errors[i].push(at(format!("Function block '{}' is defined more than once", fb.name)));
        }
    }
    let mut fb_tables = Vec::new();
    for (fb, errors) in fbs.iter_mut().zip(&mut fb_errors) {
        let mut table = SymbolTable::default();
        declare_into(&mut fb.inputs, &mut table, errors, &registry, true);
        declare_into(&mut fb.outputs, &mut table, errors, &registry, true);
        declare_into(&mut fb.vars, &mut table, errors, &registry, false);
        fb_tables.push(table);
    }
    let interfaces: Vec<FbInterface> = fbs.iter().map(interface).collect();
    for (i, cycle) in recursion(&fbs) {
        let fb = &fbs[i];
        fb_errors[i].push(err(
            ErrorKind::InvalidFunctionBlock,
            fb.line,
            fb.column,
            format!("Function block '{}' contains itself ({cycle}); recursive function blocks are not supported", fb.name),
        ));
    }
    for (i, fb) in fbs.iter_mut().enumerate() {
        let scope = Scope::new(&fb_tables[i..=i], &registry[i..=i], 0).with_fbs(&interfaces);
        let mut checker =
            Checker { symbols: &scope, mapped: &no_mapped_inputs, loop_vars: Vec::new(), errors: std::mem::take(&mut fb_errors[i]) };
        fb.body = checker.block(&fb.body);
        fb_errors[i] = checker.errors;
    }

    // --- programs ---
    let (names, mut programs): (Vec<String>, Vec<Program>) = programs.into_iter().unzip();
    let (tables, mut errors): (Vec<SymbolTable>, Vec<StErrors>) =
        programs.iter_mut().map(|program| declarations(&mut program.vars, &registry)).unzip();
    let (mut tables, names) = with_io(tables, names);

    for (i, program) in programs.iter_mut().enumerate() {
        let scope = Scope::new(&tables, &names, i).with_fbs(&interfaces);
        let mut checker = Checker { symbols: &scope, mapped: &mapped_inputs, loop_vars: Vec::new(), errors: std::mem::take(&mut errors[i]) };
        program.body = checker.block(&program.body);
        errors[i] = checker.errors;
    }

    let io_index = tables.len() - 1;
    let io_scope = Scope::new(&tables, &names, io_index).with_fbs(&interfaces);
    let mapping = io::validate(mappings, &io::CONFIG, &io_scope, &tables[io_index]);
    tables.pop();
    let programs = programs
        .into_iter()
        .zip(tables)
        .zip(errors)
        .map(|((program, symbols), errors)| if errors.is_empty() { Ok(Checked { program, symbols }) } else { Err(errors) })
        .collect();
    let function_blocks = fbs
        .into_iter()
        .zip(fb_errors)
        .map(|(def, errors)| if errors.is_empty() { Ok(CheckedFb { def }) } else { Err(errors) })
        .collect();
    ProjectCheck { programs, function_blocks, mapping, interfaces }
}

/// What callers see of a FUNCTION_BLOCK. An input without an initial value is required.
fn interface(fb: &FunctionBlockDef) -> FbInterface {
    let elementary = |decl: &VarDecl| match decl.var_type {
        VarType::Elementary(t) => Some((decl.name.clone(), t)),
        _ => None,
    };
    FbInterface {
        name: fb.name.clone(),
        user: true,
        inputs: fb.inputs.iter().filter_map(|d| elementary(d).map(|(n, t)| (n, t, d.init.is_none()))).collect(),
        outputs: fb.outputs.iter().filter_map(elementary).collect(),
        internals: fb.vars.iter().map(|d| d.name.clone()).collect(),
    }
}

/// Blocks that contain an instance of themselves, directly or through others, with the
/// chain (e.g. "A → B → A"). Such a block would need infinite memory.
fn recursion(fbs: &[FunctionBlockDef]) -> Vec<(usize, String)> {
    let uses = |i: usize| -> Vec<usize> {
        fbs[i]
            .members()
            .filter_map(|d| match d.var_type {
                VarType::UserFb(j) if j < fbs.len() => Some(j),
                _ => None,
            })
            .collect()
    };
    fn path_to(target: usize, from: usize, uses: &dyn Fn(usize) -> Vec<usize>, seen: &mut Vec<usize>) -> Option<Vec<usize>> {
        for next in uses(from) {
            if next == target {
                return Some(vec![next]);
            }
            if !seen.contains(&next) {
                seen.push(next);
                if let Some(mut path) = path_to(target, next, uses, seen) {
                    path.insert(0, next);
                    return Some(path);
                }
            }
        }
        None
    }
    (0..fbs.len())
        .filter_map(|i| {
            let path = path_to(i, i, &uses, &mut Vec::new())?;
            let chain = std::iter::once(i).chain(path).map(|j| fbs[j].name.as_str()).collect::<Vec<_>>().join(" → ");
            Some((i, chain))
        })
        .collect()
}

/// Build a program's symbol table and check its declarations (initial values are lowered).
fn declarations(vars: &mut [VarDecl], registry: &[String]) -> (SymbolTable, StErrors) {
    let mut errors = StErrors::new();
    let mut symbols = SymbolTable::default();
    declare_into(vars, &mut symbols, &mut errors, registry, false);
    (symbols, errors)
}

/// Declare `vars` in `symbols`: resolve FUNCTION_BLOCK type names against `registry`,
/// refuse duplicates, and check initial values. A function block's inputs and outputs
/// (`values_only`) must be plain values.
fn declare_into(vars: &mut [VarDecl], symbols: &mut SymbolTable, errors: &mut StErrors, registry: &[String], values_only: bool) {
    let constants = Scope::empty(); // initial values can't reference variables

    for decl in vars.iter_mut() {
        if decl.var_type == VarType::UNRESOLVED {
            match registry.iter().position(|n| n.eq_ignore_ascii_case(&decl.type_name)) {
                Some(index) => decl.var_type = VarType::UserFb(index),
                None => {
                    errors.push(err(
                        ErrorKind::InvalidFunctionBlock,
                        decl.line,
                        decl.column,
                        format!("Unknown data type or function block type '{}' for '{}'", decl.type_name, decl.name),
                    ));
                    continue;
                }
            }
        }
        if values_only && !matches!(decl.var_type, VarType::Elementary(_)) {
            errors.push(err(
                ErrorKind::InvalidFunctionBlock,
                decl.line,
                decl.column,
                format!(
                    "Function block inputs and outputs must be BOOL, INT, DINT, REAL or TIME; declare the {} instance '{}' under VAR",
                    decl.type_name, decl.name
                ),
            ));
            continue;
        }
        if io::CONFIG.kind_of(&decl.name).is_some() {
            errors.push(err(
                ErrorKind::DuplicateDeclaration,
                decl.line,
                decl.column,
                format!("'{}' is a PLC I/O address and can't be declared as a variable; use it directly or map a variable to it", decl.name),
            ));
            continue;
        }
        if let Err(original) = symbols.declare(&decl.name, decl.var_type, decl.line, decl.column) {
            errors.push(err(
                ErrorKind::DuplicateDeclaration,
                decl.line,
                decl.column,
                format!(
                    "Duplicate declaration of variable '{}' (first declared at line {}, column {})",
                    decl.name, original.line, original.column
                ),
            ));
            continue;
        }
        let Some(init) = &decl.init else { continue };
        let data_type = match decl.var_type {
            VarType::Elementary(t) => t,
            other => {
                let name = match other {
                    VarType::FunctionBlock(kind) => kind.name().to_string(),
                    _ => decl.type_name.clone(),
                };
                errors.push(err(
                    ErrorKind::InvalidAssignment,
                    init.line,
                    init.column,
                    format!("A {name} instance can't have an initial value; set its inputs in a call"),
                ));
                continue;
            }
        };
        if let Some(var) = first_variable(init) {
            errors.push(err(
                ErrorKind::InvalidAssignment,
                var.line,
                var.column,
                format!("Initial value of '{}' must be a constant", decl.name),
            ));
            continue;
        }
        match assignable(init, data_type, &decl.name, &constants) {
            Ok(lowered) => decl.init = Some(lowered),
            Err(e) => errors.push(e),
        }
    }
}

struct Checker<'a> {
    symbols: &'a Scope<'a>,
    /// Block inputs fed by I/O mappings ("INSTANCE.MEMBER"): given without the call.
    mapped: &'a HashSet<String>,
    /// Control variables of the FOR loops enclosing the current statement. IEC forbids
    /// assigning them inside the loop, since that would silently change its iteration.
    loop_vars: Vec<String>,
    errors: StErrors,
}

impl Checker<'_> {
    /// Keep a successful result; record an error and carry on.
    fn ok<T>(&mut self, result: StResult<T>) -> Option<T> {
        result.map_err(|e| self.errors.push(e)).ok()
    }

    fn report(&mut self, error: StError) {
        self.errors.push(error);
    }

    /// Check every statement, even after errors, so all problems are reported at once.
    fn block(&mut self, body: &[Stmt]) -> Vec<Stmt> {
        body.iter().filter_map(|stmt| self.stmt(stmt)).collect()
    }

    /// The checked statement, or None if it had errors (which are recorded).
    fn stmt(&mut self, stmt: &Stmt) -> Option<Stmt> {
        let symbols = self.symbols;
        match stmt {
            Stmt::Assign { target, value, line, column } => {
                let target_type = match symbols.get(target).map(|s| (s.var_type, s.read_only)) {
                    Some((_, true)) => {
                        self.report(err(
                            ErrorKind::InvalidAssignment,
                            *line,
                            *column,
                            format!("Can't assign to '{target}': PLC inputs are read-only in programs"),
                        ));
                        None
                    }
                    Some((VarType::Elementary(t), false)) => Some(t),
                    Some((other, false)) => {
                        self.report(err(
                            ErrorKind::InvalidAssignment,
                            *line,
                            *column,
                            format!("Can't assign to '{target}': it is a {} instance", symbols.type_name(other)),
                        ));
                        None
                    }
                    None => {
                        self.report(err(ErrorKind::UndeclaredVariable, *line, *column, symbols.undeclared(target, "variable")));
                        None
                    }
                };
                if self.loop_vars.contains(&key(target)) {
                    self.report(err(
                        ErrorKind::InvalidLoop,
                        *line,
                        *column,
                        format!("Can't assign to '{target}' inside the FOR loop it controls"),
                    ));
                    return None;
                }
                let Some(target_type) = target_type else {
                    // Still report problems on the right-hand side.
                    self.ok(infer(value, symbols));
                    return None;
                };
                symbols.mark_written(target);
                let value = self.ok(assignable(value, target_type, target, symbols))?;
                Some(Stmt::Assign { target: target.clone(), value, line: *line, column: *column })
            }
            Stmt::AssignMember { instance, member, value, line, column } => {
                if let Some(fb) = self.ok(function_block(instance, *line, *column, symbols)) {
                    let (error_kind, message) = if fb.is_output(member) && fb.user {
                        (ErrorKind::InvalidFunctionBlockOutput, format!("Cannot assign to Function Block output '{member}'."))
                    } else if fb.is_output(member) {
                        (
                            ErrorKind::InvalidFunctionBlockOutput,
                            format!("'{instance}.{member}' is an output of {} and is read-only", fb.name),
                        )
                    } else if fb.input_type(member).is_some() {
                        (
                            ErrorKind::InvalidFunctionBlockInput,
                            format!("Set {} inputs in a call, e.g. {instance}({member} := …);", fb.name),
                        )
                    } else if fb.is_internal(member) {
                        (ErrorKind::InvalidFunctionBlock, internal(&fb, member))
                    } else {
                        (ErrorKind::InvalidFunctionBlock, format!("{} has no member '{member}'", fb.name))
                    };
                    self.report(err(error_kind, *line, *column, message));
                }
                self.ok(infer(value, symbols));
                None
            }
            Stmt::If { branches, else_branch, line, column } => {
                let mut checked = Vec::new();
                let mut failed = false;
                for branch in branches {
                    let condition = self.ok(condition(&branch.condition, "IF", symbols));
                    let body = self.block(&branch.body);
                    match condition {
                        Some(condition) => checked.push(IfBranch { condition, body }),
                        None => failed = true,
                    }
                }
                let else_branch = self.block(else_branch);
                (!failed).then_some(Stmt::If { branches: checked, else_branch, line: *line, column: *column })
            }
            Stmt::For { var, start, end, step, body, line, column } => {
                let at = |message: String| err(ErrorKind::InvalidLoop, *line, *column, message);
                let var_type = match symbols.get(var).map(|s| s.var_type) {
                    Some(VarType::Elementary(t)) if t.is_integer() => Some(t),
                    Some(other) => {
                        let found = symbols.type_name(other);
                        self.report(at(format!("FOR loop variable '{var}' must be INT or DINT, found {found}")));
                        None
                    }
                    None => {
                        self.report(err(
                            ErrorKind::UndeclaredVariable,
                            *line,
                            *column,
                            symbols.undeclared(var, "FOR loop variable"),
                        ));
                        None
                    }
                };
                let nested_reuse = self.loop_vars.contains(&key(var));
                if nested_reuse {
                    self.report(at(format!("'{var}' already controls an enclosing FOR loop")));
                }
                // Bounds are checked against the loop variable's type when it is valid,
                // otherwise just for their own errors.
                let bound = |me: &mut Self, e: &Expr, what: &str| match var_type {
                    Some(t) => me.ok(assignable(e, t, what, symbols)),
                    None => {
                        me.ok(infer(e, symbols));
                        None
                    }
                };
                let start = bound(self, start, var);
                let end = bound(self, end, &format!("{var} end"));
                let step = match step {
                    Some(s) => match bound(self, s, &format!("{var} step")) {
                        Some(Expr { kind: ExprKind::Const(v), line, column }) if v.as_integer() == Some(0) => {
                            self.report(err(ErrorKind::InvalidLoop, line, column, "FOR step can't be 0: the loop would never end"));
                            None
                        }
                        Some(s) => Some(Some(s)),
                        None => None,
                    },
                    None => Some(None),
                };
                if var_type.is_some() {
                    symbols.mark_written(var);
                    self.loop_vars.push(key(var));
                }
                let body = self.block(body);
                if var_type.is_some() {
                    self.loop_vars.pop();
                }
                match (start, end, step, nested_reuse) {
                    (Some(start), Some(end), Some(step), false) if var_type.is_some() => {
                        Some(Stmt::For { var: var.clone(), start, end, step, body, line: *line, column: *column })
                    }
                    _ => None,
                }
            }
            Stmt::While { condition: c, body, line, column } => {
                let condition = self.ok(condition(c, "WHILE", symbols));
                let body = self.block(body);
                Some(Stmt::While { condition: condition?, body, line: *line, column: *column })
            }
            Stmt::Repeat { body, condition: c, line, column } => {
                let body = self.block(body);
                let condition = self.ok(condition(c, "UNTIL", symbols));
                Some(Stmt::Repeat { body, condition: condition?, line: *line, column: *column })
            }
            Stmt::Case { selector, branches, else_branch, line, column } => {
                let selector_type = match self.ok(infer(selector, symbols)) {
                    Some(ty) if ty.is_integer() => Some(ty.concrete()),
                    Some(ty) => {
                        self.report(err(
                            ErrorKind::InvalidCase,
                            selector.line,
                            selector.column,
                            format!("CASE selector must be INT or DINT, found {}", ty.name()),
                        ));
                        None
                    }
                    None => None,
                };
                let mut seen = HashSet::new();
                let mut failed = selector_type.is_none();
                let mut checked = Vec::new();
                for branch in branches {
                    for label in &branch.labels {
                        let at = |message: String| err(ErrorKind::InvalidCase, label.line, label.column, message);
                        if let Some(t) = selector_type {
                            if let Err(message) = int_constant(label.value, t) {
                                self.report(at(message));
                                failed = true;
                            }
                        }
                        if !seen.insert(label.value) {
                            self.report(at(format!("Duplicate CASE label {}", label.value)));
                            failed = true;
                        }
                    }
                    checked.push(CaseBranch { labels: branch.labels.clone(), body: self.block(&branch.body) });
                }
                let else_branch = self.block(else_branch);
                let selector = self.ok(lower(selector, selector_type?, symbols))?;
                (!failed).then_some(Stmt::Case { selector, branches: checked, else_branch, line: *line, column: *column })
            }
            Stmt::Call { instance, args, line, column } => {
                if symbols.get(instance).is_none() {
                    let message = match StdFunction::from_name(instance) {
                        Some(f) => format!("{} is a function: use its result, e.g. x := {}(…);", f.name(), f.name()),
                        None => format!("Unknown function or function block instance '{instance}'"),
                    };
                    self.report(err(ErrorKind::InvalidFunction, *line, *column, message));
                    for arg in args {
                        self.ok(infer(&arg.value, symbols));
                    }
                    return None;
                }
                let fb = self.ok(function_block(instance, *line, *column, symbols))?;
                let mut given = HashSet::new();
                let mut checked = Vec::new();
                for arg in args {
                    let at = |message: String| err(ErrorKind::InvalidFunctionBlockInput, arg.line, arg.column, message);
                    let Some(input_type) = fb.input_type(&arg.name) else {
                        let message = if fb.is_output(&arg.name) {
                            format!("'{}' is an output of {}; read it as {instance}.{}", arg.name, fb.name, arg.name)
                        } else if fb.user {
                            format!("Unknown input '{}' for Function Block '{}'.", arg.name, fb.name)
                        } else {
                            format!("{} has no input '{}' (inputs: {})", fb.name, arg.name, fb.input_names())
                        };
                        self.report(at(message));
                        self.ok(infer(&arg.value, symbols));
                        continue;
                    };
                    if self.mapped.contains(&format!("{}.{}", key(instance), key(&arg.name))) {
                        self.report(at(format!(
                            "'{instance}.{}' already comes from an I/O mapping; remove it from this call or from the I/O tab",
                            arg.name
                        )));
                        continue;
                    }
                    if !given.insert(key(&arg.name)) {
                        self.report(at(format!("Input '{}' is given more than once", arg.name)));
                        continue;
                    }
                    let target = format!("{instance}.{}", arg.name);
                    if let Some(value) = self.ok(assignable(&arg.value, input_type, &target, symbols)) {
                        checked.push(CallArg { value, ..arg.clone() });
                    }
                }
                // A user block's inputs without an initial value must be given in every call.
                let missing: Vec<&String> =
                    fb.inputs
                        .iter()
                        .filter(|(n, _, required)| {
                            *required && !given.contains(&key(n)) && !self.mapped.contains(&format!("{}.{}", key(instance), key(n)))
                        })
                        .map(|(n, _, _)| n)
                        .collect();
                for name in &missing {
                    self.report(err(
                        ErrorKind::InvalidFunctionBlockInput,
                        *line,
                        *column,
                        format!("Missing required input '{name}' for Function Block '{}'.", fb.name),
                    ));
                }
                (checked.len() == args.len() && missing.is_empty())
                    .then_some(Stmt::Call { instance: instance.clone(), args: checked, line: *line, column: *column })
            }
        }
    }
}

/// A condition of IF/ELSIF/WHILE/UNTIL: must be BOOL.
fn condition(expr: &Expr, keyword: &str, symbols: &Scope) -> StResult<Expr> {
    let ty = infer(expr, symbols)?;
    if ty != Ty::Known(DataType::Bool) {
        return Err(err(
            ErrorKind::InvalidCondition,
            expr.line,
            expr.column,
            format!("{keyword} condition must be BOOL, found {}", ty.name()),
        ));
    }
    lower(expr, DataType::Bool, symbols)
}

/// Look up `name` as a function block instance (standard or user-defined).
fn function_block(name: &str, line: usize, column: usize, symbols: &Scope) -> StResult<FbInterface> {
    match symbols.get(name).map(|s| s.var_type) {
        Some(t) => symbols.interface(t).ok_or_else(|| {
            err(
                ErrorKind::InvalidFunctionBlock,
                line,
                column,
                format!("'{name}' is not a function block instance (it is {})", symbols.type_name(t)),
            )
        }),
        None => Err(err(ErrorKind::UndeclaredVariable, line, column, symbols.undeclared(name, "variable"))),
    }
}

/// A user block's internal variables belong to each instance; programs can't touch them.
fn internal(fb: &FbInterface, member: &str) -> String {
    format!("'{member}' is internal to function block {}; only its inputs and outputs are accessible", fb.name)
}

/// Check that `value` can be stored in a variable of `target_type`, and lower it.
fn assignable(value: &Expr, target_type: DataType, target: &str, symbols: &Scope) -> StResult<Expr> {
    let ty = infer(value, symbols)?;
    if unify(Ty::Known(target_type), ty) != Some(Ty::Known(target_type)) {
        return Err(err(
            ErrorKind::TypeMismatch,
            value.line,
            value.column,
            format!("Type mismatch: cannot assign {} to '{target}' ({})", ty.name(), target_type.name()),
        ));
    }
    lower(value, target_type, symbols)
}

fn infer(expr: &Expr, symbols: &Scope) -> StResult<Ty> {
    let error = |kind: ErrorKind, message: String| err(kind, expr.line, expr.column, message);
    match &expr.kind {
        ExprKind::Const(v) => Ok(Ty::Known(v.data_type())),
        ExprKind::IntLiteral(_) => Ok(Ty::AnyInt),
        ExprKind::RealLiteral(_) => Ok(Ty::AnyReal),
        ExprKind::Convert { to, .. } => Ok(Ty::Known(*to)),
        ExprKind::Variable(name) => {
            match symbols.get(name).map(|s| s.var_type) {
                Some(VarType::Elementary(t)) => Ok(Ty::Known(t)),
                Some(other) => {
                    let fb = symbols.interface(other);
                    let message = match fb.as_ref().and_then(|fb| fb.outputs.first()) {
                        Some((output, _)) => format!(
                            "'{name}' is a {} instance; read one of its outputs, e.g. {name}.{output}",
                            symbols.type_name(other)
                        ),
                        None => format!("'{name}' is a {} instance and has no value to read", symbols.type_name(other)),
                    };
                    Err(error(ErrorKind::InvalidFunctionBlock, message))
                }
                None => Err(error(ErrorKind::UndeclaredVariable, symbols.undeclared(name, "variable"))),
            }
        }
        ExprKind::Call { name, args } => {
            let function = StdFunction::from_name(name)
                .ok_or_else(|| error(ErrorKind::InvalidFunction, format!("Unknown function '{name}'")))?;
            let (min, max) = function.arity();
            if args.len() < min || args.len() > max {
                let expected = match (min, max) {
                    (1, 1) => "1 argument".to_string(),
                    (n, usize::MAX) => format!("at least {n} arguments"),
                    (n, m) => format!("{n} to {m} arguments"),
                };
                return Err(error(
                    ErrorKind::InvalidFunctionArguments,
                    format!("{} expects {expected}, found {}", function.name(), args.len()),
                ));
            }
            let types = args.iter().map(|a| infer(a, symbols)).collect::<StResult<Vec<_>>>()?;
            function_type(function, args, &types, expr)
        }
        ExprKind::Member { instance, member } => {
            let fb = function_block(instance, expr.line, expr.column, symbols)?;
            fb.member_type(member).map(Ty::Known).ok_or_else(|| {
                let message = if fb.is_internal(member) {
                    internal(&fb, member)
                } else {
                    format!("{} has no member '{member}' (members: {})", fb.name, fb.member_names())
                };
                error(ErrorKind::InvalidFunctionBlock, message)
            })
        }
        ExprKind::Unary { op: UnaryOp::Not, operand } => match infer(operand, symbols)? {
            Ty::Known(DataType::Bool) => Ok(Ty::Known(DataType::Bool)),
            other => Err(error(ErrorKind::InvalidOperator, format!("Type error: NOT requires BOOL, found {}", other.name()))),
        },
        ExprKind::Unary { op: UnaryOp::Neg, operand } => match infer(operand, symbols)? {
            ty if ty.is_numeric() => Ok(ty),
            other => Err(error(ErrorKind::InvalidOperator, format!("Type error: '-' cannot be applied to {}", other.name()))),
        },
        ExprKind::Binary { op, left, right } => {
            let (l, r) = (infer(left, symbols)?, infer(right, symbols)?);
            let type_error = || {
                error(
                    ErrorKind::InvalidOperator,
                    format!("Type error: '{}' cannot be applied to {} and {}", op.symbol(), l.name(), r.name()),
                )
            };
            let common = unify(l, r).ok_or_else(type_error)?;
            let boolean = common == Ty::Known(DataType::Bool);
            let time = common == Ty::Known(DataType::Time);
            match op {
                BinaryOp::And | BinaryOp::Or | BinaryOp::Xor if boolean => Ok(common),
                BinaryOp::Equal | BinaryOp::NotEqual => Ok(Ty::Known(DataType::Bool)),
                BinaryOp::Less | BinaryOp::Greater | BinaryOp::LessEqual | BinaryOp::GreaterEqual
                    if common.is_numeric() || time =>
                {
                    Ok(Ty::Known(DataType::Bool))
                }
                BinaryOp::Add | BinaryOp::Sub if common.is_numeric() || time => Ok(common),
                BinaryOp::Mul | BinaryOp::Div if common.is_numeric() => Ok(common),
                BinaryOp::Mod if common.is_integer() => Ok(common),
                _ => Err(type_error()),
            }
        }
    }
}

/// Result type of a standard function call, checking its argument types.
/// - ABS: any number, result of the same type.
/// - SQRT, SIN, COS: REAL (INT widens), result REAL. TRUNC, ROUND: REAL, result DINT.
/// - MIN, MAX: two or more numbers or TIMEs of a common type, result that type.
fn function_type(function: StdFunction, args: &[Expr], types: &[Ty], call: &Expr) -> StResult<Ty> {
    let name = function.name();
    let invalid = |line: usize, column: usize, message: String| err(ErrorKind::InvalidFunctionArguments, line, column, message);
    let wrong = |i: usize, expected: &str| {
        invalid(args[i].line, args[i].column, format!("Type error: {name} expects {expected}, found {}", types[i].name()))
    };
    match function {
        StdFunction::Abs if types[0].is_numeric() => Ok(types[0]),
        StdFunction::Abs => Err(wrong(0, "a number (INT, DINT or REAL)")),
        StdFunction::Sqrt | StdFunction::Sin | StdFunction::Cos | StdFunction::Trunc | StdFunction::Round => {
            if unify(Ty::Known(DataType::Real), types[0]) != Some(Ty::Known(DataType::Real)) {
                return Err(wrong(0, "a REAL (or INT)"));
            }
            let integer_result = matches!(function, StdFunction::Trunc | StdFunction::Round);
            Ok(Ty::Known(if integer_result { DataType::DInt } else { DataType::Real }))
        }
        StdFunction::Min | StdFunction::Max => {
            let mut common = types[0];
            for (i, &ty) in types.iter().enumerate().skip(1) {
                common = unify(common, ty).ok_or_else(|| {
                    invalid(
                        args[i].line,
                        args[i].column,
                        format!("Type error: {name} arguments must have compatible types, found {} and {}", common.name(), ty.name()),
                    )
                })?;
            }
            if common.is_numeric() || common == Ty::Known(DataType::Time) {
                Ok(common)
            } else {
                Err(invalid(call.line, call.column, format!("Type error: {name} expects numbers or TIME, found {}", common.name())))
            }
        }
    }
}

/// Rewrite `expr` (already type-checked to fit `want`) so literals are typed constants
/// and widenings are explicit `Convert` nodes.
///
/// As in IEC, an operation's type comes from its operands, not from where the result
/// goes: with `i : INT`, `r := i / 7` is an INT division (4285) whose result is then
/// widened to REAL (4285.0). Only literals, which have no type of their own, take the
/// type of their context.
fn lower(expr: &Expr, want: DataType, symbols: &Scope) -> StResult<Expr> {
    if let Ty::Known(natural) = infer(expr, symbols)? {
        if natural != want {
            if !natural.widens_to(want) {
                return Err(err(
                    ErrorKind::TypeMismatch,
                    expr.line,
                    expr.column,
                    format!("Type mismatch: {} where {} is required", natural.name(), want.name()),
                ));
            }
            let inner = lower_as(expr, natural, symbols)?;
            return Ok(Expr { kind: ExprKind::Convert { to: want, operand: Box::new(inner) }, line: expr.line, column: expr.column });
        }
    }
    lower_as(expr, want, symbols)
}

/// Lower `expr` whose natural type is `ty` (or an untyped literal that becomes `ty`).
fn lower_as(expr: &Expr, ty: DataType, symbols: &Scope) -> StResult<Expr> {
    let kind = match &expr.kind {
        ExprKind::IntLiteral(n) => ExprKind::Const(
            int_constant(*n, ty).map_err(|message| err(ErrorKind::TypeMismatch, expr.line, expr.column, message))?,
        ),
        ExprKind::RealLiteral(x) => ExprKind::Const(Value::Real(*x)),
        ExprKind::Variable(_) | ExprKind::Member { .. } | ExprKind::Const(_) | ExprKind::Convert { .. } => expr.kind.clone(),
        ExprKind::Call { name, args } => {
            // ABS, MIN and MAX compute in their result type; the others take REAL.
            let arg_type = match StdFunction::from_name(name) {
                Some(StdFunction::Abs | StdFunction::Min | StdFunction::Max) => ty,
                _ => DataType::Real,
            };
            ExprKind::Call {
                name: name.clone(),
                args: args.iter().map(|a| lower(a, arg_type, symbols)).collect::<StResult<_>>()?,
            }
        }
        ExprKind::Unary { op: UnaryOp::Not, operand } => {
            ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(lower(operand, DataType::Bool, symbols)?) }
        }
        ExprKind::Unary { op: UnaryOp::Neg, operand } => {
            ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(lower(operand, ty, symbols)?) }
        }
        ExprKind::Binary { op, left, right } => {
            // Arithmetic runs in the expression's own type (operands widened to it);
            // comparison and logic operands use their common type, whatever the BOOL result.
            let operand_type = match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => ty,
                _ => unify(infer(left, symbols)?, infer(right, symbols)?).map(Ty::concrete).unwrap_or(DataType::Bool),
            };
            ExprKind::Binary {
                op: *op,
                left: Box::new(lower(left, operand_type, symbols)?),
                right: Box::new(lower(right, operand_type, symbols)?),
            }
        }
    };
    Ok(Expr { kind, line: expr.line, column: expr.column })
}

fn int_constant(n: i64, want: DataType) -> Result<Value, String> {
    let out_of_range = || format!("Value {n} is out of range for {}", want.name());
    match want {
        DataType::Int => i16::try_from(n).map(Value::Int).map_err(|_| out_of_range()),
        DataType::DInt => i32::try_from(n).map(Value::DInt).map_err(|_| out_of_range()),
        DataType::Real => Ok(Value::Real(n as f64)),
        other => Err(format!("Type mismatch: integer literal where {} is required", other.name())),
    }
}

fn first_variable(expr: &Expr) -> Option<&Expr> {
    match &expr.kind {
        ExprKind::Variable(_) | ExprKind::Member { .. } => Some(expr),
        ExprKind::Call { args, .. } => args.iter().find_map(first_variable),
        ExprKind::Unary { operand, .. } | ExprKind::Convert { operand, .. } => first_variable(operand),
        ExprKind::Binary { left, right, .. } => first_variable(left).or_else(|| first_variable(right)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::st::parser::parse;

    /// The checked program, or the first error.
    fn check_src(src: &str) -> StResult<Program> {
        check(parse(src).unwrap()).map(|checked| checked.program).map_err(|mut errors| errors.remove(0))
    }

    fn error(src: &str) -> StError {
        check_src(src).unwrap_err()
    }

    /// Every error in a program.
    fn errors(src: &str) -> StErrors {
        match check(parse(src).unwrap()) {
            Ok(_) => panic!("expected errors"),
            Err(errors) => errors,
        }
    }

    #[test]
    fn accepts_the_acceptance_programs() {
        let vars = "VAR Counter : INT := 10; Speed : INT := 20; Result : INT := 0; END_VAR";
        for body in [
            "Result := Counter + Speed;",
            "Result := Counter * Speed;",
            "Result := Speed - Counter;",
            "IF Counter < Speed THEN Result := 100; ELSE Result := 0; END_IF;",
            "Counter := Counter + 1;",
        ] {
            check_src(&format!("PROGRAM main {vars} {body} END_PROGRAM")).unwrap();
        }
    }

    #[test]
    fn literals_take_the_type_of_their_context() {
        let program = check_src(
            "PROGRAM p VAR i : INT; d : DINT; r : REAL; END_VAR i := i + 1; d := 2; r := 3; END_PROGRAM",
        )
        .unwrap();
        let rhs: Vec<_> = program
            .body
            .iter()
            .map(|s| match s {
                Stmt::Assign { value, .. } => value.clone(),
                other => panic!("{other:?}"),
            })
            .collect();
        match &rhs[0].kind {
            ExprKind::Binary { right, .. } => assert_eq!(right.kind, ExprKind::Const(Value::Int(1))),
            other => panic!("{other:?}"),
        }
        assert_eq!(rhs[1].kind, ExprKind::Const(Value::DInt(2)));
        assert_eq!(rhs[2].kind, ExprKind::Const(Value::Real(3.0)));
    }

    #[test]
    fn rejects_bool_arithmetic() {
        let err = error("PROGRAM p VAR Result : INT; END_VAR\nResult := TRUE + 10;\nEND_PROGRAM");
        assert_eq!((err.line, err.column), (2, 16));
        assert_eq!(err.message, "Type error: '+' cannot be applied to BOOL and integer literal");
    }

    #[test]
    fn rejects_real_into_int() {
        let err = error("PROGRAM p VAR Result : INT; END_VAR Result := 10.5; END_PROGRAM");
        assert_eq!(err.message, "Type mismatch: cannot assign REAL literal to 'Result' (INT)");
        let err = error("PROGRAM p VAR i : INT; r : REAL; END_VAR i := r; END_PROGRAM");
        assert!(err.message.contains("cannot assign REAL to 'i' (INT)"));
    }

    #[test]
    fn int_widens_but_nothing_narrows() {
        let vars = "VAR i : INT; d : DINT; r : REAL; b : BOOL; END_VAR";
        let ok = |body: &str| check_src(&format!("PROGRAM p {vars} {body} END_PROGRAM"));
        // INT → DINT and INT → REAL are implicit.
        for body in ["d := i + d;", "d := i;", "r := i * r;", "r := i + 0.5;", "b := i < d;", "b := i = r;"] {
            assert!(ok(body).is_ok(), "{body}: {:?}", ok(body).err());
        }
        // Narrowing, DINT → REAL, and BOOL are errors.
        for (body, fragment) in [
            ("i := d;", "cannot assign DINT to 'i' (INT)"),
            ("i := i + d;", "cannot assign DINT to 'i' (INT)"),
            ("i := r;", "cannot assign REAL to 'i' (INT)"),
            ("i := i + 0.5;", "cannot assign REAL to 'i' (INT)"),
            ("r := d;", "cannot assign DINT to 'r' (REAL)"),
            ("r := d + r;", "'+' cannot be applied to DINT and REAL"),
            ("b := i = TRUE;", "'=' cannot be applied to INT and BOOL"),
        ] {
            let err = ok(body).unwrap_err();
            assert!(err.message.contains(fragment), "{body}: {}", err.message);
        }
    }

    #[test]
    fn widening_is_explicit_in_the_lowered_tree() {
        let program = check_src("PROGRAM p VAR i : INT; d : DINT; END_VAR d := i + d; d := i * 2; END_PROGRAM").unwrap();
        // INT operand of a DINT operation: the operand is widened.
        match &program.body[0] {
            Stmt::Assign { value: Expr { kind: ExprKind::Binary { left, .. }, .. }, .. } => {
                assert!(matches!(&left.kind, ExprKind::Convert { to: DataType::DInt, .. }), "{left:?}");
            }
            other => panic!("{other:?}"),
        }
        // INT operation stored in a DINT: computed as INT, then the result is widened.
        match &program.body[1] {
            Stmt::Assign { value: Expr { kind: ExprKind::Convert { to: DataType::DInt, operand }, .. }, .. } => {
                match &operand.kind {
                    ExprKind::Binary { right, .. } => assert_eq!(right.kind, ExprKind::Const(Value::Int(2))),
                    other => panic!("{other:?}"),
                }
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn rejects_operators_on_wrong_types() {
        assert!(error("PROGRAM p VAR b : BOOL; END_VAR b := b < TRUE; END_PROGRAM").message.contains("'<'"));
        assert!(error("PROGRAM p VAR b : BOOL; i : INT; END_VAR b := i AND i; END_PROGRAM").message.contains("'AND'"));
        assert!(error("PROGRAM p VAR b : BOOL; i : INT; END_VAR b := NOT i; END_PROGRAM").message.contains("NOT requires BOOL"));
        assert!(error("PROGRAM p VAR b : BOOL; END_VAR b := -b; END_PROGRAM").message.contains("'-'"));
        assert!(error("PROGRAM p VAR i : INT; END_VAR IF i THEN END_IF; END_PROGRAM").message.contains("IF condition must be BOOL"));
        let err = error("PROGRAM p VAR b : BOOL; i : INT; END_VAR IF b THEN ELSIF i THEN END_IF; END_PROGRAM");
        assert!(err.message.contains("IF condition must be BOOL, found INT"), "ELSIF conditions are checked too");
    }

    #[test]
    fn range_checks_integer_literals() {
        assert!(check_src("PROGRAM p VAR i : INT := 32767; j : INT := -32768; END_VAR END_PROGRAM").is_ok());
        assert_eq!(
            error("PROGRAM p VAR i : INT := 32768; END_VAR END_PROGRAM").message,
            "Value 32768 is out of range for INT"
        );
        assert!(check_src("PROGRAM p VAR d : DINT := 40000; END_VAR END_PROGRAM").is_ok());
    }

    #[test]
    fn rejects_bad_declarations_and_names() {
        assert_eq!(
            error("PROGRAM p VAR a : BOOL; A : INT; END_VAR END_PROGRAM").message,
            "Duplicate declaration of variable 'A' (first declared at line 1, column 15)"
        );
        assert!(error("PROGRAM p VAR a : INT; b : INT := a; END_VAR END_PROGRAM").message.contains("must be a constant"));
        assert!(error("PROGRAM p VAR b : INT := TRUE; END_VAR END_PROGRAM").message.contains("Type mismatch"));
        let err = error("PROGRAM p\nMotor := TRUE;\nEND_PROGRAM");
        assert_eq!((err.line, err.column, err.message.as_str()), (2, 1, "Undeclared variable 'Motor'"));
        assert!(error("PROGRAM p VAR m : BOOL; END_VAR m := Ghost; END_PROGRAM").message.contains("'Ghost'"));
    }

    // --- TIME ---

    #[test]
    fn time_type_rules() {
        assert!(check_src(
            "PROGRAM p VAR a : TIME := T#1s; b : TIME; x : BOOL; END_VAR
             b := a + T#500ms - T#100ms; x := a >= T#1s AND a <> b;
             END_PROGRAM"
        )
        .is_ok());
        let vars = "VAR a : TIME; i : INT; END_VAR";
        for (body, fragment) in [
            ("a := a + 1;", "'+' cannot be applied to TIME and integer literal"),
            ("a := a * 2;", "'*' cannot be applied to TIME and integer literal"),
            ("a := a * a;", "'*' cannot be applied to TIME and TIME"),
            ("a := 5;", "cannot assign integer literal to 'a' (TIME)"),
            ("i := a;", "cannot assign TIME to 'i' (INT)"),
            ("a := -a;", "'-' cannot be applied to TIME"),
        ] {
            let err = error(&format!("PROGRAM p {vars} {body} END_PROGRAM"));
            assert!(err.message.contains(fragment), "{body}: {}", err.message);
        }
    }

    // --- function blocks ---

    const FB_VARS: &str = "VAR t : TON; c : CTU; run : BOOL; n : INT; d : TIME; END_VAR";

    fn fb_error(body: &str) -> String {
        error(&format!("PROGRAM p {FB_VARS} {body} END_PROGRAM")).message
    }

    #[test]
    fn accepts_calls_and_member_reads() {
        check_src(&format!(
            "PROGRAM p {FB_VARS}
             t(IN := run, PT := T#2s);
             t(in := NOT run);
             c(CU := t.Q, R := FALSE, PV := 3);
             IF t.Q AND c.CV >= 3 THEN n := c.CV + 1; d := t.ET; END_IF;
             END_PROGRAM"
        ))
        .unwrap();
    }

    #[test]
    fn call_arguments_are_type_checked() {
        let program = check_src(&format!("PROGRAM p {FB_VARS} c(PV := 3); END_PROGRAM")).unwrap();
        match &program.body[0] {
            Stmt::Call { args, .. } => assert_eq!(args[0].value.kind, ExprKind::Const(Value::Int(3)), "PV is INT"),
            other => panic!("{other:?}"),
        }
        assert!(fb_error("t(PT := 2000);").contains("cannot assign integer literal to 't.PT' (TIME)"));
        assert!(fb_error("t(IN := n);").contains("cannot assign INT to 't.IN' (BOOL)"));
    }

    #[test]
    fn rejects_misused_function_blocks() {
        assert_eq!(fb_error("t(Q := TRUE);"), "'Q' is an output of TON; read it as t.Q");
        assert_eq!(fb_error("t(FOO := TRUE);"), "TON has no input 'FOO' (inputs: IN, PT)");
        assert_eq!(fb_error("t(IN := TRUE, in := FALSE);"), "Input 'in' is given more than once");
        assert_eq!(fb_error("run(IN := TRUE);"), "'run' is not a function block instance (it is BOOL)");
        assert_eq!(fb_error("ghost(IN := TRUE);"), "Unknown function or function block instance 'ghost'");
        assert_eq!(fb_error("run := t.X;"), "TON has no member 'X' (members: IN, PT, Q, ET)");
        assert_eq!(fb_error("run := n.Q;"), "'n' is not a function block instance (it is INT)");
        assert_eq!(fb_error("run := t;"), "'t' is a TON instance; read one of its outputs, e.g. t.Q");
        assert_eq!(fb_error("t := t;"), "Can't assign to 't': it is a TON instance");
        let err = error("PROGRAM p VAR t : TON := T#1s; END_VAR END_PROGRAM");
        assert!(err.message.contains("can't have an initial value"));
        let err = error("PROGRAM p VAR t : TON; b : BOOL := t.Q; END_VAR END_PROGRAM");
        assert!(err.message.contains("must be a constant"));
    }

    #[test]
    fn timer_outputs_are_read_only() {
        for kind in ["TON", "TOF", "TP"] {
            let src = |body: &str| format!("PROGRAM p VAR Timer : {kind}; END_VAR {body} END_PROGRAM");
            assert_eq!(
                error(&src("Timer.Q := TRUE;")).message,
                format!("'Timer.Q' is an output of {kind} and is read-only")
            );
            assert!(error(&src("Timer.IN := TRUE;")).message.contains("Set"), "inputs go through a call");
            assert_eq!(
                error(&src("Timer(IN := TRUE, PT := 10);")).message,
                "Type mismatch: cannot assign integer literal to 'Timer.PT' (TIME)"
            );
            assert_eq!(
                error(&src("Timer(Q := TRUE);")).message,
                format!("'Q' is an output of {kind}; read it as Timer.Q")
            );
            assert!(check_src(&src("Timer(IN := TRUE, PT := T#1S);")).is_ok());
        }
    }

    // --- MOD / XOR ---

    #[test]
    fn mod_is_integer_only_and_xor_is_bool_only() {
        let vars = "VAR i : INT; d : DINT; r : REAL; b : BOOL; END_VAR";
        let src = |body: &str| format!("PROGRAM p {vars} {body} END_PROGRAM");
        assert!(check_src(&src("i := i MOD 3; d := d MOD i; b := b XOR TRUE;")).is_ok());
        assert!(error(&src("r := r MOD 2.0;")).message.contains("'MOD' cannot be applied to REAL"));
        assert!(error(&src("b := i XOR i;")).message.contains("'XOR' cannot be applied to INT and INT"));
    }

    // --- loops ---

    #[test]
    fn validates_for_loops() {
        let vars = "VAR i : INT; d : DINT; r : REAL; t : TON; END_VAR";
        let src = |body: &str| format!("PROGRAM p {vars} {body} END_PROGRAM");
        assert!(check_src(&src("FOR i := 1 TO 10 BY 2 DO d := d + i; END_FOR;")).is_ok());
        assert!(check_src(&src("FOR d := i TO 100000 DO END_FOR;")).is_ok(), "INT bounds widen to a DINT counter");
        for (body, fragment) in [
            ("FOR x := 1 TO 5 DO END_FOR;", "Undeclared FOR loop variable 'x'"),
            ("FOR r := 1 TO 5 DO END_FOR;", "must be INT or DINT, found REAL"),
            ("FOR t := 1 TO 5 DO END_FOR;", "must be INT or DINT, found TON"),
            ("FOR i := 1 TO d DO END_FOR;", "cannot assign DINT to 'i end' (INT)"),
            ("FOR i := 1 TO 40000 DO END_FOR;", "40000 is out of range for INT"),
            ("FOR i := 1 TO 5 BY 0 DO END_FOR;", "FOR step can't be 0"),
            ("FOR i := 1 TO 5 BY 0.5 DO END_FOR;", "cannot assign REAL literal to 'i step'"),
            ("FOR i := 1 TO 5 DO i := 3; END_FOR;", "Can't assign to 'i' inside the FOR loop it controls"),
            ("FOR i := 1 TO 5 DO FOR i := 1 TO 2 DO END_FOR; END_FOR;", "already controls an enclosing FOR loop"),
        ] {
            let err = error(&src(body));
            assert!(err.message.contains(fragment), "{body}: {}", err.message);
        }
        // After the loop, the variable is an ordinary variable again.
        assert!(check_src(&src("FOR i := 1 TO 5 DO END_FOR; i := 0;")).is_ok());
    }

    #[test]
    fn loop_conditions_must_be_bool() {
        let vars = "VAR i : INT; END_VAR";
        let err = error(&format!("PROGRAM p {vars} WHILE i DO END_WHILE; END_PROGRAM"));
        assert_eq!(err.message, "WHILE condition must be BOOL, found INT");
        let err = error(&format!("PROGRAM p {vars} REPEAT i := i + 1; UNTIL 5 END_REPEAT; END_PROGRAM"));
        assert_eq!(err.message, "UNTIL condition must be BOOL, found integer literal");
    }

    // --- CASE ---

    #[test]
    fn validates_case() {
        let vars = "VAR mode : INT; big : DINT; r : REAL; b : BOOL; END_VAR";
        let src = |body: &str| format!("PROGRAM p {vars} {body} END_PROGRAM");
        assert!(check_src(&src("CASE mode OF 0: b := FALSE; 1, 2: b := TRUE; ELSE b := FALSE; END_CASE;")).is_ok());
        assert!(check_src(&src("CASE big OF 100000: b := TRUE; END_CASE;")).is_ok());
        for (body, fragment) in [
            ("CASE r OF 1: b := TRUE; END_CASE;", "CASE selector must be INT or DINT, found REAL"),
            ("CASE b OF 1: b := TRUE; END_CASE;", "CASE selector must be INT or DINT, found BOOL"),
            ("CASE mode OF 1: b := TRUE; 2, 1: b := FALSE; END_CASE;", "Duplicate CASE label 1"),
            ("CASE mode OF 40000: b := TRUE; END_CASE;", "40000 is out of range for INT"),
            ("CASE mode OF 1: b := 5; END_CASE;", "cannot assign integer literal to 'b'"),
        ] {
            let err = error(&src(body));
            assert!(err.message.contains(fragment), "{body}: {}", err.message);
        }
    }

    // --- counters and edge triggers ---

    #[test]
    fn counter_and_trigger_interfaces() {
        let vars = "VAR c : CTU; d : CTD; r : R_TRIG; f : F_TRIG; b : BOOL; i : INT; n : DINT; x : REAL; END_VAR";
        let src = |body: &str| format!("PROGRAM p {vars} {body} END_PROGRAM");
        assert!(check_src(&src(
            "c(CU := b, R := FALSE, PV := 5); d(CD := b, LD := b, PV := i); r(CLK := b); f(CLK := NOT b);
             b := c.Q AND d.Q OR r.Q XOR f.Q; i := c.CV + d.CV;"
        ))
        .is_ok());
        for (body, message) in [
            ("c(CU := i);", "Type mismatch: cannot assign INT to 'c.CU' (BOOL)"),
            ("c(R := 1);", "Type mismatch: cannot assign integer literal to 'c.R' (BOOL)"),
            ("d(LD := x);", "Type mismatch: cannot assign REAL to 'd.LD' (BOOL)"),
            ("c(PV := n);", "Type mismatch: cannot assign DINT to 'c.PV' (INT)"),
            ("d(PV := 2.5);", "Type mismatch: cannot assign REAL literal to 'd.PV' (INT)"),
            ("r(CLK := i);", "Type mismatch: cannot assign INT to 'r.CLK' (BOOL)"),
            ("c.Q := TRUE;", "'c.Q' is an output of CTU and is read-only"),
            ("c.CV := 10;", "'c.CV' is an output of CTU and is read-only"),
            ("d.CV := 10;", "'d.CV' is an output of CTD and is read-only"),
            ("r.Q := TRUE;", "'r.Q' is an output of R_TRIG and is read-only"),
            ("f.Q := TRUE;", "'f.Q' is an output of F_TRIG and is read-only"),
            ("d(CU := b);", "CTD has no input 'CU' (inputs: CD, LD, PV)"),
            ("r(Q := b);", "'Q' is an output of R_TRIG; read it as r.Q"),
        ] {
            assert_eq!(error(&src(body)).message, message, "{body}");
        }
    }

    // --- standard functions ---

    const FN_VARS: &str = "VAR i : INT; n : DINT; x : REAL; b : BOOL; t : TIME; ri : REAL; END_VAR";

    fn fn_error(body: &str) -> String {
        error(&format!("PROGRAM p {FN_VARS} {body} END_PROGRAM")).message
    }

    #[test]
    fn accepts_well_typed_function_calls() {
        check_src(&format!(
            "PROGRAM p {FN_VARS}
             i := ABS(i); i := ABS(-10); x := ABS(-10.5); n := ABS(n);
             x := SQRT(x) + SQRT(25) + SQRT(i);    (* INT widens to REAL *)
             i := MIN(i, 3); x := MAX(x, i); n := MAX(i, n, 7); t := MIN(t, T#1s);
             n := TRUNC(10.75) + ROUND(x);
             x := SIN(x) * COS(1.5);
             b := ABS(i) > 3 AND MAX(x, 0.0) < 10.0;
             END_PROGRAM"
        ))
        .unwrap();
    }

    #[test]
    fn function_result_types() {
        assert_eq!(fn_error("i := TRUNC(x);"), "Type mismatch: cannot assign DINT to 'i' (INT)");
        assert_eq!(fn_error("x := ROUND(x);"), "Type mismatch: cannot assign DINT to 'x' (REAL)");
        assert_eq!(fn_error("i := SQRT(x);"), "Type mismatch: cannot assign REAL to 'i' (INT)");
        assert_eq!(fn_error("i := MAX(i, n);"), "Type mismatch: cannot assign DINT to 'i' (INT)");
    }

    #[test]
    fn rejects_bad_function_arguments() {
        for (body, message) in [
            ("i := ABS(TRUE);", "Type error: ABS expects a number (INT, DINT or REAL), found BOOL"),
            ("x := SQRT(TRUE);", "Type error: SQRT expects a REAL (or INT), found BOOL"),
            ("x := SQRT(n);", "Type error: SQRT expects a REAL (or INT), found DINT"),
            ("x := SIN(t);", "Type error: SIN expects a REAL (or INT), found TIME"),
            ("i := MIN(i, b);", "Type error: MIN arguments must have compatible types, found INT and BOOL"),
            ("x := MAX(n, x);", "Type error: MAX arguments must have compatible types, found DINT and REAL"),
            ("b := MAX(b, b);", "Type error: MAX expects numbers or TIME, found BOOL"),
        ] {
            assert_eq!(fn_error(body), message, "{body}");
        }
    }

    #[test]
    fn rejects_bad_argument_counts_and_unknown_functions() {
        assert_eq!(fn_error("i := ABS();"), "ABS expects 1 argument, found 0");
        assert_eq!(fn_error("i := ABS(i, i);"), "ABS expects 1 argument, found 2");
        assert_eq!(fn_error("i := MIN(i);"), "MIN expects at least 2 arguments, found 1");
        assert_eq!(fn_error("i := MY_UNKNOWN_FUNCTION(i);"), "Unknown function 'MY_UNKNOWN_FUNCTION'");
        assert_eq!(fn_error("MY_UNKNOWN_FUNCTION(i := 1);"), "Unknown function or function block instance 'MY_UNKNOWN_FUNCTION'");
        assert_eq!(fn_error("ABS(x := 1);"), "ABS is a function: use its result, e.g. x := ABS(…);");
        // Errors point at the offending argument.
        let err = error(&format!("PROGRAM p {FN_VARS}\nx := MAX(x,\n  TRUE);\nEND_PROGRAM"));
        assert_eq!((err.line, err.column), (3, 3));
    }

    #[test]
    fn widening_inside_calls_is_explicit() {
        let program = check_src(&format!("PROGRAM p {FN_VARS} x := SQRT(i); END_PROGRAM")).unwrap();
        match &program.body[0] {
            Stmt::Assign { value: Expr { kind: ExprKind::Call { args, .. }, .. }, .. } => {
                assert!(matches!(args[0].kind, ExprKind::Convert { to: DataType::Real, .. }), "{:?}", args[0]);
            }
            other => panic!("{other:?}"),
        }
    }

    // --- semantic analysis: multiple errors, kinds, locations ---

    use crate::st::error::ErrorKind;

    fn summary(errors: &[StError]) -> Vec<(ErrorKind, usize, usize)> {
        errors.iter().map(|e| (e.kind, e.line, e.column)).collect()
    }

    #[test]
    fn spec_valid_program_has_no_errors() {
        check_src(
            "PROGRAM Main
             VAR
                 Start : BOOL := FALSE; Motor : BOOL := FALSE; Counter : INT := 0;
                 Temperature : REAL := 25.5; Timer : TON;
             END_VAR
             Timer(
                 IN := Start,
                 PT := T#2S
             );
             IF Start THEN Motor := TRUE; ELSE Motor := FALSE; END_IF;
             Counter := Counter + 1;
             IF Temperature > 20.0 THEN Motor := TRUE; END_IF;
             END_PROGRAM",
        )
        .unwrap();
    }

    #[test]
    fn reports_every_error_in_one_pass_in_source_order() {
        let errors = errors(
            "PROGRAM p
VAR
    Counter : INT;
    Motor : BOOL;
END_VAR
Unknown := 10;
Counter := TRUE;
Motor := 100;
END_PROGRAM",
        );
        let messages: Vec<_> = errors.iter().map(|e| e.message.as_str()).collect();
        assert_eq!(
            messages,
            [
                "Undeclared variable 'Unknown'",
                "Type mismatch: cannot assign BOOL to 'Counter' (INT)",
                "Type mismatch: cannot assign integer literal to 'Motor' (BOOL)",
            ]
        );
        assert_eq!(
            summary(&errors),
            [(ErrorKind::UndeclaredVariable, 6, 1), (ErrorKind::TypeMismatch, 7, 12), (ErrorKind::TypeMismatch, 8, 10)]
        );
    }

    #[test]
    fn keeps_checking_inside_and_after_broken_statements() {
        let errors = errors(
            "PROGRAM p
VAR i : INT; b : BOOL; t : TON; END_VAR
IF i THEN
    b := 5;
ELSIF Ghost THEN
    i := TRUE;
END_IF;
FOR b := 1 TO 3 DO
    i := Missing;
END_FOR;
t(IN := 10, PT := TRUE);
WHILE i DO b := b + 1; END_WHILE;
END_PROGRAM",
        );
        assert_eq!(
            summary(&errors),
            [
                (ErrorKind::InvalidCondition, 3, 4),    // IF i
                (ErrorKind::TypeMismatch, 4, 10),       // b := 5, inside the broken IF
                (ErrorKind::UndeclaredVariable, 5, 7),  // ELSIF Ghost
                (ErrorKind::TypeMismatch, 6, 10),       // i := TRUE
                (ErrorKind::InvalidLoop, 8, 1),         // FOR over a BOOL
                (ErrorKind::UndeclaredVariable, 9, 10), // body of the broken FOR
                (ErrorKind::TypeMismatch, 11, 9),       // IN := 10
                (ErrorKind::TypeMismatch, 11, 19),      // PT := TRUE, same call
                (ErrorKind::InvalidCondition, 12, 7),   // WHILE i
                (ErrorKind::InvalidOperator, 12, 19),   // b + 1 in its body
            ]
        );
    }

    #[test]
    fn collects_declaration_errors_too() {
        let errors = errors(
            "PROGRAM p VAR
    a : BOOL;
    a : INT;
    t : TON := 5;
    x : INT := a;
    y : INT := TRUE;
END_VAR
END_PROGRAM",
        );
        assert_eq!(
            summary(&errors),
            [
                (ErrorKind::DuplicateDeclaration, 3, 5),
                (ErrorKind::InvalidAssignment, 4, 16),
                (ErrorKind::InvalidAssignment, 5, 16),
                (ErrorKind::TypeMismatch, 6, 16),
            ]
        );
        assert!(errors[0].message.ends_with("(first declared at line 2, column 5)"));
    }

    /// The spec's invalid-program tests, each as the only problem in its program.
    #[test]
    fn spec_invalid_programs() {
        let vars = "VAR Start : BOOL; Motor : BOOL; Counter : INT; Timer : TON; END_VAR";
        for (body, kind, message) in [
            ("Motor := UnknownVariable;", ErrorKind::UndeclaredVariable, "Undeclared variable 'UnknownVariable'"),
            ("Counter := TRUE;", ErrorKind::TypeMismatch, "Type mismatch: cannot assign BOOL to 'Counter' (INT)"),
            ("Motor := 100;", ErrorKind::TypeMismatch, "Type mismatch: cannot assign integer literal to 'Motor' (BOOL)"),
            ("Motor := 10.5;", ErrorKind::TypeMismatch, "Type mismatch: cannot assign REAL literal to 'Motor' (BOOL)"),
            ("Motor := T#2S;", ErrorKind::TypeMismatch, "Type mismatch: cannot assign TIME to 'Motor' (BOOL)"),
            ("IF Counter THEN Motor := TRUE; END_IF;", ErrorKind::InvalidCondition, "IF condition must be BOOL, found INT"),
            ("Timer(IN := Motor, PT := 2000);", ErrorKind::TypeMismatch, "Type mismatch: cannot assign integer literal to 'Timer.PT' (TIME)"),
            ("Timer.Q := TRUE;", ErrorKind::InvalidFunctionBlockOutput, "'Timer.Q' is an output of TON and is read-only"),
            ("Timer.IN := TRUE;", ErrorKind::InvalidFunctionBlockInput, "Set TON inputs in a call, e.g. Timer(IN := …);"),
            ("Counter := ABS(TRUE);", ErrorKind::InvalidFunctionArguments, "Type error: ABS expects a number (INT, DINT or REAL), found BOOL"),
            ("Counter := ABS();", ErrorKind::InvalidFunctionArguments, "ABS expects 1 argument, found 0"),
            ("Counter := ABS(10, 20);", ErrorKind::InvalidFunctionArguments, "ABS expects 1 argument, found 2"),
            ("Counter := UNKNOWN_FUNCTION(10);", ErrorKind::InvalidFunction, "Unknown function 'UNKNOWN_FUNCTION'"),
            ("Counter := TRUE + 10;", ErrorKind::InvalidOperator, "Type error: '+' cannot be applied to BOOL and integer literal"),
            ("Motor := TRUE > 10;", ErrorKind::InvalidOperator, "Type error: '>' cannot be applied to BOOL and integer literal"),
            ("Counter := Timer;", ErrorKind::InvalidFunctionBlock, "'Timer' is a TON instance; read one of its outputs, e.g. Timer.Q"),
            ("CASE Motor OF 1: Motor := TRUE; END_CASE;", ErrorKind::InvalidCase, "CASE selector must be INT or DINT, found BOOL"),
            ("FOR Motor := 1 TO 10 DO END_FOR;", ErrorKind::InvalidLoop, "FOR loop variable 'Motor' must be INT or DINT, found BOOL"),
        ] {
            let errors = errors(&format!("PROGRAM p {vars} {body} END_PROGRAM"));
            assert_eq!(errors.len(), 1, "{body}: {errors:?}");
            assert_eq!((errors[0].kind, errors[0].message.as_str()), (kind, message), "{body}");
        }
    }

    /// Spec tests 7 and 8 as written are statements, not expressions: `ABS(TRUE);` and
    /// `UNKNOWN_FUNCTION(10);`. ST only allows function blocks (with `name := value`
    /// inputs) as statements, so these are rejected before or during analysis.
    #[test]
    fn spec_function_calls_written_as_statements() {
        let vars = "VAR x : INT; END_VAR";
        let abs = errors(&format!("PROGRAM p {vars} ABS(x := 1); END_PROGRAM"));
        assert_eq!(abs[0].kind, ErrorKind::InvalidFunction);
        assert_eq!(abs[0].message, "ABS is a function: use its result, e.g. x := ABS(…);");
        let unknown = errors(&format!("PROGRAM p {vars} UNKNOWN_FUNCTION(IN := 10); END_PROGRAM"));
        assert_eq!(unknown[0].message, "Unknown function or function block instance 'UNKNOWN_FUNCTION'");
        // With positional arguments it's a syntax error: a statement call takes named inputs.
        let err = parse(&format!("PROGRAM p {vars} ABS(TRUE); END_PROGRAM")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Syntax);
        let err = parse(&format!("PROGRAM p {vars} UNKNOWN_FUNCTION(10); END_PROGRAM")).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Syntax);
    }

    #[test]
    fn spec_duplicate_declaration() {
        let errors = errors("PROGRAM p\nVAR\n    Motor : BOOL;\n    Motor : BOOL;\nEND_VAR\nEND_PROGRAM");
        assert_eq!(summary(&errors), [(ErrorKind::DuplicateDeclaration, 4, 5)]);
        assert_eq!(errors[0].message, "Duplicate declaration of variable 'Motor' (first declared at line 3, column 5)");
    }

    #[test]
    fn spec_fb_input_types_are_all_reported() {
        let errors = errors("PROGRAM p VAR Timer : TON; END_VAR\nTimer(\n    IN := 10,\n    PT := TRUE\n);\nEND_PROGRAM");
        assert_eq!(summary(&errors), [(ErrorKind::TypeMismatch, 3, 11), (ErrorKind::TypeMismatch, 4, 11)]);
    }

    #[test]
    fn invalid_time_literal_has_its_own_kind() {
        let err = parse("PROGRAM p VAR t : TON; END_VAR t(PT := T#2X); END_PROGRAM").unwrap_err();
        assert_eq!(err.kind, ErrorKind::InvalidTimeValue);
    }

    #[test]
    fn symbol_table_records_declarations_and_writes() {
        let checked = check(
            parse("PROGRAM p VAR Start : BOOL; Motor : BOOL; Timer : TON; END_VAR Motor := Start; END_PROGRAM").unwrap(),
        )
        .unwrap_or_else(|e| panic!("{e:?}"));
        let start = checked.symbols.get("start").unwrap();
        assert_eq!((start.name.as_str(), start.line, start.column, start.is_written()), ("Start", 1, 15, false));
        assert!(checked.symbols.get("MOTOR").unwrap().is_written());
        assert_eq!(checked.symbols.get("Timer").unwrap().block_type(), Some(FbKind::Ton));
    }
}
