//! R_TRIG / F_TRIG — edge detection. Q is TRUE for exactly one call after CLK changes:
//! FALSE → TRUE for R_TRIG, TRUE → FALSE for F_TRIG.
//!
//! Both start with "previous CLK = FALSE". For F_TRIG that differs from the literal IEC
//! definition (`Q := NOT CLK AND NOT M`, M starting FALSE), which also pulses on the very
//! first call when CLK is FALSE; here only a real TRUE → FALSE change pulses.

use super::Io;
use crate::compiler::value::Value;

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

