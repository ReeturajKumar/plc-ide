//! TOF — off-delay timer. Q follows IN going TRUE immediately, but stays TRUE for PT
//! after IN goes FALSE. ET counts from the falling edge and holds at PT once expired.

use super::{Io, ScanClock};
use crate::compiler::value::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Tof {
    /// When IN fell; None while IN is TRUE or before it was ever TRUE.
    fell_at: Option<i64>,
    previous_in: bool,
}

impl Tof {
    pub fn execute(&mut self, io: &mut Io, clock: ScanClock) {
        let input = io.bool("IN");
        if input {
            self.fell_at = None;
            io.put("Q", Value::Bool(true));
            io.put("ET", Value::Time(0));
        } else {
            if self.previous_in {
                self.fell_at = Some(clock.scan_start());
            }
            // Before IN was ever TRUE nothing is timing, so Q stays FALSE.
            if let Some(fell) = self.fell_at {
                let preset = io.time("PT").max(0);
                let elapsed = (clock.now_ms - fell).min(preset);
                io.put("ET", Value::Time(elapsed));
                io.put("Q", Value::Bool(elapsed < preset));
            }
        }
        self.previous_in = input;
    }
}

