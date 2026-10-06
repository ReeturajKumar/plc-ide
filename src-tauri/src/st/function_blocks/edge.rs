//! R_TRIG / F_TRIG — edge detection. Q is TRUE for exactly one call after CLK changes:
//! FALSE → TRUE for R_TRIG, TRUE → FALSE for F_TRIG.
//!
//! Both start with "previous CLK = FALSE". For F_TRIG that differs from the literal IEC
//! definition (`Q := NOT CLK AND NOT M`, M starting FALSE), which also pulses on the very
//! first call when CLK is FALSE; here only a real TRUE → FALSE change pulses.

use super::Io;
use crate::st::ast::DataType;
use crate::st::value::Value;

pub const INPUTS: &[(&str, DataType)] = &[("CLK", DataType::Bool)];
pub const OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool)];

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RTrig {
    previous_clk: bool,
}

impl RTrig {
    pub fn execute(&mut self, io: &mut Io) {
        let clk = io.bool("CLK");
        io.put("Q", Value::Bool(clk && !self.previous_clk));
        self.previous_clk = clk;
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct FTrig {
    previous_clk: bool,
}

impl FTrig {
    pub fn execute(&mut self, io: &mut Io) {
        let clk = io.bool("CLK");
        io.put("Q", Value::Bool(!clk && self.previous_clk));
        self.previous_clk = clk;
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::call;
    use super::super::FbKind;
    use crate::st::value::Value;

    /// Q after each call with the given CLK sequence.
    fn pulses(kind: FbKind, clk: &[bool]) -> Vec<bool> {
        let mut fb = kind.instantiate();
        clk.iter()
            .map(|&c| {
                call(&mut fb, &[("CLK", Value::Bool(c))]);
                fb.get("Q").and_then(Value::as_bool).unwrap()
            })
            .collect()
    }

    #[test]
    fn r_trig_spec_sequence() {
        // Scan:          1      2     3      4      5
        let clk = [false, true, true, false, true];
        assert_eq!(pulses(FbKind::RTrig, &clk), [false, true, false, false, true]);
    }

    #[test]
    fn r_trig_holding_true_pulses_once() {
        assert_eq!(pulses(FbKind::RTrig, &[true; 5]), [true, false, false, false, false]);
    }

    #[test]
    fn f_trig_spec_sequence() {
        let clk = [true, false, false, true, false];
        assert_eq!(pulses(FbKind::FTrig, &clk), [false, true, false, false, true]);
    }

    #[test]
    fn f_trig_does_not_pulse_when_clk_starts_false() {
        assert_eq!(pulses(FbKind::FTrig, &[false; 4]), [false; 4]);
    }

    #[test]
    fn instances_are_independent() {
        let mut a = FbKind::RTrig.instantiate();
        let mut b = FbKind::RTrig.instantiate();
        call(&mut a, &[("CLK", Value::Bool(true))]);
        call(&mut b, &[("CLK", Value::Bool(false))]);
        call(&mut b, &[("CLK", Value::Bool(true))]);
        assert_eq!(b.get("Q"), Some(Value::Bool(true)), "b's edge is its own");
        call(&mut a, &[("CLK", Value::Bool(true))]);
        assert_eq!(a.get("Q"), Some(Value::Bool(false)), "a remembers it was already TRUE");
    }
}
