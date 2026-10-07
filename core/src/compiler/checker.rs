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

/// A FUNCTION_BLOCK that passed semantic analysis.
pub struct CheckedFb {
    /// Initial values typed, body lowered.
    pub def: FunctionBlockDef,
}

/// The analysis of a whole project: one result per program and per FUNCTION_BLOCK, the
/// function block types (indexed by `VarType::UserFb`), and every symbol table (the
/// programs' then the I/O addresses, with `names`), even when there are errors, so the
/// I/O mappings can be checked against them.
pub struct ProjectCheck {
    pub programs: Vec<Result<Checked, StErrors>>,
    pub function_blocks: Vec<Result<CheckedFb, StErrors>>,
    pub interfaces: Vec<FbInterface>,
    pub tables: Vec<SymbolTable>,
    pub names: Vec<String>,
}

/// Name of the I/O address table in error messages.
pub const IO_SCOPE_NAME: &str = "I/O";

/// Every symbol table of a project: the programs' in order, then the I/O addresses
/// (`io`), which every program can use (`IF DI0 THEN`, `AO0 := x;`).
pub fn with_io(mut tables: Vec<SymbolTable>, mut names: Vec<String>, io: &SymbolTable) -> (Vec<SymbolTable>, Vec<String>) {
    tables.push(io.clone());
    names.push(IO_SCOPE_NAME.to_string());
    (tables, names)
}

/// Semantic analysis of a project. The FUNCTION_BLOCKs are registered first, as types the
/// programs (and other blocks) can instantiate; each block is checked on its own, seeing
/// only its members. Then every program, in order, gets one result with all of its errors;
/// a program can use the variables another program declares, as long as exactly one
/// program declares that name, and the PLC I/O addresses (`io`; reserved, they can't be
/// declared). `mapped_inputs` are block inputs fed by I/O mappings ("INSTANCE.MEMBER"):
/// calls needn't give them, and mustn't also set them.
pub fn check_project(
    programs: Vec<(String, Program)>,
    mut fbs: Vec<FunctionBlockDef>,
    io: &SymbolTable,
    mapped_inputs: &HashSet<String>,
) -> ProjectCheck {
    // Only programs' instances can be mapped, never those inside a block.
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
        declare_into(&mut fb.inputs, &mut table, errors, &registry, io, true);
        declare_into(&mut fb.outputs, &mut table, errors, &registry, io, true);
        declare_into(&mut fb.vars, &mut table, errors, &registry, io, false);
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
        programs.iter_mut().map(|program| declarations(&mut program.vars, &registry, io)).unzip();
    let (tables, names) = with_io(tables, names, io);

    for (i, program) in programs.iter_mut().enumerate() {
        let scope = Scope::new(&tables, &names, i).with_fbs(&interfaces);
        let mut checker = Checker { symbols: &scope, mapped: mapped_inputs, loop_vars: Vec::new(), errors: std::mem::take(&mut errors[i]) };
        program.body = checker.block(&program.body);
        errors[i] = checker.errors;
    }

    let programs = programs
        .into_iter()
        .zip(&tables)
        .zip(errors)
        .map(|((program, symbols), errors)| {
            if errors.is_empty() { Ok(Checked { program, symbols: symbols.clone() }) } else { Err(errors) }
        })
        .collect();
    let function_blocks = fbs
        .into_iter()
        .zip(fb_errors)
        .map(|(def, errors)| if errors.is_empty() { Ok(CheckedFb { def }) } else { Err(errors) })
        .collect();
    ProjectCheck { programs, function_blocks, interfaces, tables, names }
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
fn declarations(vars: &mut [VarDecl], registry: &[String], io: &SymbolTable) -> (SymbolTable, StErrors) {
    let mut errors = StErrors::new();
    let mut symbols = SymbolTable::default();
    declare_into(vars, &mut symbols, &mut errors, registry, io, false);
    (symbols, errors)
}

/// Declare `vars` in `symbols`: resolve FUNCTION_BLOCK type names against `registry`,
/// refuse duplicates, and check initial values. A function block's inputs and outputs
/// (`values_only`) must be plain values.
fn declare_into(
    vars: &mut [VarDecl],
    symbols: &mut SymbolTable,
    errors: &mut StErrors,
    registry: &[String],
    io: &SymbolTable,
    values_only: bool,
) {
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
        if io.get(&decl.name).is_some() {
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

