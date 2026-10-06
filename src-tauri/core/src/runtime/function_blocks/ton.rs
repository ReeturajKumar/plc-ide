//! TON — on-delay timer. Q goes TRUE once IN has been TRUE for PT; IN FALSE resets it.

use super::{Io, ScanClock};
use crate::compiler::value::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ton {
    /// When IN rose; None while IN is FALSE.
    started_at: Option<i64>,
}

impl Ton {
    pub fn execute(&mut self, io: &mut Io, clock: ScanClock) {
        let preset = io.time("PT").max(0);
        if io.bool("IN") {
            let started = *self.started_at.get_or_insert(clock.scan_start());
            let elapsed = (clock.now_ms - started).min(preset);
            io.put("ET", Value::Time(elapsed));
            io.put("Q", Value::Bool(elapsed >= preset));
        } else {
            self.started_at = None;
            io.put("ET", Value::Time(0));
            io.put("Q", Value::Bool(false));
        }
    }
}

