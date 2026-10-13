//! CTUD — up/down counter. Rising edges of CU count CV up, of CD count it down; RESET
//! sets CV to 0, LOAD to PV (RESET wins). QU is CV >= PV, QD is CV <= 0.

use super::Io;
use crate::compiler::value::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ctud {
    /// CU and CD as of the previous call, for rising-edge detection.
    previous_cu: bool,
    previous_cd: bool,
}

impl Ctud {
    pub fn execute(&mut self, io: &mut Io) {
        let (cu, cd) = (io.bool("CU"), io.bool("CD"));
        let (up, down) = (cu && !self.previous_cu, cd && !self.previous_cd);
        (self.previous_cu, self.previous_cd) = (cu, cd);

        let mut count = io.int("CV");
        if io.bool("RESET") {
            count = 0;
        } else if io.bool("LOAD") {
            count = io.int("PV");
        } else {
            if up && count < i16::MAX {
                count += 1;
            }
            if down && count > i16::MIN {
                count -= 1;
            }
        }
        io.put("CV", Value::Int(count));
        io.put("QU", Value::Bool(count >= io.int("PV")));
        io.put("QD", Value::Bool(count <= 0));
    }
}
