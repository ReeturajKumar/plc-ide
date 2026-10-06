//! IEC 61131-3 standard function blocks.
//!
//! A block is described by an interface (`FbKind`: its input and output members, used by
//! the checker) and implemented by a behavior type in its own module (`ton`, `tof`, `tp`,
//! `ctu`). An instance (`FbInstance`) owns the current value of every member (`Io`) plus
//! the behavior's private state, and all of it persists between scans. Inputs left out of
//! a call keep their previous value, as IEC specifies.

mod ctd;
mod ctu;
mod edge;
mod reference;
mod tof;
mod ton;
mod tp;

pub use reference::reference_source;

use super::ast::DataType;
use super::value::Value;

/// PLC time for one scan. Blocks run on scan time, not wall-clock time: the scan loop
/// advances `now_ms` by `cycle_ms` per scan, so timing is exact and independent of how
/// long a scan really took.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanClock {
    /// PLC time at the end of this scan.
    pub now_ms: i64,
    /// Length of one scan.
    pub cycle_ms: i64,
}

impl ScanClock {
    /// An input change seen during a scan is taken to have happened at the scan's start,
    /// so a timer started in scan 1 of a 100 ms cycle already reads ET = 100 ms.
    pub fn scan_start(self) -> i64 {
        self.now_ms - self.cycle_ms
    }
}

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
            FbKind::Ctu => ctu::INPUTS,
            FbKind::Ctd => ctd::INPUTS,
            FbKind::RTrig | FbKind::FTrig => edge::INPUTS,
        }
    }

    pub fn outputs(self) -> &'static [(&'static str, DataType)] {
        match self {
            FbKind::Ton | FbKind::Tof | FbKind::Tp => TIMER_OUTPUTS,
            FbKind::Ctu => ctu::OUTPUTS,
            FbKind::Ctd => ctd::OUTPUTS,
            FbKind::RTrig | FbKind::FTrig => edge::OUTPUTS,
        }
    }

    /// Inputs then outputs, the order used for storage and the monitor.
    pub fn members(self) -> impl Iterator<Item = (&'static str, DataType)> {
        self.inputs().iter().chain(self.outputs()).copied()
    }

    pub fn input_type(self, name: &str) -> Option<DataType> {
        lookup(self.inputs(), name)
    }

    #[cfg(test)]
    pub fn member_type(self, name: &str) -> Option<DataType> {
        self.members().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, t)| t)
    }

    pub fn is_output(self, name: &str) -> bool {
        lookup(self.outputs(), name).is_some()
    }

    #[cfg(test)]
    pub fn list(members: &[(&str, DataType)]) -> String {
        members.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")
    }

    pub fn instantiate(self) -> FbInstance {
        let state = match self {
            FbKind::Ton => State::Ton(ton::Ton::default()),
            FbKind::Tof => State::Tof(tof::Tof::default()),
            FbKind::Tp => State::Tp(tp::Tp::default()),
            FbKind::Ctu => State::Ctu(ctu::Ctu::default()),
            FbKind::Ctd => State::Ctd(ctd::Ctd::default()),
            FbKind::RTrig => State::RTrig(edge::RTrig::default()),
            FbKind::FTrig => State::FTrig(edge::FTrig::default()),
        };
        FbInstance { io: Io::new(self), state }
    }
}

fn lookup(members: &[(&str, DataType)], name: &str) -> Option<DataType> {
    members.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, t)| *t)
}

/// Current value of every member of one instance, in `FbKind::members()` order.
#[derive(Debug, Clone, PartialEq)]
pub struct Io {
    kind: FbKind,
    values: Vec<Value>,
}

impl Io {
    fn new(kind: FbKind) -> Self {
        Io { kind, values: kind.members().map(|(_, t)| Value::default_for(t)).collect() }
    }

    fn index(&self, name: &str) -> Option<usize> {
        self.kind.members().position(|(n, _)| n.eq_ignore_ascii_case(name))
    }

    pub fn get(&self, name: &str) -> Option<Value> {
        self.index(name).map(|i| self.values[i])
    }

    /// Write a member, keeping its declared type.
    fn set(&mut self, name: &str, value: Value) -> Result<(), String> {
        let i = self.index(name).ok_or_else(|| format!("{} has no member '{name}'", self.kind.name()))?;
        if self.values[i].data_type() != value.data_type() {
            return Err(format!(
                "{}.{name} is {}, not {}",
                self.kind.name(),
                self.values[i].data_type().name(),
                value.data_type().name()
            ));
        }
        self.values[i] = value;
        Ok(())
    }

    // Typed access for the block implementations, whose member names and types are fixed
    // by their own interface (and covered by their tests).
    fn bool(&self, name: &str) -> bool {
        self.get(name).and_then(Value::as_bool).unwrap_or(false)
    }

    fn time(&self, name: &str) -> i64 {
        match self.get(name) {
            Some(Value::Time(ms)) => ms,
            _ => 0,
        }
    }

