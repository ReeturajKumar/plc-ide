//! CTD — down counter. LD loads CV from PV; each rising edge of CD counts CV down;
//! Q is CV <= 0.

use super::Io;
use crate::compiler::value::Value;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Ctd {
    /// CD as of the previous call, for rising-edge detection.
    previous_cd: bool,
}

impl Ctd {
    pub fn execute(&mut self, io: &mut Io) {
        let cd = io.bool("CD");
        let rising = cd && !self.previous_cd;
        self.previous_cd = cd;

        let mut count = io.int("CV");
        if io.bool("LD") {
            count = io.int("PV");
        } else if rising && count > i16::MIN {
            count -= 1;
        }
        io.put("CV", Value::Int(count));
        io.put("Q", Value::Bool(count <= 0));
    }
}

