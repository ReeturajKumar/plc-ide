//! Simulated PLC: runs a validated ST program in a fixed-interval scan cycle.

#[allow(clippy::module_inception)] // file layout is part of the project spec
pub mod simulator;
pub mod state;

pub use simulator::{compile, standard_function_blocks, Breakpoint, CompileReport, FbSummary, ProgramSource, Simulator};
