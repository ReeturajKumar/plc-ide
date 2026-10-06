//! TOF — off-delay timer. Q follows IN going TRUE immediately, but stays TRUE for PT
//! after IN goes FALSE. ET counts from the falling edge and holds at PT once expired.

use super::{Io, ScanClock};
use crate::st::value::Value;

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

#[cfg(test)]
mod tests {
    use super::super::test_support::Scans;
    use super::super::FbKind;

    #[test]
    fn q_holds_for_pt_after_in_falls() {
        let mut t = Scans::new(FbKind::Tof, 1_000);
        assert_eq!(t.scan(true), (true, 0));
        assert_eq!(t.scan(true), (true, 0));
        for scan in 1..=9 {
            assert_eq!(t.scan(false), (true, scan * 100), "Q still TRUE {scan} scans after IN fell");
        }
        assert_eq!(t.scan(false), (false, 1_000), "PT expired");
        assert_eq!(t.scan(false), (false, 1_000), "ET holds at PT");
    }

    #[test]
    fn in_true_during_timing_cancels_it() {
        let mut t = Scans::new(FbKind::Tof, 1_000);
        t.scan(true);
        t.scan(false);
        t.scan(false);
        assert_eq!(t.scan(true), (true, 0));
        assert_eq!(t.scan(false), (true, 100), "a new falling edge starts over");
    }

    #[test]
    fn stays_off_until_in_was_true() {
        let mut t = Scans::new(FbKind::Tof, 1_000);
        assert_eq!(t.scan(false), (false, 0));
        assert_eq!(t.scan(false), (false, 0));
    }
}
