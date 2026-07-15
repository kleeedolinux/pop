//! Reference interpreter for verified canonical MIR.
//!
//! Module ownership is intentionally explicit:
//! - `interpreter` verifies MIR and owns resource-limited control-flow execution;
//! - `evaluation` implements arithmetic and structured-value operations;
//! - `values` separates observable results from backend-private runtime state;
//! - `runtime` supplies the deterministic PLRI adapter used by conformance tests.
//!
//! New MIR operations should remain backend-neutral. Put execution sequencing in
//! `interpreter`, value semantics in `evaluation`, and runtime capabilities behind
//! `RuntimeAdapter`; never reconstruct source semantics or perform string lookup.
#![allow(
    clippy::match_same_arms,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

mod evaluation;
mod ffi_buffer;
mod interpreter;
mod runtime;
mod values;

pub use interpreter::*;
pub use runtime::*;
pub use values::*;
