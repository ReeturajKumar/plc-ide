//! Static checking between parsing and execution.
//!
//! Validates names and types once, before any scan runs, and lowers the AST so every
//! numeric literal becomes a concrete, range-checked `Value` and every implicit widening
//! becomes an explicit `Convert` node. After this pass the runtime only ever combines
//! values of the same type.
//!
//! Conversion rules (IEC 61131-3 Ed. 3 "safe" implicit conversions, `DataType::widens_to`):
//! - A smaller integer widens to a larger one (SINT → INT → DINT → LINT, USINT → UINT …,
//!   unsigned to a larger signed), and to REAL/LREAL where that is exact; REAL → LREAL;
//!   BYTE → WORD → DWORD → LWORD.
//! - Nothing narrows (DINT → INT, REAL → INT) and e.g. DINT → REAL is not implicit
//!   because it can lose precision; these are type errors. `*_TO_*` functions convert
//!   explicitly.
//! - An operation's type comes from its operands: INT + INT is an INT addition (and can
//!   overflow as INT) even when the result is stored in a DINT; INT + DINT widens the INT
//!   operand and adds as DINT.
//! - Numeric literals take the type their context needs, range-checked
//!   (`Counter + 1` is INT + INT). An integer literal may become any integer, bit string
//!   or real type; a REAL literal only REAL or LREAL.
//! - BOOL and TIME never convert implicitly.
//!
//! Error collection: each statement is checked independently and every problem found is
//! returned, in source order, so one run reports them all. Within a single expression only
//! the first problem is reported, since later ones are usually consequences of it.

use std::collections::HashSet;

use super::ast::{
    BinaryOp, CallArg, CaseBranch, DataType, Expr, ExprKind, FunctionBlockDef, IfBranch, Layer, Program, Stmt, TypeBody,
    TypeDecl, TypeDef, UnaryOp, VarDecl, VarType,
};
use super::parser::MAX_ELEMENTS;
use super::error::{ErrorKind, StError, StErrors, StResult};
use super::function_blocks::FbKind;
use super::functions::StdFunction;
pub use super::symbols::key;
use super::symbols::{var_type_def, FbInterface, Scope, SymbolTable};
use super::value::{date_arith, Value};

fn err(kind: ErrorKind, line: usize, column: usize, message: impl Into<String>) -> StError {
    StError::new(kind, line, column, message)
}

/// Static type of an expression. Numeric literals are untyped until context decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Ty {
    Known(DataType),
    AnyInt,
    AnyReal,
    /// A whole array, structure or pointer (`VarType::Array`/`Struct`/`Pointer`).
    Composite(VarType),
    /// `ADR(x)`: a pointer to a value of this type.
    Address(VarType),
}

impl Ty {
    fn of(var_type: VarType) -> Option<Ty> {
        match var_type {
            VarType::Elementary(t) => Some(Ty::Known(t)),
            VarType::Array(_) | VarType::Struct(_) | VarType::Pointer(_) | VarType::Reference(_) => Some(Ty::Composite(var_type)),
            VarType::FunctionBlock(_) | VarType::UserFb(_) => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Ty::Known(t) => t.name(),
            Ty::AnyInt => "integer literal",
            Ty::AnyReal => "REAL literal",
            Ty::Composite(VarType::Array(_)) => "an array",
            Ty::Composite(VarType::Pointer(_)) => "a pointer",
            Ty::Composite(VarType::Reference(_)) => "a reference",
            Ty::Composite(_) => "a structure",
            Ty::Address(_) => "an address (ADR)",
        }
    }

    fn is_numeric(self) -> bool {
        match self {
            Ty::Known(t) => t.is_numeric(),
            Ty::AnyInt | Ty::AnyReal => true,
            Ty::Composite(_) | Ty::Address(_) => false,
        }
    }

    fn is_integer(self) -> bool {
        match self {
            Ty::Known(t) => t.is_integer(),
            Ty::AnyInt => true,
            Ty::AnyReal | Ty::Composite(_) | Ty::Address(_) => false,
        }
    }

    /// The concrete type used when nothing else constrains a literal (e.g. `1 < 2`).
    /// Never asked of a composite: the checker rejects those as operands first.
    fn concrete(self) -> DataType {
        match self {
            Ty::Known(t) => t,
            Ty::AnyInt => DataType::DInt,
            Ty::AnyReal => DataType::Real,
            Ty::Composite(_) | Ty::Address(_) => DataType::Pointer,
        }
    }
}

