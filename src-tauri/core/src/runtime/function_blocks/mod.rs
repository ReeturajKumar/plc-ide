//! Behaviors of the IEC 61131-3 standard function blocks, one module each (`ton`, `tof`,
//! `tp`, `ctu`, `ctd`, `edge`). An instance (`FbInstance`) owns the current value of every
//! member (`Io`) plus the behavior's private state, and all of it persists between scans.
//! Inputs left out of a call keep their previous value, as IEC specifies. The interfaces
//! (`FbKind`) belong to the compiler.

mod ctd;
mod ctu;
mod edge;
mod tof;
mod ton;
mod tp;

use crate::compiler::function_blocks::FbKind;
use crate::compiler::value::Value;

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

impl FbKind {
    /// A new instance with every member at its default value.
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

