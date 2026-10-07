//! Symbol table: every declared variable of a program, with its type, where it was
//! declared, and whether the project writes it. `Scope` is what a program can see:
//! its own variables, plus variables declared by exactly one other program.

use std::cell::Cell;
use std::collections::HashMap;

use super::ast::{DataType, VarType};
use super::function_blocks::FbKind;

/// IEC identifiers are case-insensitive: `Motor`, `MOTOR` and `motor` are one symbol.
pub fn key(name: &str) -> String {
    name.to_ascii_uppercase()
}

#[derive(Debug, Clone)]
pub struct Symbol {
    /// Name as written in the declaration.
    pub name: String,
    pub var_type: VarType,
    pub line: usize,
    pub column: usize,
    /// A PLC input (DI, AI): programs read it, only the I/O layer writes it.
    pub read_only: bool,
    // A Cell, so writes can be recorded while the table is shared by the checker.
    written: Cell<bool>,
}

impl Symbol {
    pub fn data_type(&self) -> Option<DataType> {
        match self.var_type {
            VarType::Elementary(t) => Some(t),
            VarType::FunctionBlock(_) | VarType::UserFb(_) => None,
        }
    }

    /// The program assigns this symbol somewhere (`:=` or as a FOR loop variable).
    pub fn is_written(&self) -> bool {
        self.written.get()
    }
}

#[derive(Debug, Clone, Default)]
pub struct SymbolTable {
    /// In declaration order.
    symbols: Vec<Symbol>,
    by_key: HashMap<String, usize>,
}

impl SymbolTable {
    /// Add a declaration. If the name is taken, returns the existing symbol instead.
    pub fn declare(&mut self, name: &str, var_type: VarType, line: usize, column: usize) -> Result<(), &Symbol> {
        if let Some(&i) = self.by_key.get(&key(name)) {
            return Err(&self.symbols[i]);
        }
        self.by_key.insert(key(name), self.symbols.len());
        self.symbols.push(Symbol {
            name: name.to_string(),
            var_type,
            line,
            column,
            read_only: false,
            written: Cell::new(false),
        });
        Ok(())
    }

    /// Add a PLC I/O address (no source position). Inputs are read-only in programs.
    pub fn declare_io(&mut self, name: &str, var_type: VarType, read_only: bool) {
        if self.declare(name, var_type, 0, 0).is_ok() {
            let last = self.symbols.len() - 1;
            self.symbols[last].read_only = read_only;
        }
    }

    pub fn get(&self, name: &str) -> Option<&Symbol> {
        self.by_key.get(&key(name)).map(|&i| &self.symbols[i])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Symbol> {
        self.symbols.iter()
    }
}

/// What callers see of a function block type, standard or user-defined: its inputs (with
/// whether a call must give them), outputs, and the names of its internal variables.
#[derive(Debug, Clone, PartialEq)]
pub struct FbInterface {
    pub name: String,
    /// Defined in the project (FUNCTION_BLOCK), as opposed to a standard block.
    pub user: bool,
    /// (name, type, required). A user input without an initial value is required.
    pub inputs: Vec<(String, DataType, bool)>,
    pub outputs: Vec<(String, DataType)>,
    pub internals: Vec<String>,
}

impl FbInterface {
    pub fn builtin(kind: FbKind) -> Self {
        Self {
            name: kind.name().to_string(),
            user: false,
            inputs: kind.inputs().iter().map(|(n, t)| (n.to_string(), *t, false)).collect(),
            outputs: kind.outputs().iter().map(|(n, t)| (n.to_string(), *t)).collect(),
            internals: Vec::new(),
        }
    }

    pub fn input_type(&self, name: &str) -> Option<DataType> {
        self.inputs.iter().find(|(n, _, _)| n.eq_ignore_ascii_case(name)).map(|(_, t, _)| *t)
    }

    pub fn is_output(&self, name: &str) -> bool {
        self.outputs.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    pub fn is_internal(&self, name: &str) -> bool {
        self.internals.iter().any(|n| n.eq_ignore_ascii_case(name))
    }

    /// Inputs and outputs: what a program may read as `instance.member`.
    pub fn member_type(&self, name: &str) -> Option<DataType> {
        self.input_type(name)
            .or_else(|| self.outputs.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, t)| *t))
    }

    pub fn input_names(&self) -> String {
        self.inputs.iter().map(|(n, _, _)| n.as_str()).collect::<Vec<_>>().join(", ")
    }

