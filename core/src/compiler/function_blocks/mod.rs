//! IEC 61131-3 standard function blocks: their interfaces (`FbKind`: input and output
//! members, used by the checker) and read-only reference sources. The behaviors and
//! instance state live in the runtime (`runtime::function_blocks`).

mod reference;

pub use reference::reference_source;

use super::ast::DataType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FbKind {
    /// On-delay timer.
    Ton,
    /// Off-delay timer.
    Tof,
    /// Pulse timer.
    Tp,
    /// Up counter.
    Ctu,
    /// Down counter.
    Ctd,
    /// Rising-edge detector.
    RTrig,
    /// Falling-edge detector.
    FTrig,
}

const TIMER_INPUTS: &[(&str, DataType)] = &[("IN", DataType::Bool), ("PT", DataType::Time)];
const TIMER_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("ET", DataType::Time)];
const CTU_INPUTS: &[(&str, DataType)] = &[("CU", DataType::Bool), ("R", DataType::Bool), ("PV", DataType::Int)];
const CTU_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("CV", DataType::Int)];
const CTD_INPUTS: &[(&str, DataType)] = &[("CD", DataType::Bool), ("LD", DataType::Bool), ("PV", DataType::Int)];
const CTD_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("CV", DataType::Int)];
const EDGE_INPUTS: &[(&str, DataType)] = &[("CLK", DataType::Bool)];
const EDGE_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool)];

impl FbKind {
    pub const ALL: [FbKind; 7] = [FbKind::Ton, FbKind::Tof, FbKind::Tp, FbKind::Ctu, FbKind::Ctd, FbKind::RTrig, FbKind::FTrig];

    /// Standard library names are not reserved words, so they arrive as identifiers.
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "TON" => Some(FbKind::Ton),
            "TOF" => Some(FbKind::Tof),
            "TP" => Some(FbKind::Tp),
            "CTU" => Some(FbKind::Ctu),
            "CTD" => Some(FbKind::Ctd),
            "R_TRIG" => Some(FbKind::RTrig),
            "F_TRIG" => Some(FbKind::FTrig),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            FbKind::Ton => "TON",
            FbKind::Tof => "TOF",
            FbKind::Tp => "TP",
            FbKind::Ctu => "CTU",
            FbKind::Ctd => "CTD",
            FbKind::RTrig => "R_TRIG",
            FbKind::FTrig => "F_TRIG",
        }
    }

    pub fn inputs(self) -> &'static [(&'static str, DataType)] {
        match self {
            FbKind::Ton | FbKind::Tof | FbKind::Tp => TIMER_INPUTS,
            FbKind::Ctu => CTU_INPUTS,
            FbKind::Ctd => CTD_INPUTS,
            FbKind::RTrig | FbKind::FTrig => EDGE_INPUTS,
        }
    }

    pub fn outputs(self) -> &'static [(&'static str, DataType)] {
        match self {
            FbKind::Ton | FbKind::Tof | FbKind::Tp => TIMER_OUTPUTS,
            FbKind::Ctu => CTU_OUTPUTS,
            FbKind::Ctd => CTD_OUTPUTS,
            FbKind::RTrig | FbKind::FTrig => EDGE_OUTPUTS,
        }
    }

    /// Inputs then outputs, the order used for storage and the monitor.
    pub fn members(self) -> impl Iterator<Item = (&'static str, DataType)> {
        self.inputs().iter().chain(self.outputs()).copied()
    }

    pub fn input_type(self, name: &str) -> Option<DataType> {
        lookup(self.inputs(), name)
    }

}

fn lookup(members: &[(&str, DataType)], name: &str) -> Option<DataType> {
    members.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, t)| *t)
}

