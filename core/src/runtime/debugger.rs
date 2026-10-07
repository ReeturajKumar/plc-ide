//! The runtime debugger: breakpoints, stopping a scan before a statement, stepping, and
//! going on. It knows source files and lines (from the AST), never anything about an
//! editor: the host reports `Location` and the UI decides what to show.

use std::collections::HashSet;

use super::executor::{execute_programs, Ctx, Memory, ProgramError};
use super::Runtime;
use crate::compiler::error::{StError, StResult};
use crate::runtime::function_blocks::ScanClock;
use crate::compiler::value::Value;

/// Where the debugger stopped: the statement about to run.
#[derive(Debug, Clone, PartialEq)]
pub struct Location {
    /// The program whose scan is running (the caller, when inside a function block).
    pub program: String,
    /// The source file of the statement: the program's, or the function block's.
    pub file: String,
    pub line: usize,
    pub column: usize,
    /// Set when the statement is inside a user function block.
    pub function_block: Option<String>,
    /// The instance running it, e.g. "Counter1" (nested: "Station1.Blinker").
    pub instance: Option<String>,
}

/// When a debugged scan stops before a statement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// Run the whole scan.
    Never,
    /// At an enabled breakpoint (once per line, so a line with several statements stops once).
    AtBreakpoint,
    /// Before the very next statement (STEP STATEMENT).
    NextStatement,
}

/// A scan stopped at a breakpoint or step, waiting to go on.
///
/// The interpreter isn't resumable mid-statement, so going on replays the scan from its
/// start: a scan is deterministic (same memory, inputs and clock give the same result),
/// so re-running the first `paused_at - 1` statements reproduces exactly the state the
/// user inspected, then execution carries on. PLC scans are short, so this costs little.
pub(super) struct Suspended {
    /// Memory when the scan began (inputs already read).
    start: Memory,
    clock: ScanClock,
    /// Number of the statement it stopped before (1-based, counted across the scan).
    paused_at: u64,
    location: Location,
    /// Values set while stopped, with the statement they were set before; the replay
    /// applies each one when it reaches that statement again.
    edits: Edits,
}

/// (statement number, slot, value): see `Suspended::edits`.
type Edits = Vec<(u64, String, Value)>;

/// The debugger's state: enabled breakpoints and the stopped scan, if any.
#[derive(Default)]
pub(super) struct DebuggerState {
    /// (file id, line).
    pub(super) breakpoints: HashSet<(usize, usize)>,
    pub(super) suspended: Option<Suspended>,
}

/// Debugger bookkeeping for one scan: counts statements and decides where to stop.
pub(super) struct Trace<'a> {
    breakpoints: &'a HashSet<(usize, usize)>,
    stop: Stop,
    /// Statements that run without stopping: those replayed up to and including the one
    /// the scan last stopped at.
    skip: u64,
    pub(super) edits: &'a [(u64, String, Value)],
    pub(super) executed: u64,
    /// (file, line) of the previous statement.
    last: Option<(usize, usize)>,
    /// Where it stopped.
    pub(super) hit: Option<(Ctx, usize, usize)>,
}

impl Trace<'_> {
    /// Called before every statement; an error unwinds the scan to stop there.
    pub(super) fn enter(&mut self, ctx: &Ctx, (line, column): (usize, usize)) -> StResult<()> {
        self.executed += 1;
        let new_line = self.last != Some((ctx.file, line));
        self.last = Some((ctx.file, line));
        let stop = self.executed > self.skip
            && match self.stop {
                Stop::Never => false,
                Stop::NextStatement => true,
                Stop::AtBreakpoint => new_line && self.breakpoints.contains(&(ctx.file, line)),
            };
        if stop {
            self.hit = Some((ctx.clone(), line, column));
            // Not a failure: `finish` sees `hit` and suspends the scan.
            return Err(StError::general("debugger stop"));
        }
        Ok(())
    }
}

impl Runtime {
    /// Execute a scan's programs under the debugger (inputs already read). Returns where it
    /// stopped, or None if the programs completed. A stopped scan is finished with `proceed`.
    pub fn run(&mut self, clock: ScanClock, stop: Stop) -> Result<Option<Location>, ProgramError> {
        self.debugger.suspended = None;
        // Only a scan that may stop needs its starting memory, to replay it.
        let start = if stop == Stop::Never { Memory::default() } else { self.memory.clone() };
        self.finish(start, clock, 0, stop, Edits::new())
    }

    /// Continue the stopped scan from the statement it stopped at (which now runs), until
    /// `stop` or the end of the programs. Nothing is reset: the replay rebuilds the paused state.
    pub fn proceed(&mut self, stop: Stop) -> Result<Option<Location>, ProgramError> {
        let Some(suspended) = self.debugger.suspended.take() else { return Ok(None) };
        self.memory = suspended.start.clone();
        self.finish(suspended.start, suspended.clock, suspended.paused_at, stop, suspended.edits)
    }

    fn finish(&mut self, start: Memory, clock: ScanClock, skip: u64, stop: Stop, edits: Edits) -> Result<Option<Location>, ProgramError> {
        let mut trace = Trace { breakpoints: &self.debugger.breakpoints, stop, skip, edits: &edits, executed: 0, last: None, hit: None };
        let result = execute_programs(&self.units, &mut self.memory, &self.fbs, clock, &mut trace);
        if let Some((ctx, line, column)) = trace.hit.take() {
            let location = Location {
                program: self.units[ctx.program].name.clone(),
                file: self.files[ctx.file].clone(),
                line,
                column,
                function_block: ctx.fb.map(|fb| self.fbs.defs[fb].name.clone()),
                instance: ctx.instance,
            };
            let paused_at = trace.executed;
            self.debugger.suspended = Some(Suspended { start, clock, paused_at, location: location.clone(), edits });
            return Ok(Some(location));
        }
        result.map(|()| None)
    }

    /// Where the debugger stopped the current scan.
    pub fn location(&self) -> Option<&Location> {
        self.debugger.suspended.as_ref().map(|s| &s.location)
    }

    /// Replace the breakpoints: (source file name, line). A breakpoint in a function block's
    /// file stops in whichever instance reaches it. Unknown files are ignored, and so are
    /// lines without a statement (execution never stops there).
    pub fn set_breakpoints(&mut self, breakpoints: &[(String, usize)]) {
        self.debugger.breakpoints = breakpoints
            .iter()
            .filter_map(|(file, line)| Some((self.files.iter().position(|f| f == file)?, *line)))
            .collect();
    }

    /// The enabled breakpoints: (source file name, line), sorted.
    pub fn breakpoints(&self) -> Vec<(String, usize)> {
        let mut list: Vec<_> = self.debugger.breakpoints.iter().map(|&(file, line)| (self.files[file].clone(), line)).collect();
        list.sort();
        list
    }

    pub fn has_breakpoints(&self) -> bool {
        !self.debugger.breakpoints.is_empty()
    }

    /// A value was set from outside while stopped mid-scan: going on replays the scan, so
    /// the edit is redone when the replay reaches this point.
    pub(super) fn record_edit(&mut self, slot: String, value: Value) {
        if let Some(suspended) = self.debugger.suspended.as_mut() {
            suspended.edits.push((suspended.paused_at, slot, value));
        }
    }
}
