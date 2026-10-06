//! Simulated PLC I/O: the process image (inputs set by the simulator UI, outputs written
//! by the PLC after each scan) and the mapping of ST variables to I/O addresses.
//!
//! Programs can use the addresses directly (`IF DI0 THEN`, `AO0 := Level;`) or through
//! mapped variables (StartButton → DI0). Inputs are read-only in programs; outputs are
//! written only by the PLC, never by the UI.

use serde::{Deserialize, Serialize};

use crate::st::ast::{DataType, VarType};
use crate::st::error::{ErrorKind, StError, StErrors};
use std::collections::HashSet;

use crate::st::symbols::{key, Scope, Symbol, SymbolTable};
use crate::st::value::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IoKind {
    DigitalInput,
    DigitalOutput,
    AnalogInput,
    AnalogOutput,
}

impl IoKind {
    const ALL: [IoKind; 4] = [IoKind::DigitalInput, IoKind::DigitalOutput, IoKind::AnalogInput, IoKind::AnalogOutput];

    fn prefix(self) -> &'static str {
        match self {
            IoKind::DigitalInput => "DI",
            IoKind::DigitalOutput => "DO",
            IoKind::AnalogInput => "AI",
            IoKind::AnalogOutput => "AO",
        }
    }

    fn label(self) -> &'static str {
        match self {
            IoKind::DigitalInput => "DIGITAL INPUT",
            IoKind::DigitalOutput => "DIGITAL OUTPUT",
            IoKind::AnalogInput => "ANALOG INPUT",
            IoKind::AnalogOutput => "ANALOG OUTPUT",
        }
    }

    pub fn data_type(self) -> DataType {
        match self {
            IoKind::DigitalInput | IoKind::DigitalOutput => DataType::Bool,
            IoKind::AnalogInput | IoKind::AnalogOutput => DataType::Real,
        }
    }

    pub fn is_input(self) -> bool {
        matches!(self, IoKind::DigitalInput | IoKind::AnalogInput)
    }
}

/// How many points of each kind the simulated PLC has.
#[derive(Debug, Clone, Copy)]
pub struct IoConfig {
    pub digital_inputs: usize,
    pub digital_outputs: usize,
    pub analog_inputs: usize,
    pub analog_outputs: usize,
}

/// The fixed simulated I/O of this phase: DI0-3, DO0-3, AI0-1, AO0-1.
pub const CONFIG: IoConfig = IoConfig { digital_inputs: 4, digital_outputs: 4, analog_inputs: 2, analog_outputs: 2 };

impl IoConfig {
    fn count(&self, kind: IoKind) -> usize {
        match kind {
            IoKind::DigitalInput => self.digital_inputs,
            IoKind::DigitalOutput => self.digital_outputs,
            IoKind::AnalogInput => self.analog_inputs,
            IoKind::AnalogOutput => self.analog_outputs,
        }
    }

    /// Every address in display order: DI0…, DO0…, AI0…, AO0….
    pub fn points(&self) -> Vec<(String, IoKind)> {
        IoKind::ALL
            .into_iter()
            .flat_map(|kind| (0..self.count(kind)).map(move |i| (format!("{}{i}", kind.prefix()), kind)))
            .collect()
    }

    /// The kind of `address` (case-insensitive), if this PLC has it.
    pub fn kind_of(&self, address: &str) -> Option<IoKind> {
        let address = key(address);
        self.points().into_iter().find(|(a, _)| *a == address).map(|(_, kind)| kind)
    }

    /// The addresses as symbols, so programs can use them like variables.
    pub fn symbols(&self) -> SymbolTable {
        let mut table = SymbolTable::default();
        for (address, kind) in self.points() {
            table.declare_io(&address, VarType::Elementary(kind.data_type()), kind.is_input());
        }
        table
    }
}

/// One I/O point and its current value, as the monitor shows it.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IoPoint {
    pub address: String,
    pub kind: IoKind,
    pub value: Value,
}

/// A value the simulator UI sets on an input.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    Bool(bool),
    Number(f64),
}

/// The process image. Inputs belong to the simulator UI, outputs to the PLC.
#[derive(Debug, Clone)]
pub struct IoImage {
    points: Vec<IoPoint>,
}

