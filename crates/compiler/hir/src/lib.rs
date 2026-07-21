//! Typed, resolved, backend-neutral high-level IR.
//!
//! Ownership is intentionally visible at the crate root:
//!
//! - [`ir`] defines the typed HIR data model and its invariants;
//! - lowering translates accepted typed bodies into that model;
//! - verification rejects malformed HIR before MIR construction;
//! - text rendering provides deterministic debug and test output.
//!
//! Keeping these concerns separate prevents source lowering, validation, and
//! presentation mechanics from growing back into one contributor-hostile file.

// HIR owns large data-model, lowering, verification, and dump routines that
// predate the Rust 1.96 clippy gate. Keep the baseline explicit until those
// modules are split deliberately.
#![allow(
    clippy::assigning_clones,
    clippy::double_must_use,
    clippy::match_same_arms,
    clippy::semicolon_if_nothing_returned,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::wildcard_imports,
    clippy::write_with_newline
)]

mod effects;
mod ir;
mod lowering;
mod text;
mod verification;

pub use effects::*;
pub use ir::*;
pub use lowering::*;
pub use verification::*;

#[cfg(test)]
mod tests;
