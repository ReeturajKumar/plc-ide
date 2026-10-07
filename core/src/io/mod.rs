//! PLC I/O: the address space (DI/DO/AI/AO), the `IoProvider` abstraction the runtime
//! reads inputs from and writes outputs to, the process image, its simulated
//! implementation, and the mapping of ST variables to addresses.
//!
//! Programs can use the addresses directly (`IF DI0 THEN`, `AO0 := Level;`) or through
//! mapped variables (StartButton → DI0). Inputs are read-only in programs; outputs are
//! written only by the PLC, never by the UI.

pub mod mapping;
pub mod simulation;
pub mod state;

use serde::{Deserialize, Serialize};

use crate::compiler::ast::{DataType, VarType};
use crate::compiler::symbols::{key, SymbolTable};

pub use mapping::{Mapping, MappingTarget, ResolvedMapping};
pub use simulation::InputValue;
pub use state::{IoImage, IoPoint};

/// Where the runtime's I/O comes from and goes to. The runtime only knows this trait;
/// the simulated process image implements it today, real hardware could later.
pub trait IoProvider {
    fn read_digital_input(&self, index: usize) -> bool;
    fn read_analog_input(&self, index: usize) -> f64;
    fn write_digital_output(&mut self, index: usize, value: bool);
    fn write_analog_output(&mut self, index: usize, value: f64);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IoKind {
    DigitalInput,
    DigitalOutput,
    AnalogInput,
    AnalogOutput,
}

impl IoKind {
    pub const ALL: [IoKind; 4] = [IoKind::DigitalInput, IoKind::DigitalOutput, IoKind::AnalogInput, IoKind::AnalogOutput];

    fn prefix(self) -> &'static str {
        match self {
            IoKind::DigitalInput => "DI",
            IoKind::DigitalOutput => "DO",
            IoKind::AnalogInput => "AI",
            IoKind::AnalogOutput => "AO",
        }
    }

    pub(crate) fn label(self) -> &'static str {
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

    /// The address of point `index` of this kind, e.g. DI0.
    pub fn address(self, index: usize) -> String {
        format!("{}{index}", self.prefix())
    }
}

/// How many points of each kind the PLC has.
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
    pub fn count(&self, kind: IoKind) -> usize {
        match kind {
            IoKind::DigitalInput => self.digital_inputs,
            IoKind::DigitalOutput => self.digital_outputs,
            IoKind::AnalogInput => self.analog_inputs,
            IoKind::AnalogOutput => self.analog_outputs,
        }
    }

    /// Every address in display order: DI0…, DO0…, AI0…, AO0….
    pub fn points(&self) -> Vec<(String, IoKind)> {
        IoKind::ALL.into_iter().flat_map(|kind| (0..self.count(kind)).map(move |i| (kind.address(i), kind))).collect()
    }

    /// The kind of `address` (case-insensitive), if this PLC has it.
    pub fn kind_of(&self, address: &str) -> Option<IoKind> {
        self.locate(address).map(|(kind, _)| kind)
    }

    /// The kind and index of `address` (case-insensitive), e.g. DI2 → (DigitalInput, 2).
    pub fn locate(&self, address: &str) -> Option<(IoKind, usize)> {
        let address = key(address);
        IoKind::ALL.into_iter().find_map(|kind| (0..self.count(kind)).find(|&i| kind.address(i) == address).map(|i| (kind, i)))
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

