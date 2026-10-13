//! SR / RS — bistables (latches). Q1 is set and reset by the inputs and keeps its value
//! while both are FALSE. When both are TRUE, SR sets (set-dominant) and RS resets
//! (reset-dominant).

use super::Io;
use crate::compiler::value::Value;

pub fn sr(io: &mut Io) {
    let q1 = io.bool("SET1") || (!io.bool("RESET") && io.bool("Q1"));
    io.put("Q1", Value::Bool(q1));
}

pub fn rs(io: &mut Io) {
    let q1 = !io.bool("RESET1") && (io.bool("SET") || io.bool("Q1"));
    io.put("Q1", Value::Bool(q1));
}
