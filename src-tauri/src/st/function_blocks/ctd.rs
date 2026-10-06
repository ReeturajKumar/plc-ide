//! CTD — down counter. LD loads CV from PV; each rising edge of CD counts CV down;
//! Q is CV <= 0.

use super::Io;
use crate::st::ast::DataType;
use crate::st::value::Value;

pub const INPUTS: &[(&str, DataType)] = &[("CD", DataType::Bool), ("LD", DataType::Bool), ("PV", DataType::Int)];
pub const OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("CV", DataType::Int)];

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

#[cfg(test)]
mod tests {
    use super::super::test_support::call;
    use super::super::FbKind;
    use crate::st::value::Value;

    fn cv_q(fb: &super::super::FbInstance) -> (Value, Value) {
        (fb.get("CV").unwrap(), fb.get("Q").unwrap())
    }

    #[test]
    fn starts_at_zero() {
        let c = FbKind::Ctd.instantiate();
        assert_eq!(cv_q(&c), (Value::Int(0), Value::Bool(false)), "outputs are FALSE/0 until the first call");
    }

    #[test]
    fn load_then_count_down_to_zero() {
        let mut c = FbKind::Ctd.instantiate();
        call(&mut c, &[("PV", Value::Int(5)), ("LD", Value::Bool(true))]);
        assert_eq!(cv_q(&c), (Value::Int(5), Value::Bool(false)));
        call(&mut c, &[("LD", Value::Bool(false))]);

        for expected in [4, 3, 2, 1] {
            call(&mut c, &[("CD", Value::Bool(true))]);
            assert_eq!(cv_q(&c), (Value::Int(expected), Value::Bool(false)));
            call(&mut c, &[("CD", Value::Bool(false))]);
        }
        call(&mut c, &[("CD", Value::Bool(true))]);
        assert_eq!(cv_q(&c), (Value::Int(0), Value::Bool(true)), "Q at CV = 0");
    }

    #[test]
    fn holding_cd_counts_once() {
        let mut c = FbKind::Ctd.instantiate();
        call(&mut c, &[("PV", Value::Int(5)), ("LD", Value::Bool(true))]);
        call(&mut c, &[("LD", Value::Bool(false))]);
        for _ in 0..10 {
            call(&mut c, &[("CD", Value::Bool(true))]);
        }
        assert_eq!(c.get("CV"), Some(Value::Int(4)));
    }

    #[test]
    fn load_wins_over_an_edge_and_reloads() {
        let mut c = FbKind::Ctd.instantiate();
        call(&mut c, &[("PV", Value::Int(3)), ("LD", Value::Bool(true)), ("CD", Value::Bool(true))]);
        assert_eq!(c.get("CV"), Some(Value::Int(3)), "LD takes priority over CD");
        call(&mut c, &[("LD", Value::Bool(false)), ("CD", Value::Bool(false))]);
        call(&mut c, &[("CD", Value::Bool(true))]);
        assert_eq!(c.get("CV"), Some(Value::Int(2)));
        call(&mut c, &[("LD", Value::Bool(true))]);
        assert_eq!(c.get("CV"), Some(Value::Int(3)));
    }

    #[test]
    fn keeps_counting_below_zero_and_stops_at_int_min() {
        let mut c = FbKind::Ctd.instantiate();
        for cd in [true, false, true] {
            call(&mut c, &[("CD", Value::Bool(cd))]);
        }
        assert_eq!(cv_q(&c), (Value::Int(-2), Value::Bool(true)));

        let mut c = FbKind::Ctd.instantiate();
        call(&mut c, &[("PV", Value::Int(i16::MIN)), ("LD", Value::Bool(true))]);
        call(&mut c, &[("LD", Value::Bool(false)), ("CD", Value::Bool(true))]);
        assert_eq!(c.get("CV"), Some(Value::Int(i16::MIN)));
    }
}
