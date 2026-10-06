//! CTU — up counter. CV counts rising edges of CU; R resets CV; Q is CV >= PV.

use super::Io;
use crate::compiler::value::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ctu {
    /// CU as of the previous call, for rising-edge detection.
    previous_cu: bool,
}

impl Ctu {
    pub fn execute(&mut self, io: &mut Io) {
        let cu = io.bool("CU");
        let rising = cu && !self.previous_cu;
        self.previous_cu = cu;

        let mut count = io.int("CV");
        if io.bool("R") {
            count = 0;
        } else if rising && count < i16::MAX {
            count += 1;
        }
        io.put("CV", Value::Int(count));
        io.put("Q", Value::Bool(count >= io.int("PV")));
    }
}