/// The common type two operands can share, or None if they're incompatible.
fn unify(a: Ty, b: Ty) -> Option<Ty> {
    match (a, b) {
        _ if a == b => Some(a),
        (Ty::Known(x), Ty::Known(y)) if x.widens_to(y) => Some(b),
        (Ty::Known(x), Ty::Known(y)) if y.widens_to(x) => Some(a),
        (Ty::AnyInt, Ty::Known(t)) | (Ty::Known(t), Ty::AnyInt) if t.is_numeric() || t.is_bit_string() => {
            Some(Ty::Known(t))
        }
        // The smallest real type both fit: INT and 1.5 make REAL, DINT and 1.5 LREAL.
        (Ty::AnyReal, Ty::Known(t)) | (Ty::Known(t), Ty::AnyReal) => {
            [DataType::Real, DataType::LReal].into_iter().find(|&r| t.widens_to(r)).map(Ty::Known)
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
    /// Each TYPE declaration's errors, in order.
    pub type_errors: Vec<StErrors>,
    /// The array and structure types, indexed by `VarType::Array`/`Struct`.
    pub types: Vec<TypeDef>,
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
    type_decls: &[TypeDecl],
    io: &SymbolTable,
    mapped_inputs: &HashSet<String>,
) -> ProjectCheck {
    // Only programs' instances can be mapped, never those inside a block.
    let no_mapped_inputs = HashSet::new();
    let registry: Vec<String> = fbs.iter().map(|fb| fb.name.clone()).collect();

    // --- TYPEs: every one resolved (and its errors found) even if nothing uses it ---
    let mut types = Types::new(type_decls, &registry);
    for (i, decl) in type_decls.iter().enumerate() {
        let at = |message: String| err(ErrorKind::DuplicateDeclaration, decl.line, decl.column, message);
        let clash = if DataType::from_name(&decl.name).is_some() || FbKind::from_name(&decl.name).is_some() {
            Some(format!("'{}' is a standard type; give yours another name", decl.name))
        } else if registry.iter().any(|n| n.eq_ignore_ascii_case(&decl.name)) {
            Some(format!("'{}' is already a function block", decl.name))
        } else if type_decls[..i].iter().any(|d| d.name.eq_ignore_ascii_case(&decl.name)) {
            Some(format!("Type '{}' is defined more than once", decl.name))
        } else {
            None
        };
        match clash {
            Some(message) => types.errors[i].push(at(message)),
            // Its problems are recorded in its own errors; the result is for users of it.
            None => {
                let _ = types.named(i, decl.line, decl.column, false);
            }
        }
    }

    // --- function blocks: members, recursion, bodies ---
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
        declare_into(&mut fb.inputs, &mut table, errors, &mut types, io, true);
        declare_into(&mut fb.outputs, &mut table, errors, &mut types, io, true);
        declare_into(&mut fb.vars, &mut table, errors, &mut types, io, false);
        fb_tables.push(table);
    }
    let interfaces: Vec<FbInterface> = fbs.iter().map(interface).collect();
    // Programs' declarations too, before any body is checked: they may add array types.
    let (names, mut programs): (Vec<String>, Vec<Program>) = programs.into_iter().unzip();
    let (tables, mut errors): (Vec<SymbolTable>, Vec<StErrors>) =
        programs.iter_mut().map(|program| declarations(&mut program.vars, &mut types, io)).unzip();
    let Types { defs: type_defs, errors: type_errors, .. } = types;
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
        let scope = Scope::new(&fb_tables[i..=i], &registry[i..=i], 0).with_fbs(&interfaces).with_types(&type_defs);
        let mut checker =
            Checker { symbols: &scope, mapped: &no_mapped_inputs, loop_vars: Vec::new(), errors: std::mem::take(&mut fb_errors[i]) };
        fb.body = checker.block(&fb.body);
        fb_errors[i] = checker.errors;
    }

    // --- programs ---
    let (tables, names) = with_io(tables, names, io);

    for (i, program) in programs.iter_mut().enumerate() {
        let scope = Scope::new(&tables, &names, i).with_fbs(&interfaces).with_types(&type_defs);
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
    ProjectCheck { programs, function_blocks, type_errors, types: type_defs, interfaces, tables, names }
}

/// The project's TYPE declarations being resolved into the type table (`defs`), which
/// array declarations anywhere also add to. Errors in a TYPE go to its own `errors`.
struct Types<'d> {
    decls: &'d [TypeDecl],
    fbs: &'d [String],
    /// What each declaration resolved to; None if not yet, or it has errors (`failed`).
    resolved: Vec<Option<VarType>>,
    failed: Vec<bool>,
    /// The declarations being resolved right now, innermost last: to catch a type that
    /// contains itself, and to know whose errors a member's problem is.
    resolving: Vec<usize>,
    /// A structure's type while its members are resolved: a member may point to it
    /// (`next : POINTER TO Node`), just not contain it.
    reserved: Vec<Option<VarType>>,
    errors: Vec<StErrors>,
    defs: Vec<TypeDef>,
}

impl<'d> Types<'d> {
    fn new(decls: &'d [TypeDecl], fbs: &'d [String]) -> Self {
        let n = decls.len();
        Self {
            decls,
            fbs,
            resolved: vec![None; n],
            failed: vec![false; n],
            resolving: Vec::new(),
            reserved: vec![None; n],
            errors: vec![StErrors::new(); n],
            defs: Vec::new(),
        }
    }

    /// The type `decl` declares: its elementary, function block or named type, inside its
    /// array dimensions. On failure the error is in `errors`.
    fn resolve(&mut self, decl: &VarDecl, errors: &mut StErrors) -> Option<VarType> {
        let at = |kind: ErrorKind, message: String| err(kind, decl.line, decl.column, message);
        let mut t = decl.var_type;
        if t == VarType::UNRESOLVED {
            t = if let Some(i) = self.fbs.iter().position(|n| n.eq_ignore_ascii_case(&decl.type_name)) {
                VarType::UserFb(i)
            } else if let Some(i) = self.decls.iter().position(|d| d.name.eq_ignore_ascii_case(&decl.type_name)) {
                let behind_pointer = decl.layers.contains(&Layer::Pointer);
                match self.named(i, decl.line, decl.column, behind_pointer) {
                    Ok(t) => t,
                    Err(e) => {
                        errors.push(e);
                        return None;
                    }
                }
            } else {
                let message = format!("Unknown data type or function block type '{}' for '{}'", decl.type_name, decl.name);
                errors.push(at(ErrorKind::InvalidFunctionBlock, message));
                return None;
            };
        }
        if !decl.layers.is_empty() && matches!(t, VarType::FunctionBlock(_) | VarType::UserFb(_)) {
            let message = format!(
                "Arrays of, pointers to and references to function blocks ({}) are not supported yet; declare the instances one by one",
                decl.type_name
            );
            errors.push(at(ErrorKind::InvalidFunctionBlock, message));
            return None;
        }
        for (k, layer) in decl.layers.iter().enumerate().rev() {
            if matches!(t, VarType::Reference(_)) {
                let message = "REFERENCE TO can only be a variable's own type; inside ARRAY OF or POINTER TO, use POINTER TO";
                errors.push(at(ErrorKind::TypeMismatch, message.to_string()));
                return None;
            }
            let def = match *layer {
                Layer::Array(lower, upper) => {
                    if upper < lower {
                        errors.push(at(ErrorKind::TypeMismatch, format!("Array bounds {lower}..{upper} are empty: the upper bound must not be below the lower one")));
                        return None;
                    }
                    if (upper - lower + 1).saturating_mul(self.size(t)) > MAX_ELEMENTS {
                        errors.push(at(ErrorKind::TypeMismatch, format!("'{}' is too large: more than {MAX_ELEMENTS} elements", decl.name)));
                        return None;
                    }
                    TypeDef::Array { lower, upper, of: t }
                }
                Layer::Pointer => TypeDef::Pointer { to: t },
                Layer::Reference if k == 0 => TypeDef::Reference { to: t },
                Layer::Reference => {
                    let message = "REFERENCE TO can only be a variable's own type; inside ARRAY OF or POINTER TO, use POINTER TO";
                    errors.push(at(ErrorKind::TypeMismatch, message.to_string()));
                    return None;
                }
            };
            let index = self.defs.iter().position(|d| *d == def).unwrap_or_else(|| {
                self.defs.push(def);
                self.defs.len() - 1
            });
            t = match layer {
                Layer::Array(..) => VarType::Array(index),
                Layer::Pointer => VarType::Pointer(index),
                Layer::Reference => VarType::Reference(index),
            };
        }
        Some(t)
    }

