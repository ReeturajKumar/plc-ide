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
    /// Up/down counter.
    Ctud,
    /// Set-dominant bistable (SET1 wins over RESET).
    Sr,
    /// Reset-dominant bistable (RESET1 wins over SET).
    Rs,
}

const TIMER_INPUTS: &[(&str, DataType)] = &[("IN", DataType::Bool), ("PT", DataType::Time)];
const TIMER_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("ET", DataType::Time)];
// R / LD are the IEC names; RESET / LOAD the names other IDEs use. Either works.
const CTU_INPUTS: &[(&str, DataType)] =
    &[("CU", DataType::Bool), ("R", DataType::Bool), ("RESET", DataType::Bool), ("PV", DataType::Int)];
const CTU_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("CV", DataType::Int)];
const CTD_INPUTS: &[(&str, DataType)] =
    &[("CD", DataType::Bool), ("LD", DataType::Bool), ("LOAD", DataType::Bool), ("PV", DataType::Int)];
const CTD_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool), ("CV", DataType::Int)];
const EDGE_INPUTS: &[(&str, DataType)] = &[("CLK", DataType::Bool)];
const EDGE_OUTPUTS: &[(&str, DataType)] = &[("Q", DataType::Bool)];
const CTUD_INPUTS: &[(&str, DataType)] = &[
    ("CU", DataType::Bool),
    ("CD", DataType::Bool),
    ("RESET", DataType::Bool),
    ("LOAD", DataType::Bool),
    ("PV", DataType::Int),
];
const CTUD_OUTPUTS: &[(&str, DataType)] = &[("QU", DataType::Bool), ("QD", DataType::Bool), ("CV", DataType::Int)];
const SR_INPUTS: &[(&str, DataType)] = &[("SET1", DataType::Bool), ("RESET", DataType::Bool)];
const RS_INPUTS: &[(&str, DataType)] = &[("SET", DataType::Bool), ("RESET1", DataType::Bool)];
const BISTABLE_OUTPUTS: &[(&str, DataType)] = &[("Q1", DataType::Bool)];

impl FbKind {
    pub const ALL: [FbKind; 10] = [
        FbKind::Ton,
        FbKind::Tof,
        FbKind::Tp,
        FbKind::Ctu,
        FbKind::Ctd,
        FbKind::Ctud,
        FbKind::RTrig,
        FbKind::FTrig,
        FbKind::Sr,
        FbKind::Rs,
    ];

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
            "CTUD" => Some(FbKind::Ctud),
            "SR" => Some(FbKind::Sr),
            "RS" => Some(FbKind::Rs),
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
            FbKind::Ctud => "CTUD",
            FbKind::Sr => "SR",
            FbKind::Rs => "RS",
        }
    }

    pub fn inputs(self) -> &'static [(&'static str, DataType)] {
        match self {
            FbKind::Ton | FbKind::Tof | FbKind::Tp => TIMER_INPUTS,
            FbKind::Ctu => CTU_INPUTS,
            FbKind::Ctd => CTD_INPUTS,
            FbKind::RTrig | FbKind::FTrig => EDGE_INPUTS,
            FbKind::Ctud => CTUD_INPUTS,
            FbKind::Sr => SR_INPUTS,
            FbKind::Rs => RS_INPUTS,
        }
    }

    pub fn outputs(self) -> &'static [(&'static str, DataType)] {
        match self {
            FbKind::Ton | FbKind::Tof | FbKind::Tp => TIMER_OUTPUTS,
            FbKind::Ctu => CTU_OUTPUTS,
            FbKind::Ctd => CTD_OUTPUTS,
            FbKind::RTrig | FbKind::FTrig => EDGE_OUTPUTS,
            FbKind::Ctud => CTUD_OUTPUTS,
            FbKind::Sr | FbKind::Rs => BISTABLE_OUTPUTS,
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

