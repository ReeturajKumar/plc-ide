//! I/O mapping: wiring ST variables and function block members to I/O addresses, and
//! checking that wiring against the analyzed project.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use super::{IoConfig, IoKind};
use crate::compiler::ast::{DataType, VarType};
use crate::compiler::error::{ErrorKind, StError, StErrors};
use crate::compiler::symbols::{key, Scope, Symbol, SymbolTable};

/// `variable` (declared by one program of the project) is wired to `address`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Mapping {
    pub address: String,
    pub variable: String,
}

fn mapping_error(message: String) -> StError {
    StError::new(ErrorKind::InvalidIoMapping, 0, 0, message)
}

/// What a mapping's variable refers to.
pub enum Endpoint<'a> {
    /// A program variable (`Level`, `main.Level`).
    Variable { owner: usize, symbol: &'a Symbol },
    /// A function block instance itself (only its members can be mapped).
    Instance { symbol: &'a Symbol },
    /// An input or output of a function block instance a program declares
    /// (`Motor1.Start`, `main.Timer1.Q`).
    Member { owner: usize, instance: &'a Symbol, member: String, data_type: DataType, input: bool, user: bool },
}

/// Resolve a mapping target: `Variable`, `Program.Variable`, `Instance.Member` or
/// `Program.Instance.Member`. Err says why it doesn't resolve.
pub fn resolve<'a>(name: &str, scope: &Scope<'a>) -> Result<Endpoint<'a>, String> {
    if let Some((owner, symbol)) = scope.lookup(name) {
        return Ok(match symbol.var_type {
            VarType::Elementary(_) => Endpoint::Variable { owner, symbol },
            _ => Endpoint::Instance { symbol },
        });
    }
    if let Some((prefix, member)) = name.rsplit_once('.') {
        if let Some((owner, instance)) = scope.lookup(prefix) {
            let Some(fb) = scope.interface(instance.var_type) else {
                return Err(format!("'{}' is a variable, not a function block instance", instance.name));
            };
            let (data_type, input) = match (fb.input_type(member), fb.outputs.iter().find(|(n, _)| n.eq_ignore_ascii_case(member))) {
                (Some(t), _) => (t, true),
                (None, Some((_, t))) => (*t, false),
                (None, None) if fb.is_internal(member) => {
                    return Err(format!("'{member}' is internal to function block {}; map one of its inputs or outputs", fb.name))
                }
                (None, None) => return Err(format!("{} has no input or output '{member}' (members: {})", fb.name, fb.member_names())),
            };
            let member = fb.inputs.iter().map(|(n, _, _)| n).chain(fb.outputs.iter().map(|(n, _)| n)).find(|n| n.eq_ignore_ascii_case(member));
            return Ok(Endpoint::Member { owner, instance, member: member.cloned().unwrap_or_default(), data_type, input, user: fb.user });
        }
    }
    let owners = scope.declared_in(name);
    Err(match name.split_once('.') {
        Some((program, variable)) => format!("program '{program}' has no variable '{variable}'"),
        None if owners.len() > 1 => format!(
            "'{name}' is declared in programs {}; write {}",
            owners.join(", "),
            owners.iter().map(|p| format!("{p}.{name}")).collect::<Vec<_>>().join(" or ")
        ),
        None => scope.undeclared(name, "variable"),
    })
}

/// Function block inputs that mappings feed from DI/AI, as "INSTANCE.MEMBER" (upper case):
/// a call doesn't need to give those inputs, and mustn't also set them.
pub fn mapped_block_inputs(mappings: &[Mapping], config: &IoConfig) -> HashSet<String> {
    mappings
        .iter()
        .filter(|m| config.kind_of(&m.address).is_some_and(IoKind::is_input))
        .filter_map(|m| {
            let (prefix, member) = m.variable.rsplit_once('.')?;
            let instance = prefix.rsplit('.').next()?;
            Some(format!("{}.{}", key(instance), key(member)))
        })
        .collect()
}

