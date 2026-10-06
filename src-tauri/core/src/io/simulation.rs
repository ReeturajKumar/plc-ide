//! Simulated I/O: the process image driven by the simulator UI. It is the `IoProvider`
//! the runtime uses while there is no real hardware.

use serde::{Deserialize, Serialize};

use super::state::IoImage;
use super::{IoKind, IoProvider};
use crate::compiler::symbols::key;
use crate::compiler::value::Value;

/// A value the simulator UI sets on an input.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum InputValue {
    Bool(bool),
    Number(f64),
}

impl IoImage {
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
}

impl IoProvider for IoImage {
    fn read_digital_input(&self, index: usize) -> bool {
        self.point(IoKind::DigitalInput, index).and_then(|p| p.value.as_bool()).unwrap_or(false)
    }

    fn read_analog_input(&self, index: usize) -> f64 {
        match self.point(IoKind::AnalogInput, index).map(|p| p.value) {
            Some(Value::Real(x)) => x,
            _ => 0.0,
        }
    }

    fn write_digital_output(&mut self, index: usize, value: bool) {
        if let Some(point) = self.point_mut(IoKind::DigitalOutput, index) {
            point.value = Value::Bool(value);
        }
    }

    fn write_analog_output(&mut self, index: usize, value: f64) {
        if let Some(point) = self.point_mut(IoKind::AnalogOutput, index) {
            point.value = Value::Real(value);
        }
    }
}