    fn int(&self, name: &str) -> i16 {
        match self.get(name) {
            Some(Value::Int(n)) => n,
            _ => 0,
        }
    }

    fn put(&mut self, name: &str, value: Value) {
        let _ = self.set(name, value);
    }
}

/// Each block's private state, beyond its visible members.
#[derive(Debug, Clone, PartialEq)]
enum State {
    Ton(ton::Ton),
    Tof(tof::Tof),
    Tp(tp::Tp),
    Ctu(ctu::Ctu),
    Ctd(ctd::Ctd),
    RTrig(edge::RTrig),
    FTrig(edge::FTrig),
}

/// One declared instance, e.g. `RunTimer : TON;`.
#[derive(Debug, Clone, PartialEq)]
pub struct FbInstance {
    io: Io,
    state: State,
}

impl FbInstance {
    pub fn kind(&self) -> FbKind {
        self.io.kind
    }

    pub fn get(&self, member: &str) -> Option<Value> {
        self.io.get(member)
    }

    /// Set an input before a call. The checker guarantees name and type; this still
    /// refuses outputs and mismatches rather than trusting that.
    pub fn set_input(&mut self, name: &str, value: Value) -> Result<(), String> {
        if self.kind().input_type(name).is_none() {
            return Err(format!("{} has no input '{name}'", self.kind().name()));
        }
        self.io.set(name, value)
    }

    /// Run the block once with its current inputs.
    pub fn execute(&mut self, clock: ScanClock) {
        match &mut self.state {
            State::Ton(ton) => ton.execute(&mut self.io, clock),
            State::Tof(tof) => tof.execute(&mut self.io, clock),
            State::Tp(tp) => tp.execute(&mut self.io, clock),
            State::Ctu(ctu) => ctu.execute(&mut self.io),
            State::Ctd(ctd) => ctd.execute(&mut self.io),
            State::RTrig(trig) => trig.execute(&mut self.io),
            State::FTrig(trig) => trig.execute(&mut self.io),
        }
    }
}

#[cfg(test)]
mod test_support {
    use super::*;

    /// Set `inputs`, then run the block once (for blocks that don't use time).
    pub fn call(fb: &mut FbInstance, inputs: &[(&str, Value)]) {
        for (name, value) in inputs {
            fb.set_input(name, *value).unwrap();
        }
        fb.execute(ScanClock { now_ms: 0, cycle_ms: 0 });
    }

    /// Call `fb` once per 100 ms scan, returning (Q, ET) after each.
    pub struct Scans {
        pub fb: FbInstance,
        pub now_ms: i64,
    }

    impl Scans {
        pub const CYCLE_MS: i64 = 100;

        pub fn new(kind: FbKind, preset_ms: i64) -> Self {
            let mut fb = kind.instantiate();
            fb.set_input("PT", Value::Time(preset_ms)).unwrap();
            Scans { fb, now_ms: 0 }
        }

        pub fn scan(&mut self, input: bool) -> (bool, i64) {
            self.now_ms += Self::CYCLE_MS;
            self.fb.set_input("IN", Value::Bool(input)).unwrap();
            self.fb.execute(ScanClock { now_ms: self.now_ms, cycle_ms: Self::CYCLE_MS });
            let q = self.fb.get("Q").and_then(Value::as_bool).unwrap();
            let et = match self.fb.get("ET") {
                Some(Value::Time(ms)) => ms,
                other => panic!("ET is {other:?}"),
            };
            (q, et)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn describes_interfaces() {
        assert_eq!(FbKind::from_name("tof"), Some(FbKind::Tof));
        assert_eq!(FbKind::from_name("Tp"), Some(FbKind::Tp));
        assert_eq!(FbKind::from_name("ctd"), Some(FbKind::Ctd));
        assert_eq!(FbKind::from_name("r_trig"), Some(FbKind::RTrig));
        assert_eq!(FbKind::from_name("CTUD"), None, "not implemented");
        assert_eq!(FbKind::list(FbKind::Ctd.inputs()), "CD, LD, PV");
        assert_eq!(FbKind::list(FbKind::FTrig.inputs()), "CLK");
        assert_eq!(FbKind::Ton.input_type("pt"), Some(DataType::Time));
        assert_eq!(FbKind::Ton.input_type("Q"), None, "Q is an output");
        assert!(FbKind::Tp.is_output("et"));
        assert!(!FbKind::Tp.is_output("IN"));
        assert_eq!(FbKind::Ctu.member_type("cv"), Some(DataType::Int));
        assert_eq!(FbKind::list(FbKind::Tof.inputs()), "IN, PT");
    }

    #[test]
    fn instances_refuse_outputs_and_wrong_types() {
        let mut fb = FbKind::Ton.instantiate();
        assert!(fb.set_input("Q", Value::Bool(true)).is_err());
        assert!(fb.set_input("PT", Value::Int(10)).is_err());
        assert!(fb.set_input("pt", Value::Time(10)).is_ok(), "names are case-insensitive");
        assert_eq!(fb.get("PT"), Some(Value::Time(10)));
    }
}