impl IoImage {
    pub fn new(config: &IoConfig) -> Self {
        let points = config
            .points()
            .into_iter()
            .map(|(address, kind)| IoPoint { address, kind, value: Value::default_for(kind.data_type()) })
            .collect();
        Self { points }
    }

    pub fn points(&self) -> &[IoPoint] {
        &self.points
    }

    #[cfg(test)]
    pub fn get(&self, address: &str) -> Option<Value> {
        self.points.iter().find(|p| p.address == key(address)).map(|p| p.value)
    }

    /// Set a simulated input. Outputs are refused: only the PLC writes them.
    pub fn set_input(&mut self, address: &str, value: InputValue) -> Result<(), String> {
        let point = self
            .points
            .iter_mut()
            .find(|p| p.address == key(address))
            .ok_or_else(|| format!("Unknown I/O address '{address}'."))?;
        if !point.kind.is_input() {
            return Err(format!("{} is an output: only the PLC program drives it.", point.address));
        }
        point.value = match (point.kind, value) {
            (IoKind::DigitalInput, InputValue::Bool(b)) => Value::Bool(b),
            (IoKind::AnalogInput, InputValue::Number(x)) if x.is_finite() => Value::Real(x),
            (IoKind::AnalogInput, InputValue::Number(_)) => return Err(format!("{} needs a finite number.", point.address)),
            (IoKind::DigitalInput, _) => return Err(format!("{} is digital: set it ON or OFF.", point.address)),
            _ => return Err(format!("{} needs a number.", point.address)),
        };
        Ok(())
    }

    /// Written by the PLC at the end of a scan.
    pub fn set_output(&mut self, address: &str, value: Value) {
        if let Some(point) = self.points.iter_mut().find(|p| p.address == address && !p.kind.is_input()) {
            point.value = value;
        }
    }

    /// The safe state when the PLC isn't running: every output OFF / 0.0. Inputs are kept.
    pub fn reset_outputs(&mut self) {
        for point in self.points.iter_mut().filter(|p| !p.kind.is_input()) {
            point.value = Value::default_for(point.kind.data_type());
        }
    }
}

/// `variable` (declared by one program of the project) is wired to `address`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_the_spec_points() {
        let addresses: Vec<String> = CONFIG.points().into_iter().map(|(a, _)| a).collect();
        assert_eq!(addresses, ["DI0", "DI1", "DI2", "DI3", "DO0", "DO1", "DO2", "DO3", "AI0", "AI1", "AO0", "AO1"]);
        assert_eq!(CONFIG.kind_of("ai1"), Some(IoKind::AnalogInput));
        assert_eq!(CONFIG.kind_of("DI4"), None);
        let symbols = CONFIG.symbols();
        assert!(symbols.get("DI0").unwrap().read_only && !symbols.get("DO0").unwrap().read_only);
        assert_eq!(symbols.get("AO1").unwrap().data_type(), Some(DataType::Real));
    }

    #[test]
    fn the_ui_sets_inputs_only() {
        let mut image = IoImage::new(&CONFIG);
        image.set_input("di0", InputValue::Bool(true)).unwrap();
        image.set_input("AI0", InputValue::Number(75.5)).unwrap();
        assert_eq!((image.get("DI0"), image.get("AI0")), (Some(Value::Bool(true)), Some(Value::Real(75.5))));
        assert_eq!(image.set_input("DO0", InputValue::Bool(true)).unwrap_err(), "DO0 is an output: only the PLC program drives it.");
        assert!(image.set_input("DI0", InputValue::Number(1.0)).is_err());
        assert!(image.set_input("AI0", InputValue::Bool(true)).is_err());
        assert!(image.set_input("AI0", InputValue::Number(f64::NAN)).is_err());
        assert!(image.set_input("XX9", InputValue::Bool(true)).is_err());

        image.set_output("DO0", Value::Bool(true));
        image.set_output("DI1", Value::Bool(true)); // not an output: ignored
        assert_eq!((image.get("DO0"), image.get("DI1")), (Some(Value::Bool(true)), Some(Value::Bool(false))));
        image.reset_outputs();
        assert_eq!(image.get("DO0"), Some(Value::Bool(false)));
        assert_eq!(image.get("DI0"), Some(Value::Bool(true)), "inputs survive a reset");
    }
}