    /// What TYPE declaration `i` names, resolving it the first time; used at (line,
    /// column), through a pointer if `behind_pointer`.
    fn named(&mut self, i: usize, line: usize, column: usize, behind_pointer: bool) -> StResult<VarType> {
        let decl = &self.decls[i];
        let at = |message: String| err(ErrorKind::TypeMismatch, line, column, message);
        if let Some(t) = self.resolved[i] {
            return Ok(t);
        }
        if self.failed[i] {
            return Err(at(format!("Type '{}' has errors", decl.name)));
        }
        if self.resolving.contains(&i) {
            return match self.reserved[i] {
                Some(t) if behind_pointer => Ok(t),
                _ => Err(at(format!("Type '{}' contains itself; a type can't be part of itself (it can point to itself)", decl.name))),
            };
        }
        self.resolving.push(i);
        let mut errors = std::mem::take(&mut self.errors[i]);
        let resolved = match &decl.body {
            TypeBody::Alias(alias) => self.resolve(alias, &mut errors),
            TypeBody::Struct(fields) => self.structure(i, fields, &mut errors),
        };
        self.errors[i].extend(errors);
        self.resolving.pop();
        self.resolved[i] = resolved;
        self.failed[i] = resolved.is_none();
        resolved.ok_or_else(|| at(format!("Type '{}' has errors", decl.name)))
    }

    fn structure(&mut self, i: usize, fields: &[VarDecl], errors: &mut StErrors) -> Option<VarType> {
        let name = &self.decls[i].name;
        // Its index first, so members can point to it.
        let index = self.defs.len();
        self.defs.push(TypeDef::Struct { name: name.clone(), fields: Vec::new() });
        self.reserved[i] = Some(VarType::Struct(index));
        let mut checked: Vec<VarDecl> = Vec::new();
        let before = errors.len();
        for field in fields {
            let at = |kind: ErrorKind, message: String| err(kind, field.line, field.column, message);
            if checked.iter().any(|f| f.name.eq_ignore_ascii_case(&field.name)) {
                errors.push(at(ErrorKind::DuplicateDeclaration, format!("Structure {name} has two members named '{}'", field.name)));
                continue;
            }
            let Some(t) = self.resolve(field, errors) else { continue };
            if matches!(t, VarType::FunctionBlock(_) | VarType::UserFb(_)) {
                let message = format!("Structure members can't be function blocks ('{}' of {name}); declare the instance in the program", field.name);
                errors.push(at(ErrorKind::InvalidFunctionBlock, message));
                continue;
            }
            if matches!(t, VarType::Reference(_)) {
                errors.push(at(ErrorKind::TypeMismatch, format!("Structure members can't be references ('{}' of {name}); use POINTER TO", field.name)));
                continue;
            }
            let init = match field.init.as_ref().map(|init| initializer(init, t, &field.name, &self.defs)).transpose() {
                Ok(init) => init,
                Err(e) => {
                    errors.push(e);
                    continue;
                }
            };
            checked.push(VarDecl { var_type: t, init, ..field.clone() });
        }
        if errors.len() > before {
            return None;
        }
        self.defs[index] = TypeDef::Struct { name: name.clone(), fields: checked };
        Some(VarType::Struct(index))
    }

    /// How many elementary values a type holds (a function block counts as one).
    fn size(&self, t: VarType) -> i128 {
        match var_type_def(t, &self.defs) {
            Some(TypeDef::Array { lower, upper, of }) => (upper - lower + 1).saturating_mul(self.size(*of)),
            Some(TypeDef::Struct { fields, .. }) => fields.iter().map(|f| self.size(f.var_type)).sum(),
            Some(TypeDef::Pointer { .. } | TypeDef::Reference { .. }) | None => 1,
        }
    }
}