    pub fn member_names(&self) -> String {
        self.inputs.iter().map(|(n, _, _)| n.as_str()).chain(self.outputs.iter().map(|(n, _)| n.as_str())).collect::<Vec<_>>().join(", ")
    }
}

/// Name lookup for one program of a project. Its own declarations come first; a name it
/// doesn't declare resolves to another program's variable if exactly one program declares
/// it, which is how programs share state (e.g. `Start` declared in Main, read in Motor).
#[derive(Clone, Copy)]
pub struct Scope<'a> {
    /// Every program's table, in project order.
    tables: &'a [SymbolTable],
    names: &'a [String],
    current: usize,
    /// The project's FUNCTION_BLOCK types, indexed by `VarType::UserFb`.
    fbs: &'a [FbInterface],
}

impl<'a> Scope<'a> {
    pub fn new(tables: &'a [SymbolTable], names: &'a [String], current: usize) -> Self {
        Self { tables, names, current, fbs: &[] }
    }

    /// Sees nothing; for constant expressions such as initial values.
    pub fn empty() -> Self {
        Self { tables: &[], names: &[], current: 0, fbs: &[] }
    }

    /// Also know the project's FUNCTION_BLOCK types.
    pub fn with_fbs(self, fbs: &'a [FbInterface]) -> Self {
        Self { fbs, ..self }
    }

    /// The interface of a function block type (None for plain values).
    pub fn interface(&self, var_type: VarType) -> Option<FbInterface> {
        match var_type {
            VarType::FunctionBlock(kind) => Some(FbInterface::builtin(kind)),
            VarType::UserFb(index) => self.fbs.get(index).cloned(),
            VarType::Elementary(_) => None,
        }
    }

    /// A type's name for messages: BOOL, TON, MotorControl, ….
    pub fn type_name(&self, var_type: VarType) -> String {
        match var_type {
            VarType::Elementary(t) => t.name().to_string(),
            other => self.interface(other).map_or_else(|| "function block".to_string(), |i| i.name),
        }
    }

    /// Programs other than the current one that declare `name`.
    fn declared_elsewhere(&self, name: &str) -> impl Iterator<Item = usize> + '_ {
        let name = key(name);
        (0..self.tables.len()).filter(move |&i| i != self.current && self.tables[i].by_key.contains_key(&name))
    }

    /// Index of the program whose variable `name` refers to, if it resolves.
    pub fn owner(&self, name: &str) -> Option<usize> {
        if self.tables.get(self.current).is_some_and(|t| t.get(name).is_some()) {
            return Some(self.current);
        }
        let mut owners = self.declared_elsewhere(name);
        match (owners.next(), owners.next()) {
            (Some(i), None) => Some(i),
            _ => None,
        }
    }

    pub fn get(&self, name: &str) -> Option<&'a Symbol> {
        self.owner(name).and_then(|i| self.tables[i].get(name))
    }

    /// Record a write; it counts for the declaring program's symbol (so the monitor knows
    /// a shared variable is driven by the project, not an input).
    pub fn mark_written(&self, name: &str) {
        if let Some(symbol) = self.get(name) {
            symbol.written.set(true);
        }
    }

    /// Like `get`, but also accepts `Program.Variable` (the way to name a variable that
    /// several programs declare); returns the declaring program's index too.
    pub fn lookup(&self, name: &str) -> Option<(usize, &'a Symbol)> {
        if let Some((program, variable)) = name.split_once('.') {
            let index = self.names.iter().position(|n| n.eq_ignore_ascii_case(program.trim()))?;
            return self.tables[index].get(variable.trim()).map(|symbol| (index, symbol));
        }
        let index = self.owner(name)?;
        self.tables[index].get(name).map(|symbol| (index, symbol))
    }

    /// The other programs that declare `name`.
    pub fn declared_in(&self, name: &str) -> Vec<&'a str> {
        self.declared_elsewhere(name).map(|i| self.names[i].as_str()).collect()
    }

    /// Why `name` doesn't resolve; `what` is e.g. "variable" or "FOR loop variable".
    pub fn undeclared(&self, name: &str, what: &str) -> String {
        let owners: Vec<&str> = self.declared_elsewhere(name).map(|i| self.names[i].as_str()).collect();
        if owners.len() > 1 {
            format!(
                "Ambiguous {what} '{name}': declared in programs {}; declare it in this program or give it a unique name",
                owners.join(", ")
            )
        } else {
            format!("Undeclared {what} '{name}'")
        }
    }
}

