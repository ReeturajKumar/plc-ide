use serde::{Deserialize, Serialize};

/// What went wrong, for the editor and Problems list. Serialized as its name
/// (e.g. `"TypeMismatch"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    /// The source doesn't follow ST grammar (from the lexer/parser).
    Syntax,
    UndeclaredVariable,
    DuplicateDeclaration,
    TypeMismatch,
    /// Assigning to something that can't be assigned (an FB instance, a non-constant
    /// initial value, …).
    InvalidAssignment,
    InvalidOperator,
    InvalidCondition,
    /// Unknown function, or a function used like a statement.
    InvalidFunction,
    InvalidFunctionArguments,
    /// Misuse of a function block instance or an unknown member.
    InvalidFunctionBlock,
    InvalidFunctionBlockInput,
    /// Writing to a read-only FB output.
    InvalidFunctionBlockOutput,
    InvalidTimeValue,
    InvalidCase,
    InvalidLoop,
    /// An I/O mapping that doesn't fit the program (unknown variable, wrong type, …).
    InvalidIoMapping,
    /// Raised while the program runs (division by zero, overflow, loop limit, …).
    Runtime,
    /// Not about the source (e.g. "the program is not running").
    General,
}

/// Structured error sent to the frontend. User-facing ST problems are always
/// reported through this type, never as a panic message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StError {
    pub kind: ErrorKind,
    /// 1-based source line; 0 when the error is not tied to a source position.
    pub line: usize,
    pub column: usize,
    pub message: String,
}

impl StError {
    pub fn new(kind: ErrorKind, line: usize, column: usize, message: impl Into<String>) -> Self {
        Self { kind, line, column, message: message.into() }
    }

    /// An error with no source position (e.g. "program is not running").
    pub fn general(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::General, 0, 0, message)
    }
}

pub type StResult<T> = Result<T, StError>;

/// Everything wrong with a program, in source order. Non-empty when returned as an error.
pub type StErrors = Vec<StError>;