/// Check and lower `init`, the initial value of `target` of type `t`: a constant, `[…]`
/// for an array (fewer values than elements leave the rest at their default), or
/// `(member := …, …)` for a structure.
fn initializer(init: &Expr, t: VarType, target: &str, types: &[TypeDef]) -> StResult<Expr> {
    let at = |message: String| err(ErrorKind::InvalidAssignment, init.line, init.column, message);
    let kind = match (var_type_def(t, types), &init.kind) {
        (Some(TypeDef::Array { lower, upper, of }), ExprKind::ArrayInit(items)) => {
            let length = upper - lower + 1;
            if items.len() as i128 > length {
                return Err(at(format!("Too many initial values for '{target}': {} for {length} elements", items.len())));
            }
            let element = format!("{target}[…]");
            ExprKind::ArrayInit(items.iter().map(|item| initializer(item, *of, &element, types)).collect::<StResult<_>>()?)
        }
        (Some(TypeDef::Struct { name, fields: members }), ExprKind::StructInit(given)) => {
            let mut lowered: Vec<(String, Expr)> = Vec::new();
            for (field, value) in given {
                let member = members
                    .iter()
                    .find(|m| m.name.eq_ignore_ascii_case(field))
                    .ok_or_else(|| at(format!("Structure {name} has no member '{field}'")))?;
                if lowered.iter().any(|(n, _)| *n == member.name) {
                    return Err(at(format!("Member '{field}' is given twice")));
                }
                lowered.push((member.name.clone(), initializer(value, member.var_type, &format!("{target}.{}", member.name), types)?));
            }
            ExprKind::StructInit(lowered)
        }
        (Some(TypeDef::Array { .. }), _) => return Err(at(format!("Initial value of array '{target}' must be a list, e.g. [1, 2, 3]"))),
        (Some(TypeDef::Pointer { .. } | TypeDef::Reference { .. }), _) => {
            return Err(at(format!("'{target}' can't have an initial value; set it in the program, e.g. {target} := ADR(x);")))
        }
        (Some(TypeDef::Struct { .. }), _) => {
            return Err(at(format!("Initial value of structure '{target}' must list its members, e.g. (x := 1.0)")))
        }
        (None, _) => {
            let VarType::Elementary(data_type) = t else {
                return Err(at(format!("'{target}' can't have an initial value")));
            };
            if let ExprKind::ArrayInit(_) | ExprKind::StructInit(_) = init.kind {
                return Err(at(format!("'{target}' is {}, not an array or structure: give it one value", data_type.name())));
            }
            if let Some(var) = first_variable(init) {
                return Err(err(ErrorKind::InvalidAssignment, var.line, var.column, format!("Initial value of '{target}' must be a constant")));
            }
            return assignable(init, data_type, target, &Scope::empty());
        }
    };
    Ok(Expr { kind, line: init.line, column: init.column })
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
fn declarations(vars: &mut [VarDecl], types: &mut Types, io: &SymbolTable) -> (SymbolTable, StErrors) {
    let mut errors = StErrors::new();
    let mut symbols = SymbolTable::default();
    declare_into(vars, &mut symbols, &mut errors, types, io, false);
    (symbols, errors)
}

/// Declare `vars` in `symbols`: resolve their types (FUNCTION_BLOCKs, TYPEs, arrays),
/// refuse duplicates, and check initial values. A function block's inputs and outputs
/// (`values_only`) must be plain values.
fn declare_into(
    vars: &mut [VarDecl],
    symbols: &mut SymbolTable,
    errors: &mut StErrors,
    types: &mut Types,
    io: &SymbolTable,
    values_only: bool,
) {
    for decl in vars.iter_mut() {
        match types.resolve(decl, errors) {
            Some(t) => decl.var_type = t,
            None => continue,
        }
        if values_only && !matches!(decl.var_type, VarType::Elementary(_)) {
            errors.push(err(
                ErrorKind::InvalidFunctionBlock,
                decl.line,
                decl.column,
                match decl.var_type {
                    VarType::Array(_) | VarType::Struct(_) => format!(
                        "Function block inputs and outputs can't be arrays or structures yet; declare '{}' under VAR",
                        decl.name
                    ),
                    _ => format!(
                        "Function block inputs and outputs must be elementary values (BOOL, INT, REAL, WORD, TIME, …); declare the {} instance '{}' under VAR",
                        decl.type_name, decl.name
                    ),
                },
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
        if let VarType::FunctionBlock(_) | VarType::UserFb(_) = decl.var_type {
            let name = match decl.var_type {
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
        match initializer(init, decl.var_type, &decl.name, &types.defs) {
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
            // Through a reference: write what it refers to.
            Stmt::Assign { target, value, line, column }
                if matches!(symbols.get(target).map(|s| s.var_type), Some(VarType::Reference(_))) =>
            {
                let target = Expr { kind: ExprKind::Variable(target.clone()), line: *line, column: *column };
                self.stmt(&Stmt::AssignPath { target, value: value.clone(), line: *line, column: *column })
            }
            Stmt::RefAssign { target, value, line, column } => {
                let at = |kind: ErrorKind, message: String| err(kind, *line, *column, message);
                let to = match symbols.get(target).map(|s| (s.var_type, var_type_def(s.var_type, symbols.types()))) {
                    Some((_, Some(TypeDef::Reference { to }))) => *to,
                    Some((t, _)) => {
                        let message = format!("'{target}' is {}, not a REFERENCE TO; REF= binds references", symbols.type_name(t));
                        self.report(at(ErrorKind::InvalidAssignment, message));
                        return None;
                    }
                    None => {
                        self.report(at(ErrorKind::UndeclaredVariable, symbols.undeclared(target, "variable")));
                        return None;
                    }
                };
                let place = self.ok(address_of(value, symbols))?;
                if place != to {
                    let message = format!(
                        "Type mismatch: '{target}' is a REFERENCE TO {}, it can't refer to {}",
                        symbols.type_name(to),
                        symbols.type_name(place)
                    );
                    self.report(err(ErrorKind::TypeMismatch, value.line, value.column, message));
                    return None;
                }
                symbols.mark_written(target);
                let value = Expr { kind: ExprKind::Address(Box::new(self.ok(lower_place(value, symbols))?)), line: value.line, column: value.column };
                Some(Stmt::Assign { target: target.clone(), value, line: *line, column: *column })
            }
            Stmt::Assign { target, value, line, column } => {
                let target_type = match symbols.get(target).map(|s| (s.var_type, s.read_only)) {
                    Some((_, true)) => {
                        self.report(err(
                            ErrorKind::InvalidAssignment,
                            *line,
                            *column,
                            format!("Can't assign to '{target}': PLC inputs and SCADA tags are read-only in programs"),
                        ));
                        None
                    }
                    Some((t @ (VarType::Elementary(_) | VarType::Array(_) | VarType::Struct(_) | VarType::Pointer(_)), false)) => Some(t),
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
                let value = self.ok(assignable_to(value, target_type, target, symbols))?;
                Some(Stmt::Assign { target: target.clone(), value, line: *line, column: *column })
            }
            // A structure's member: the same as `s.x` in any other path.
            Stmt::AssignMember { instance, member, value, line, column } if is_struct(instance, symbols) =>
            {
                let target = Expr {
                    kind: ExprKind::Member { instance: instance.clone(), member: member.clone() },
                    line: *line,
                    column: *column,
                };
                self.stmt(&Stmt::AssignPath { target, value: value.clone(), line: *line, column: *column })
            }
            Stmt::AssignPath { target, value, line, column } => {
                let root = root_variable(target);
                let at = |kind: ErrorKind, message: String| err(kind, *line, *column, message);
                match symbols.get(root).map(|s| s.read_only) {
                    Some(true) => self.report(at(ErrorKind::InvalidAssignment, format!("Can't assign to '{root}': PLC inputs and SCADA tags are read-only in programs"))),
                    Some(false) => symbols.mark_written(root),
                    None => {
                        self.report(at(ErrorKind::UndeclaredVariable, symbols.undeclared(root, "variable")));
                        self.ok(infer(value, symbols));
                        return None;
                    }
                }
                let Some(target_type) = self.ok(infer(target, symbols)) else {
                    self.ok(infer(value, symbols));
                    return None;
                };
                let lowered = match target_type {
                    Ty::Known(t) => self.ok(assignable(value, t, &describe(target), symbols)),
                    Ty::Composite(t) => self.ok(assignable_to(value, t, &describe(target), symbols)),
                    other => {
                        self.report(at(ErrorKind::InvalidAssignment, format!("Can't assign to {}", other.name())));
                        None
                    }
                };
                let target = self.ok(lower_place(target, symbols))?;
                Some(Stmt::AssignPath { target, value: lowered?, line: *line, column: *column })
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
                        self.report(at(format!("FOR loop variable '{var}' must be an integer (SINT … ULINT), found {found}")));
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
                    Some(ty) if ty.is_integer() || ty.concrete().is_bit_string() => Some(ty.concrete()),
                    Some(ty) => {
                        self.report(err(
                            ErrorKind::InvalidCase,
                            selector.line,
                            selector.column,
                            format!("CASE selector must be an integer or bit string, found {}", ty.name()),
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

/// Like `assignable`, for any variable type: a whole array or structure takes a value of
/// exactly its type.
fn assignable_to(value: &Expr, target_type: VarType, target: &str, symbols: &Scope) -> StResult<Expr> {
    if let Some(TypeDef::Pointer { to }) = var_type_def(target_type, symbols.types()) {
        let ty = infer(value, symbols)?;
        if is_null(value) || pointee(ty, symbols) == Some(*to) {
            return lower_pointer(value, symbols);
        }
        let found = match (ty, pointee(ty, symbols)) {
            (_, Some(t)) => format!("POINTER TO {}", symbols.type_name(t)),
            (Ty::Composite(t), None) => symbols.type_name(t),
            (other, None) => other.name().to_string(),
        };
        return Err(err(
            ErrorKind::TypeMismatch,
            value.line,
            value.column,
            format!("Type mismatch: cannot assign {found} to '{target}' ({})", symbols.type_name(target_type)),
        ));
    }
    let VarType::Elementary(t) = target_type else {
        let ty = infer(value, symbols)?;
        if ty != Ty::Composite(target_type) {
            let found = match ty {
                Ty::Composite(t) => symbols.type_name(t),
                other => other.name().to_string(),
            };
            return Err(err(
                ErrorKind::TypeMismatch,
                value.line,
                value.column,
                format!("Type mismatch: cannot assign {found} to '{target}' ({})", symbols.type_name(target_type)),
            ));
        }
        return lower_place(value, symbols);
    };
    assignable(value, t, target, symbols)
}

/// The variable at the root of `a[i].x`, `s.inner.y`, `p^`, ….
fn root_variable(expr: &Expr) -> &str {
    match &expr.kind {
        ExprKind::Variable(name) | ExprKind::Member { instance: name, .. } => name,
        ExprKind::Index { base, .. } | ExprKind::Field { base, .. } | ExprKind::Deref(base) => root_variable(base),
        _ => "",
    }
}

/// `name` is a structure (or a reference to one), so `name.x` is a member, not a block's.
fn is_struct(name: &str, symbols: &Scope) -> bool {
    let variable = Expr { kind: ExprKind::Variable(name.to_string()), line: 0, column: 0 };
    matches!(infer(&variable, symbols), Ok(Ty::Composite(VarType::Struct(_))))
}

/// The literal 0, the null pointer.
fn is_null(expr: &Expr) -> bool {
    matches!(expr.kind, ExprKind::IntLiteral(0))
}

/// What a pointer-valued expression points to (`ADR(x)`, a pointer variable or element).
fn pointee(ty: Ty, symbols: &Scope) -> Option<VarType> {
    match ty {
        Ty::Address(t) => Some(t),
        Ty::Composite(t) => match var_type_def(t, symbols.types()) {
            Some(TypeDef::Pointer { to }) => Some(*to),
            _ => None,
        },
        _ => None,
    }
}

/// Lower a pointer value: `0` (null), `ADR(x)`, or a pointer variable or element.
fn lower_pointer(expr: &Expr, symbols: &Scope) -> StResult<Expr> {
    match &expr.kind {
        ExprKind::IntLiteral(0) => Ok(Expr { kind: ExprKind::Const(Value::Pointer(None)), line: expr.line, column: expr.column }),
        ExprKind::Call { name, args } if name.eq_ignore_ascii_case("ADR") => {
            let place = lower_place(&args[0], symbols)?;
            Ok(Expr { kind: ExprKind::Address(Box::new(place)), line: expr.line, column: expr.column })
        }
        _ => lower_place(expr, symbols),
    }
}

/// The type of the variable or element `place` (for `ADR(place)`, `r REF= place`), which
/// must be one the program may write: not a PLC input, not a block's member.
fn address_of(place: &Expr, symbols: &Scope) -> StResult<VarType> {
    let at = |message: String| err(ErrorKind::InvalidAssignment, place.line, place.column, message);
    let is_place = match &place.kind {
        ExprKind::Variable(_) | ExprKind::Index { .. } | ExprKind::Field { .. } | ExprKind::Deref(_) => true,
        ExprKind::Member { instance, .. } => is_struct(instance, symbols),
        _ => false,
    };
    if !is_place {
        return Err(at("Expected a variable or an element of one, e.g. ADR(x) or ADR(a[2])".to_string()));
    }
    let root = root_variable(place);
    if symbols.get(root).is_some_and(|s| s.read_only) {
        return Err(at(format!("'{root}' is a PLC input: a pointer or reference could write it")));
    }
    match infer(place, symbols)? {
        Ty::Known(t) => Ok(VarType::Elementary(t)),
        Ty::Composite(t) => Ok(t),
        other => Err(at(format!("Can't point to {}", other.name()))),
    }
}

/// `a[…].x` as text for messages.
fn describe(expr: &Expr) -> String {
    match &expr.kind {
        ExprKind::Variable(name) => name.clone(),
        ExprKind::Member { instance, member } => format!("{instance}.{member}"),
        ExprKind::Index { base, .. } => format!("{}[…]", describe(base)),
        ExprKind::Field { base, field, .. } => format!("{}.{field}", describe(base)),
        _ => "value".to_string(),
    }
}

/// The type of a structure's member `field`, and its position.
fn field_of(base: Ty, field: &str, symbols: &Scope, expr: &Expr) -> StResult<(usize, VarType)> {
    let Ty::Composite(t @ VarType::Struct(_)) = base else {
        return Err(err(ErrorKind::TypeMismatch, expr.line, expr.column, format!("'.{field}' needs a structure, found {}", base.name())));
    };
    let Some(TypeDef::Struct { name, fields }) = var_type_def(t, symbols.types()) else {
        return Err(err(ErrorKind::TypeMismatch, expr.line, expr.column, "Unknown structure".to_string()));
    };
    fields.iter().position(|f| f.name.eq_ignore_ascii_case(field)).map(|i| (i, fields[i].var_type)).ok_or_else(|| {
        let members = fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ");
        err(ErrorKind::TypeMismatch, expr.line, expr.column, format!("Structure {name} has no member '{field}' (members: {members})"))
    })
}

/// The bounds and element type of the array `base` is.
fn element_of(base: Ty, symbols: &Scope, expr: &Expr) -> StResult<(i128, i128, VarType)> {
    match base {
        Ty::Composite(t @ VarType::Array(_)) => match var_type_def(t, symbols.types()) {
            Some(TypeDef::Array { lower, upper, of }) => Ok((*lower, *upper, *of)),
            _ => Err(err(ErrorKind::TypeMismatch, expr.line, expr.column, "Unknown array type".to_string())),
        },
        other => Err(err(ErrorKind::TypeMismatch, expr.line, expr.column, format!("'[…]' needs an array, found {}", other.name()))),
    }
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
        ExprKind::ArrayInit(_) | ExprKind::StructInit(_) => Err(error(
            ErrorKind::InvalidAssignment,
            "'[…]' and '(member := …)' are only allowed as initial values in a declaration".to_string(),
        )),
        ExprKind::Index { base, index, .. } => {
            let (_, _, of) = element_of(infer(base, symbols)?, symbols, base)?;
            let index_type = infer(index, symbols)?;
            if !index_type.is_integer() {
                return Err(err(
                    ErrorKind::TypeMismatch,
                    index.line,
                    index.column,
                    format!("Array index must be an integer, found {}", index_type.name()),
                ));
            }
            Ty::of(of).ok_or_else(|| error(ErrorKind::InvalidFunctionBlock, "Arrays of function blocks are not supported".to_string()))
        }
        ExprKind::Field { base, field, .. } => {
            let (_, t) = field_of(infer(base, symbols)?, field, symbols, expr)?;
            Ok(Ty::of(t).unwrap_or(Ty::Known(DataType::Struct)))
        }
        ExprKind::Member { instance, member } if is_struct(instance, symbols) => {
            let base = infer(&Expr { kind: ExprKind::Variable(instance.clone()), ..expr.clone() }, symbols)?;
            let (_, t) = field_of(base, member, symbols, expr)?;
            Ok(Ty::of(t).unwrap_or(Ty::Known(DataType::Struct)))
        }
        ExprKind::Deref(base) => {
            if let ExprKind::Variable(name) = &base.kind {
                if matches!(symbols.get(name).map(|s| s.var_type), Some(VarType::Reference(_))) {
                    return Err(error(ErrorKind::InvalidOperator, format!("'{name}' is a reference: use it directly, without '^'")));
                }
            }
            let ty = infer(base, symbols)?;
            match var_type_def(match ty {
                Ty::Composite(t) => t,
                _ => VarType::UNRESOLVED,
            }, symbols.types())
            {
                Some(TypeDef::Pointer { to }) => Ok(Ty::of(*to).unwrap_or(Ty::Known(DataType::Pointer))),
                _ => Err(error(ErrorKind::InvalidOperator, format!("'^' needs a pointer, found {}", ty.name()))),
            }
        }
        ExprKind::Address(_) => Err(error(ErrorKind::InvalidOperator, "Internal error: an address before checking".to_string())),
        ExprKind::Call { name, args } if name.eq_ignore_ascii_case("ADR") => match args.as_slice() {
            [place] => Ok(Ty::Address(address_of(place, symbols)?)),
            _ => Err(error(ErrorKind::InvalidFunctionArguments, format!("ADR expects 1 argument, found {}", args.len()))),
        },
        // ISVALIDREF(r): whether a reference (or pointer) is bound.
        ExprKind::Call { name, args } if is_valid_ref(name) => match args.as_slice() {
            [Expr { kind: ExprKind::Variable(v), .. }]
                if matches!(symbols.get(v).map(|s| s.var_type), Some(VarType::Reference(_) | VarType::Pointer(_))) =>
            {
                Ok(Ty::Known(DataType::Bool))
            }
            _ => Err(error(ErrorKind::InvalidFunctionArguments, format!("{name} expects a REFERENCE TO or POINTER TO variable"))),
        },
        ExprKind::Variable(name) => {
            match symbols.get(name).map(|s| s.var_type) {
                // A reference reads what it refers to.
                Some(t @ VarType::Reference(_)) => match var_type_def(t, symbols.types()) {
                    Some(TypeDef::Reference { to }) => Ok(Ty::of(*to).unwrap_or(Ty::Known(DataType::Pointer))),
                    _ => Err(error(ErrorKind::TypeMismatch, "Unknown reference type".to_string())),
                },
                Some(t @ (VarType::Elementary(_) | VarType::Array(_) | VarType::Struct(_) | VarType::Pointer(_))) => {
                    Ok(Ty::of(t).unwrap_or(Ty::AnyInt))
                }
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
        ExprKind::Call { name, args } if conversion(name).is_some() => {
            let (from, to) = conversion(name).unwrap_or((None, DataType::Bool));
            let [arg] = args.as_slice() else {
                return Err(error(ErrorKind::InvalidFunctionArguments, format!("{name} expects 1 argument, found {}", args.len())));
            };
            let ty = infer(arg, symbols)?;
            if let Ty::Composite(_) | Ty::Address(_) = ty {
                return Err(err(ErrorKind::InvalidFunctionArguments, arg.line, arg.column, format!("Type error: {name} can't convert {}", ty.name())));
            }
            match from {
                Some(from) if unify(Ty::Known(from), ty) != Some(Ty::Known(from)) => Err(err(
                    ErrorKind::InvalidFunctionArguments,
                    arg.line,
                    arg.column,
                    format!("Type error: {name} expects {}, found {}", from.name(), ty.name()),
                )),
                _ => Ok(Ty::Known(to)),
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
            Ty::Known(t) if t == DataType::Bool || t.is_bit_string() => Ok(Ty::Known(t)),
            other => Err(error(
                ErrorKind::InvalidOperator,
                format!("Type error: NOT requires BOOL or a bit string, found {}", other.name()),
            )),
        },
        ExprKind::Unary { op: UnaryOp::Neg, operand } => match infer(operand, symbols)? {
            Ty::Composite(_) | Ty::Address(_) => {
                Err(error(ErrorKind::InvalidOperator, "Type error: '-' can't be applied to an array, structure or pointer".to_string()))
            }
            Ty::Known(t) if t.is_unsigned() => {
                Err(error(ErrorKind::InvalidOperator, format!("Type error: '-' cannot be applied to unsigned {}", t.name())))
            }
            ty if ty.is_numeric() => Ok(ty),
            other => Err(error(ErrorKind::InvalidOperator, format!("Type error: '-' cannot be applied to {}", other.name()))),
        },
        ExprKind::Binary { op, left, right } => {
            let (l, r) = (infer(left, symbols)?, infer(right, symbols)?);
            // Pointers compare with each other (same target type) and with 0 (null).
            let (pl, pr) = (pointee(l, symbols), pointee(r, symbols));
            if matches!(op, BinaryOp::Equal | BinaryOp::NotEqual) && (pl.is_some() || pr.is_some()) {
                let comparable = match (pl, pr) {
                    (Some(a), Some(b)) => a == b,
                    (Some(_), None) => is_null(right),
                    (None, _) => is_null(left),
                };
                return if comparable {
                    Ok(Ty::Known(DataType::Bool))
                } else {
                    Err(error(ErrorKind::InvalidOperator, format!("Type error: can't compare {} with {}", l.name(), r.name())))
                };
            }
            if matches!(l, Ty::Composite(_) | Ty::Address(_)) || matches!(r, Ty::Composite(_) | Ty::Address(_)) {
                return Err(error(
                    ErrorKind::InvalidOperator,
                    format!("Type error: '{}' can't be applied to {} and {}", op.symbol(), l.name(), r.name()),
                ));
            }
            if let (Ty::Known(a), Ty::Known(b)) = (l, r) {
                if let Some(result) = date_arith(*op, a, b) {
                    return Ok(Ty::Known(result));
                }
            }
            let type_error = || {
                error(
                    ErrorKind::InvalidOperator,
                    format!("Type error: '{}' cannot be applied to {} and {}", op.symbol(), l.name(), r.name()),
                )
            };
            let common = unify(l, r).ok_or_else(type_error)?;
            let boolean = common == Ty::Known(DataType::Bool);
            let time = common == Ty::Known(DataType::Time);
            let dates = matches!(common, Ty::Known(t) if t.is_time_like() || t.is_text() || matches!(t, DataType::Char | DataType::WChar));
            let bits = matches!(common, Ty::Known(t) if t.is_bit_string());
            match op {
                BinaryOp::And | BinaryOp::Or | BinaryOp::Xor if boolean || bits => Ok(common),
                BinaryOp::Equal | BinaryOp::NotEqual => Ok(Ty::Known(DataType::Bool)),
                BinaryOp::Less | BinaryOp::Greater | BinaryOp::LessEqual | BinaryOp::GreaterEqual
                    if common.is_numeric() || dates || bits =>
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

/// The `from`/`to` types of a conversion function: `INT_TO_REAL` (from INT) or `TO_REAL`
/// (from whatever the argument is).
fn conversion(name: &str) -> Option<(Option<DataType>, DataType)> {
    let upper = name.to_ascii_uppercase();
    if let Some(to) = upper.strip_prefix("TO_") {
        return Some((None, DataType::from_name(to)?));
    }
    let (from, to) = upper.split_once("_TO_")?;
    Some((Some(DataType::from_name(from)?), DataType::from_name(to)?))
}

/// The real type an argument of SQRT, SIN, … computes in: REAL, or LREAL for an LREAL
/// (or a type only LREAL holds exactly, like DINT).
fn real_argument(ty: Ty) -> Option<DataType> {
    [DataType::Real, DataType::LReal].into_iter().find(|&r| unify(Ty::Known(r), ty) == Some(Ty::Known(r)))
}

/// Result type of a standard function call, checking its argument types.
/// - ABS: any number, result of the same type.
/// - SQRT, SIN, COS: REAL or LREAL (integers widen), result the same. TRUNC, ROUND: REAL
///   or LREAL, result DINT.
/// - MIN, MAX: two or more numbers, bit strings or TIMEs of a common type, result that type.
/// - SHL, SHR, ROL, ROR: a bit string and an integer count, result the bit string's type.
/// - LEN(s) → INT; CONCAT(s, s, …) → the strings' type; LEFT(s, n), RIGHT(s, n),
///   MID(s, n, p) → s's type; FIND(s1, s2) → INT. STRING or WSTRING, never mixed.
fn function_type(function: StdFunction, args: &[Expr], types: &[Ty], call: &Expr) -> StResult<Ty> {
    let name = function.name();
    let invalid = |line: usize, column: usize, message: String| err(ErrorKind::InvalidFunctionArguments, line, column, message);
    let wrong = |i: usize, expected: &str| {
        invalid(args[i].line, args[i].column, format!("Type error: {name} expects {expected}, found {}", types[i].name()))
    };
    match function {
        StdFunction::Move if matches!(types[0], Ty::Composite(_) | Ty::Address(_)) => {
            Err(wrong(0, "an elementary value (copy arrays and structures with :=)"))
        }
        StdFunction::Move => Ok(types[0]),
        StdFunction::Abs if types[0].is_numeric() => Ok(types[0]),
        StdFunction::Abs => Err(wrong(0, "a number")),
        StdFunction::Sqrt | StdFunction::Sin | StdFunction::Cos | StdFunction::Trunc | StdFunction::Round => {
            let real = real_argument(types[0]).ok_or_else(|| wrong(0, "a REAL or LREAL"))?;
            let integer_result = matches!(function, StdFunction::Trunc | StdFunction::Round);
            Ok(Ty::Known(if integer_result { DataType::DInt } else { real }))
        }
        StdFunction::Shl | StdFunction::Shr | StdFunction::Rol | StdFunction::Ror => match (types[0], types[1]) {
            (Ty::Known(t), count) if t.is_bit_string() && count.is_integer() => Ok(types[0]),
            (Ty::Known(t), _) if t.is_bit_string() => Err(wrong(1, "an integer count")),
            _ => Err(wrong(0, "a bit string (BYTE, WORD, DWORD, LWORD)")),
        },
        StdFunction::Len | StdFunction::Concat | StdFunction::Left | StdFunction::Right | StdFunction::Mid | StdFunction::Find => {
            let text = |i: usize| matches!(types[i], Ty::Known(t) if t.is_text());
            if !text(0) {
                return Err(wrong(0, "a STRING or WSTRING"));
            }
            let (strings, counts) = match function {
                StdFunction::Concat => (args.len(), 0),
                StdFunction::Find => (2, 0),
                _ => (1, args.len() - 1),
            };
            for i in 1..strings {
                if types[i] != types[0] {
                    return Err(wrong(i, types[0].name()));
                }
            }
            if let Some(i) = (strings..strings + counts).find(|&i| !types[i].is_integer()) {
                return Err(wrong(i, "an integer"));
            }
            Ok(if matches!(function, StdFunction::Len | StdFunction::Find) { Ty::Known(DataType::Int) } else { types[0] })
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
            let ordered = common.is_numeric() || matches!(common, Ty::Known(t) if t.is_time_like() || t.is_bit_string() || t.is_text());
            if ordered {
                Ok(common)
            } else {
                Err(invalid(call.line, call.column, format!("Type error: {name} expects numbers, bit strings, TIME or dates, found {}", common.name())))
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
        ExprKind::RealLiteral(x) => ExprKind::Const(Value::real(*x, ty)),
        ExprKind::Variable(_) | ExprKind::Member { .. } | ExprKind::Index { .. } | ExprKind::Field { .. } | ExprKind::Deref(_) => {
            lower_place(expr, symbols)?.kind
        }
        ExprKind::Const(_)
        | ExprKind::Convert { .. }
        | ExprKind::ArrayInit(_)
        | ExprKind::StructInit(_)
        | ExprKind::Address(_) => expr.kind.clone(),
        ExprKind::Call { name, .. } if name.eq_ignore_ascii_case("ADR") => lower_pointer(expr, symbols)?.kind,
        // ISVALIDREF(r) is r <> null.
        ExprKind::Call { name, args } if is_valid_ref(name) => ExprKind::Binary {
            op: BinaryOp::NotEqual,
            left: Box::new(args[0].clone()),
            right: Box::new(Expr { kind: ExprKind::Const(Value::Pointer(None)), ..args[0].clone() }),
        },
        ExprKind::Call { name, args } if conversion(name).is_some() => {
            let (from, to) = conversion(name).unwrap_or((None, DataType::Bool));
            let arg = &args[0];
            let from = match from {
                Some(from) => from,
                None => infer(arg, symbols)?.concrete(),
            };
            ExprKind::Convert { to, operand: Box::new(lower(arg, from, symbols)?) }
        }
        ExprKind::Call { name, args } => {
            let natural = |a: &Expr| infer(a, symbols).map(Ty::concrete);
            let arg_types = match StdFunction::from_name(name) {
                // ABS, MIN, MAX and MOVE compute in their result type.
                Some(StdFunction::Abs | StdFunction::Min | StdFunction::Max | StdFunction::Move) => vec![ty; args.len()],
                Some(StdFunction::Shl | StdFunction::Shr | StdFunction::Rol | StdFunction::Ror) => vec![ty, natural(&args[1])?],
                Some(StdFunction::Concat) => vec![ty; args.len()],
                // The strings keep their type; lengths and positions theirs.
                Some(StdFunction::Len | StdFunction::Find | StdFunction::Left | StdFunction::Right | StdFunction::Mid) => {
                    args.iter().map(natural).collect::<StResult<_>>()?
                }
                _ => vec![real_argument(infer(&args[0], symbols)?).unwrap_or(DataType::Real)],
            };
            ExprKind::Call {
                name: name.clone(),
                args: args.iter().zip(arg_types).map(|(a, t)| lower(a, t, symbols)).collect::<StResult<_>>()?,
            }
        }
        ExprKind::Unary { op: UnaryOp::Not, operand } => {
            ExprKind::Unary { op: UnaryOp::Not, operand: Box::new(lower(operand, ty, symbols)?) }
        }
        ExprKind::Unary { op: UnaryOp::Neg, operand } => {
            ExprKind::Unary { op: UnaryOp::Neg, operand: Box::new(lower(operand, ty, symbols)?) }
        }
        ExprKind::Binary { op, left, right } => {
            // TOD + TIME, DT - DT, …: each operand keeps its own type.
            let (l, r) = (infer(left, symbols)?, infer(right, symbols)?);
            if pointee(l, symbols).is_some() || pointee(r, symbols).is_some() {
                let (left, right) = (lower_pointer(left, symbols)?, lower_pointer(right, symbols)?);
                let kind = ExprKind::Binary { op: *op, left: Box::new(left), right: Box::new(right) };
                return Ok(Expr { kind, line: expr.line, column: expr.column });
            }
            if let (Ty::Known(a), Ty::Known(b)) = (l, r) {
                if date_arith(*op, a, b).is_some() {
                    let (left, right) = (lower(left, a, symbols)?, lower(right, b, symbols)?);
                    let kind = ExprKind::Binary { op: *op, left: Box::new(left), right: Box::new(right) };
                    return Ok(Expr { kind, line: expr.line, column: expr.column });
                }
            }
            // Arithmetic runs in the expression's own type (operands widened to it);
            // comparison and logic operands use their common type, whatever the BOOL result.
            let operand_type = match op {
                BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Mod => ty,
                _ => unify(l, r).map(Ty::concrete).unwrap_or(DataType::Bool),
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

/// Lower a variable, `a[i]`, `s.x` or a chain of them: array bounds and member positions
/// filled in, indexes typed; a structure's `s.x` becomes a `Field`.
fn lower_place(expr: &Expr, symbols: &Scope) -> StResult<Expr> {
    let kind = match &expr.kind {
        ExprKind::Member { instance, member } if is_struct(instance, symbols) => {
            let base = Expr { kind: ExprKind::Variable(instance.clone()), line: expr.line, column: expr.column };
            let (index, _) = field_of(infer(&base, symbols)?, member, symbols, expr)?;
            ExprKind::Field { base: Box::new(lower_place(&base, symbols)?), field: member.clone(), index }
        }
        // A reference stands for what it refers to.
        ExprKind::Variable(name) if matches!(symbols.get(name).map(|s| s.var_type), Some(VarType::Reference(_))) => {
            ExprKind::Deref(Box::new(expr.clone()))
        }
        ExprKind::Deref(base) => {
            infer(expr, symbols)?;
            ExprKind::Deref(Box::new(lower_place(base, symbols)?))
        }
        ExprKind::Field { base, field, .. } => {
            let (index, _) = field_of(infer(base, symbols)?, field, symbols, expr)?;
            ExprKind::Field { base: Box::new(lower_place(base, symbols)?), field: field.clone(), index }
        }
        ExprKind::Index { base, index, .. } => {
            let (low, high, _) = element_of(infer(base, symbols)?, symbols, base)?;
            let index_type = infer(index, symbols)?.concrete();
            ExprKind::Index {
                base: Box::new(lower_place(base, symbols)?),
                index: Box::new(lower(index, index_type, symbols)?),
                lower: low,
                upper: high,
            }
        }
        _ => expr.kind.clone(),
    };
    Ok(Expr { kind, line: expr.line, column: expr.column })
}

fn is_valid_ref(name: &str) -> bool {
    name.eq_ignore_ascii_case("ISVALIDREF") || name.eq_ignore_ascii_case("__ISVALIDREF")
}

fn int_constant(n: i128, want: DataType) -> Result<Value, String> {
    match want {
        t if t.is_real() => Ok(Value::real(n as f64, t)),
        t if t.is_integer() || t.is_bit_string() => {
            Value::integer(n, t).map_err(|_| format!("Value {n} is out of range for {}", t.name()))
        }
        other => Err(format!("Type mismatch: integer literal where {} is required", other.name())),
    }
}

fn first_variable(expr: &Expr) -> Option<&Expr> {
    match &expr.kind {
        ExprKind::Variable(_) | ExprKind::Member { .. } => Some(expr),
        ExprKind::Call { args, .. } | ExprKind::ArrayInit(args) => args.iter().find_map(first_variable),
        ExprKind::StructInit(fields) => fields.iter().find_map(|(_, e)| first_variable(e)),
        ExprKind::Index { .. } | ExprKind::Field { .. } | ExprKind::Deref(_) | ExprKind::Address(_) => Some(expr),
        ExprKind::Unary { operand, .. } | ExprKind::Convert { operand, .. } => first_variable(operand),
        ExprKind::Binary { left, right, .. } => first_variable(left).or_else(|| first_variable(right)),
        _ => None,
    }
}

