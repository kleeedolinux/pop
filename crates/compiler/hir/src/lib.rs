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

mod ir;
mod lowering;
mod text;
mod verification;

pub use ir::*;
pub use lowering::*;
pub use verification::*;

#[cfg(test)]
mod tests;
