//! Structured Text pipeline: source → lexer → parser → AST → checker (semantic analysis:
//! symbol table, names, types; all errors collected) → runtime. A program with any
//! error never reaches the runtime.
//! ST is only ever interpreted here; nothing is compiled to or executed as native/JS code.

pub mod ast;
pub mod checker;
pub mod error;
pub mod function_blocks;
pub mod functions;
pub mod lexer;
pub mod parser;
pub mod runtime;
pub mod symbols;
pub mod value;