/// Check every mapping against the analyzed project. `scope` is the I/O table's scope
/// (it sees every program's variables) and `io` that table.
pub fn validate(mappings: &[Mapping], config: &IoConfig, scope: &Scope, io: &SymbolTable) -> StErrors {
    let mut errors = StErrors::new();
    let mut inputs_of: Vec<(String, String)> = Vec::new(); // target → its input address
    let mut outputs: Vec<(String, String)> = Vec::new(); // output address → its source
    for m in mappings {
        let Some(kind) = config.kind_of(&m.address) else {
            errors.push(mapping_error(format!("Unknown I/O address '{}' (mapped to '{}').", m.address, m.variable)));
            continue;
        };
        let address = key(&m.address);
        let what = format!("{} {address}", kind.label());
        let endpoint = match resolve(&m.variable, scope) {
            Ok(Endpoint::Variable { symbol, .. }) if io.get(&symbol.name).is_some() => {
                errors.push(mapping_error(format!("Cannot map {what}: {} is an I/O address, not a variable.", symbol.name)));
                continue;
            }
            Ok(endpoint) => endpoint,
            Err(why) => {
                errors.push(mapping_error(format!("Cannot map {what}: {why}.")));
                continue;
            }
        };
        let (data_type, label, target) = match endpoint {
            Endpoint::Variable { owner, symbol } => {
                (symbol.data_type().unwrap_or(DataType::Bool), format!("variable '{}'", symbol.name), format!("{owner}.{}", key(&symbol.name)))
            }
            Endpoint::Instance { symbol } => {
                let fb = scope.interface(symbol.var_type);
                let example = fb.as_ref().and_then(|f| f.outputs.first().map(|(n, _)| n.clone())).unwrap_or_else(|| "…".into());
                errors.push(mapping_error(format!(
                    "Cannot map {what} to {} instance '{}'; map one of its inputs or outputs, e.g. {}.{example}.",
                    scope.type_name(symbol.var_type),
                    symbol.name,
                    symbol.name
                )));
                continue;
            }
            Endpoint::Member { owner, instance, member, data_type, input, .. } => {
                if kind.is_input() && !input {
                    errors.push(mapping_error(format!(
                        "Cannot map {what} to {}.{member}: it is an output, written by the block.",
                        instance.name
                    )));
                    continue;
                }
                let role = if input { "input" } else { "output" };
                (data_type, format!("{role} '{}.{member}'", instance.name), format!("{owner}.{}.{}", key(&instance.name), key(&member)))
            }
        };
        if data_type != kind.data_type() {
            errors.push(mapping_error(format!("Cannot map {what} to {} {label}.", data_type.name())));
            continue;
        }
        // A target reads one input; an output is driven by one source. A program may also
        // assign a mapped variable or an output directly: inputs are copied in before the
        // programs run (so the input is back next scan), mapped outputs are copied out after
        // them (so the mapping wins).
        let name = m.variable.trim().to_string();
        let (taken, mine) = if kind.is_input() { (&mut inputs_of, target) } else { (&mut outputs, address.clone()) };
        if let Some((_, other)) = taken.iter().find(|(k, _)| *k == mine) {
            errors.push(mapping_error(if kind.is_input() {
                format!("'{name}' is mapped to two inputs, {other} and {address}.")
            } else {
                format!("{address} is mapped more than once ('{other}' and '{name}').")
            }));
        }
        taken.push((mine, if kind.is_input() { address.clone() } else { name }));
    }
    errors
}

/// A validated mapping, resolved to what the runtime reads or writes.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedMapping {
    pub kind: IoKind,
    /// Point index within its kind (DI2 → 2).
    pub index: usize,
    pub target: MappingTarget,
}

/// The ST side of a mapping, by program index (in project order).
#[derive(Debug, Clone, PartialEq)]
pub enum MappingTarget {
    /// A program variable.
    Variable { program: usize, name: String },
    /// An input or output of a function block instance declared by a program; `standard`
    /// for TON, CTU, …, as opposed to a project FUNCTION_BLOCK.
    BlockMember { program: usize, instance: String, member: String, standard: bool },
}

/// Resolve mappings that `validate` accepted (anything else is skipped).
pub fn resolve_all(mappings: &[Mapping], config: &IoConfig, scope: &Scope) -> Vec<ResolvedMapping> {
    mappings
        .iter()
        .filter_map(|m| {
            let (kind, index) = config.locate(&m.address)?;
            let target = match resolve(&m.variable, scope).ok()? {
                Endpoint::Variable { owner, symbol } => MappingTarget::Variable { program: owner, name: symbol.name.clone() },
                Endpoint::Member { owner, instance, member, user, .. } => {
                    MappingTarget::BlockMember { program: owner, instance: instance.name.clone(), member, standard: !user }
                }
                Endpoint::Instance { .. } => return None,
            };
            Some(ResolvedMapping { kind, index, target })
        })
        .collect()
}
