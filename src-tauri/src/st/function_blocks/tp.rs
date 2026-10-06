//! TP — pulse timer. A rising edge on IN starts a pulse: Q is TRUE for PT regardless of
//! IN. Edges during a pulse are ignored. After the pulse, ET holds at PT while IN stays
//! TRUE and resets once IN is FALSE, ready for the next edge.

use super::{Io, ScanClock};
use crate::st::value::Value;

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

#[cfg(test)]
mod tests {
    use super::super::test_support::Scans;
    use super::super::FbKind;

    #[test]
    fn rising_edge_gives_one_pulse_of_pt() {
        let mut t = Scans::new(FbKind::Tp, 1_000);
        assert_eq!(t.scan(false), (false, 0));
        assert_eq!(t.scan(true), (true, 100), "edge starts the pulse");
        for scan in 2..=9 {
            // IN is released mid-pulse: the pulse continues regardless.
            assert_eq!(t.scan(scan < 4), (true, scan * 100), "scan {scan}");
        }
        assert_eq!(t.scan(false), (false, 0), "pulse over and IN FALSE: ready again");
    }

    #[test]
    fn edges_during_a_pulse_are_ignored() {
        let mut t = Scans::new(FbKind::Tp, 500);
        t.scan(true);
        t.scan(false);
        assert_eq!(t.scan(true), (true, 300), "second edge doesn't restart the pulse");
        t.scan(true);
        assert_eq!(t.scan(true), (false, 500), "pulse ends at PT; ET holds while IN stays TRUE");
        assert_eq!(t.scan(true), (false, 500), "holding IN doesn't start another pulse");
        assert_eq!(t.scan(false), (false, 0));
        assert_eq!(t.scan(true), (true, 100), "a fresh edge starts a new pulse");
    }
}
