//! TP — pulse timer. A rising edge on IN starts a pulse: Q is TRUE for PT regardless of
//! IN. Edges during a pulse are ignored. After the pulse, ET holds at PT while IN stays
//! TRUE and resets once IN is FALSE, ready for the next edge.

use super::{Io, ScanClock};
use crate::compiler::value::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tp {
    /// When the current pulse started; None when idle.
    started_at: Option<i64>,
    previous_in: bool,
}

impl Tp {
    pub fn execute(&mut self, io: &mut Io, clock: ScanClock) {
        let input = io.bool("IN");
        if input && !self.previous_in && self.started_at.is_none() {
            self.started_at = Some(clock.scan_start());
        }
        self.previous_in = input;

        let Some(started) = self.started_at else {
            io.put("Q", Value::Bool(false));
            io.put("ET", Value::Time(0));
            return;
        };
        let preset = io.time("PT").max(0);
        let elapsed = (clock.now_ms - started).min(preset);
        let pulsing = elapsed < preset;
        io.put("Q", Value::Bool(pulsing));
        if !pulsing && !input {
            self.started_at = None;
            io.put("ET", Value::Time(0));
        } else {
            io.put("ET", Value::Time(elapsed));
        }
    }
}

