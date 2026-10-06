//! Engine state: what the runtime holds right now, as plain data. It describes the PLC,
//! not a screen: how (and whether) to show it is up to the host and the UI.

use super::debugger::Location;
use super::executor::slot;
use super::Runtime;
use crate::compiler::ast::{DataType, VarType};
use crate::compiler::symbols::key;
use crate::compiler::value::Value;

/// A program variable.
#[derive(Debug, Clone, PartialEq)]
pub struct VariableState {
    /// The program that declares it.
    pub program: String,
    pub name: String,
    pub data_type: DataType,
    pub value: Value,
    /// Some program assigns it (`:=` or as a FOR variable); otherwise its value can only
    /// come from outside the programs.
    pub written_by_program: bool,
    /// The I/O address it is mapped to, if any.
    pub io_address: Option<String>,
}

/// One member of a function block instance.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberState {
    pub name: String,
    pub data_type: DataType,
    pub value: Value,
}

/// A function block instance declared by a program.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionBlockState {
    pub program: String,
    pub instance: String,
    pub type_name: String,
    /// A standard block (TON, CTU, …), as opposed to a project FUNCTION_BLOCK.
    pub standard: bool,
    pub inputs: Vec<MemberState>,
    pub outputs: Vec<MemberState>,
    /// A project block's internal variables (standard blocks have none to show).
    pub internals: Vec<MemberState>,
}

/// Everything the engine holds that the host may report (the default: nothing loaded).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EngineState {
    /// Programs in execution order.
    pub programs: Vec<String>,
    pub cycle_count: u64,
    pub variables: Vec<VariableState>,
    pub function_blocks: Vec<FunctionBlockState>,
    /// Where the debugger stopped the current scan, if it did.
    pub location: Option<Location>,
    /// Enabled breakpoints: (source file name, line).
    pub breakpoints: Vec<(String, usize)>,
}

impl Runtime {
    pub fn state(&self) -> EngineState {
        EngineState {
            programs: self.program_names(),
            cycle_count: self.cycle_count(),
            variables: self.variables(),
            function_blocks: self.function_blocks(),
            location: self.location().cloned(),
            breakpoints: self.breakpoints(),
        }
    }

    /// Plain (non function block) variables, by program in order.
    pub fn variables(&self) -> Vec<VariableState> {
        let mut out = Vec::new();
        for (i, unit) in self.units.iter().enumerate() {
            for symbol in unit.symbols.iter() {
                let Some(data_type) = symbol.data_type() else { continue };
                let slot = slot(i, &symbol.name);
                out.push(VariableState {
                    program: unit.name.clone(),
                    name: symbol.name.clone(),
                    data_type,
                    value: self.memory.values.get(&slot).copied().unwrap_or(Value::default_for(data_type)),
                    written_by_program: symbol.is_written(),
                    io_address: self.mapped.get(&slot).cloned(),
                });
            }
        }
        out
    }

    /// Function block instances, by program in order, with every member's value.
    pub fn function_blocks(&self) -> Vec<FunctionBlockState> {
        let mut out = Vec::new();
        for (i, unit) in self.units.iter().enumerate() {
            for symbol in unit.symbols.iter() {
                let prefix = slot(i, &symbol.name);
                let state = match symbol.var_type {
                    VarType::UserFb(index) => self.user_instance(&prefix, index),
                    VarType::FunctionBlock(kind) => {
                        let fb = self.memory.instances.get(&prefix);
                        let members = |list: &[(&str, DataType)]| -> Vec<MemberState> {
                            list.iter()
                                .map(|(name, data_type)| MemberState {
                                    name: name.to_string(),
                                    data_type: *data_type,
                                    value: fb.and_then(|fb| fb.get(name)).unwrap_or(Value::default_for(*data_type)),
                                })
                                .collect()
                        };
                        FunctionBlockState {
                            program: String::new(),
                            instance: String::new(),
                            type_name: kind.name().to_string(),
                            standard: true,
                            inputs: members(kind.inputs()),
                            outputs: members(kind.outputs()),
                            internals: Vec::new(),
                        }
                    }
                    VarType::Elementary(_) => continue,
                };
                out.push(FunctionBlockState { program: unit.name.clone(), instance: symbol.name.clone(), ..state });
            }
        }
        out
    }

    /// A project block instance: its members are slots under the instance's slot.
    fn user_instance(&self, prefix: &str, index: usize) -> FunctionBlockState {
        let fb = &self.fbs.interfaces[index];
        let member = |name: &str, data_type: DataType| MemberState {
            name: name.to_string(),
            data_type,
            value: self.memory.values.get(&format!("{prefix}.{}", key(name))).copied().unwrap_or(Value::default_for(data_type)),
        };
        FunctionBlockState {
            program: String::new(),
            instance: String::new(),
            type_name: fb.name.clone(),
            standard: false,
            inputs: fb.inputs.iter().map(|(n, t, _)| member(n, *t)).collect(),
            outputs: fb.outputs.iter().map(|(n, t)| member(n, *t)).collect(),
            internals: self.fbs.defs[index]
                .vars
                .iter()
                .filter_map(|decl| match decl.var_type {
                    VarType::Elementary(t) => Some(member(&decl.name, t)),
                    _ => None,
                })
                .collect(),
        }
    }
}
