//! TON — on-delay timer. Q goes TRUE once IN has been TRUE for PT; IN FALSE resets it.

use super::{Io, ScanClock};
use crate::st::value::Value;

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

#[cfg(test)]
mod tests {
    use super::super::test_support::Scans;
    use super::super::FbKind;

    #[test]
    fn q_after_pt_at_100ms_scans() {
        let mut t = Scans::new(FbKind::Ton, 2_000);
        for scan in 1..=19 {
            assert_eq!(t.scan(true), (false, scan * 100), "scan {scan}");
        }
        assert_eq!(t.scan(true), (true, 2_000), "scan 20 reaches PT");
        assert_eq!(t.scan(true), (true, 2_000), "ET holds at PT");
    }

    #[test]
    fn step_example_from_the_spec() {
        let mut t = Scans::new(FbKind::Ton, 500);
        let steps: Vec<_> = (0..5).map(|_| t.scan(true)).collect();
        assert_eq!(steps, vec![(false, 100), (false, 200), (false, 300), (false, 400), (true, 500)]);
    }

    #[test]
    fn in_false_resets_and_restarts() {
        let mut t = Scans::new(FbKind::Ton, 300);
        t.scan(true);
        t.scan(true);
        assert_eq!(t.scan(false), (false, 0));
        assert_eq!(t.scan(true), (false, 100), "timing restarts from the new edge");
        t.scan(true);
        assert_eq!(t.scan(true), (true, 300));
        assert_eq!(t.scan(false), (false, 0));
    }

    #[test]
    fn zero_preset_is_immediate() {
        let mut t = Scans::new(FbKind::Ton, 0);
        assert_eq!(t.scan(true), (true, 0));
    }
}
