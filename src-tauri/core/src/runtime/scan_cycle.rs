//! The PLC scan cycle: READ INPUTS → every program once, in project order → WRITE OUTPUTS,
//! on the PLC clock. I/O goes through the `IoProvider` abstraction, never a concrete image.

use super::debugger::{Location, Stop};
use super::executor::{slot, ProgramError};
use super::{Runtime, Target};
use crate::io::{IoKind, IoProvider};
use crate::runtime::function_blocks::ScanClock;
use crate::compiler::value::Value;

/// PLC time and completed scans since the runtime was loaded or reset. Timers run on this
/// clock: exactly one scan interval per scan, so scan 20 of a 100 ms cycle is always
/// 2000 ms however long the scans really took, and a pause freezes them.
#[derive(Debug, Clone, Copy, Default)]
pub(super) struct ScanCycle {
    clock_ms: i64,
    cycles: u64,
}

/// How a call of `scan_cycle` ended.
#[derive(Debug, Clone, PartialEq)]
pub enum ScanOutcome {
    /// The scan completed: outputs written, one more cycle counted.
    Completed,
    /// The debugger stopped it before a statement; the next call carries on from there.
    Paused(Location),
    /// A program failed; the scan didn't complete.
    Failed(ProgramError),
}

impl Runtime {
    /// One PLC scan of `cycle_ms`: read the inputs from `io`, run every program once, write
    /// the outputs to `io`. The debugger may stop it before a statement (`stop`); the next
    /// call then carries on from there instead of starting a new scan.
    pub fn scan_cycle(&mut self, io: &mut dyn IoProvider, cycle_ms: i64, stop: Stop) -> ScanOutcome {
        let outcome = if self.location().is_some() {
            self.proceed(stop)
        } else {
            self.cycle.clock_ms = self.cycle.clock_ms.saturating_add(cycle_ms);
            self.read_inputs(io);
            self.run(ScanClock { now_ms: self.cycle.clock_ms, cycle_ms }, stop)
        };
        match outcome {
            Ok(None) => {
                self.write_outputs(io);
                self.cycle.cycles += 1;
                ScanOutcome::Completed
            }
            Ok(Some(location)) => ScanOutcome::Paused(location),
            Err(failure) => ScanOutcome::Failed(failure),
        }
    }

    /// Completed scans since load or reset.
    pub fn cycle_count(&self) -> u64 {
        self.cycle.cycles
    }

    pub(super) fn reset_cycle(&mut self) {
        self.cycle = ScanCycle::default();
    }

    /// READ INPUTS, the first step of a scan: take every input, then copy them into the
    /// mapped variables. Programs see these values for the whole scan.
    fn read_inputs(&mut self, io: &dyn IoProvider) {
        for kind in [IoKind::DigitalInput, IoKind::AnalogInput] {
            for index in 0..self.io_config.count(kind) {
                let value = match kind {
                    IoKind::DigitalInput => Value::Bool(io.read_digital_input(index)),
                    _ => Value::Real(io.read_analog_input(index)),
                };
                self.memory.values.insert(slot(self.io_index, &kind.address(index)), value);
            }
        }
        for (io_slot, target) in &self.inputs {
            let Some(&value) = self.memory.values.get(io_slot) else { continue };
            match target {
                Target::Slot(slot) => {
                    self.memory.values.insert(slot.clone(), value);
                }
                Target::Builtin { instance, member } => {
                    if let Some(fb) = self.memory.instances.get_mut(instance) {
                        let _ = fb.set_input(member, value); // type checked by the mapping validation
                    }
                }
            }
        }
    }

    /// WRITE OUTPUTS, the last step of a scan: copy the mapped variables to their outputs,
    /// then write every output.
    fn write_outputs(&mut self, io: &mut dyn IoProvider) {
        for (source, io_slot) in &self.outputs {
            let value = match source {
                Target::Slot(slot) => self.memory.values.get(slot).copied(),
                Target::Builtin { instance, member } => self.memory.instances.get(instance).and_then(|fb| fb.get(member)),
            };
            if let Some(value) = value {
                self.memory.values.insert(io_slot.clone(), value);
            }
        }
        for kind in [IoKind::DigitalOutput, IoKind::AnalogOutput] {
            for index in 0..self.io_config.count(kind) {
                match self.memory.values.get(&slot(self.io_index, &kind.address(index))) {
                    Some(Value::Bool(on)) if kind == IoKind::DigitalOutput => io.write_digital_output(index, *on),
                    Some(Value::Real(x)) if kind == IoKind::AnalogOutput => io.write_analog_output(index, *x),
                    _ => {}
                }
            }
        }
    }
}
