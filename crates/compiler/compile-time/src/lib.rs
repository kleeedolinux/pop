//! Restricted typed compile-time HIR, canonical values, and deterministic evaluation.
//!
//! Contributor ownership follows the compile-time phase boundary:
//!
//! - [`model`] defines immutable values, typed expressions, dependencies, and
//!   deterministic result contracts;
//! - lowering accepts only the restricted typed-HIR subset;
//! - program verification rejects invalid handles, effects, and value shapes;
//! - interpretation enforces deterministic resource budgets and provenance.
//!
//! None of these modules may access ambient I/O, parse source, or invoke a
//! backend. Keeping that isolation visible is part of the language contract.

mod evaluation;
mod interpreter;
mod lowering;
mod model;
mod program;

pub use evaluation::*;
pub use interpreter::CompileTimeInterpreter;
pub use lowering::{lower_compile_time_expression, lower_compile_time_function};
pub use model::*;
pub use program::*;
