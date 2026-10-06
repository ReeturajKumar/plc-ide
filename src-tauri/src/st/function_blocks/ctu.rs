//! CTU — up counter. CV counts rising edges of CU; R resets CV; Q is CV >= PV.

use super::Io;
use crate::st::ast::DataType;
use crate::st::value::Value;

pub const INPUTS: &[(&str, DataType)] = &[("CU", DataType::Bool), ("R", DataType::Bool), ("PV", DataType::Int)];
pub const OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("CV", DataType::Int)];

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
        let c = FbKind::Ctu.instantiate();
        assert_eq!(cv_q(&c), (Value::Int(0), Value::Bool(false)));
    }

    #[test]
    fn spec_sequence_counts_edges_to_pv_then_resets() {
        let mut c = FbKind::Ctu.instantiate();
        call(&mut c, &[("PV", Value::Int(3))]);
        assert_eq!(cv_q(&c), (Value::Int(0), Value::Bool(false)));
        for expected in [1, 2, 3] {
            call(&mut c, &[("CU", Value::Bool(true))]);
            assert_eq!(c.get("CV"), Some(Value::Int(expected)));
            call(&mut c, &[("CU", Value::Bool(false))]);
        }
        assert_eq!(cv_q(&c), (Value::Int(3), Value::Bool(true)), "Q once CV >= PV");

        call(&mut c, &[("R", Value::Bool(true))]);
        assert_eq!(cv_q(&c), (Value::Int(0), Value::Bool(false)));
        call(&mut c, &[("CU", Value::Bool(true))]);
        assert_eq!(c.get("CV"), Some(Value::Int(0)), "edges don't count while R is held");
    }

    #[test]
    fn holding_cu_true_is_one_edge() {
        let mut c = FbKind::Ctu.instantiate();
        for _ in 0..10 {
            call(&mut c, &[("CU", Value::Bool(true))]);
        }
        assert_eq!(c.get("CV"), Some(Value::Int(1)));
    }

    #[test]
    fn q_follows_pv() {
        let mut c = FbKind::Ctu.instantiate();
        call(&mut c, &[("PV", Value::Int(2)), ("CU", Value::Bool(true))]);
        assert_eq!(cv_q(&c), (Value::Int(1), Value::Bool(false)));
        call(&mut c, &[("PV", Value::Int(1))]);
        assert_eq!(c.get("Q"), Some(Value::Bool(true)), "Q is re-evaluated against the current PV");
    }

    #[test]
    fn stops_at_int_max() {
        let mut c = FbKind::Ctu.instantiate();
        c.io.put("CV", Value::Int(i16::MAX));
        call(&mut c, &[("CU", Value::Bool(true))]);
        assert_eq!(c.get("CV"), Some(Value::Int(i16::MAX)));
    }

    #[test]
    fn instances_are_independent() {
        let (mut a, mut b) = (FbKind::Ctu.instantiate(), FbKind::Ctu.instantiate());
        for cu in [true, false, true] {
            call(&mut a, &[("CU", Value::Bool(cu))]);
        }
        call(&mut b, &[("CU", Value::Bool(true))]);
        assert_eq!((a.get("CV"), b.get("CV")), (Some(Value::Int(2)), Some(Value::Int(1))));
    }
}
