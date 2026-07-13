//! Canonical backend-neutral control-flow IR and portable verification.
//!
//! The crate root is an ownership map rather than an implementation file:
//!
//! - [`ir`] defines MIR values, blocks, instructions, and portable contracts;
//! - lowering makes HIR evaluation order and effects explicit;
//! - verification proves CFG, type, failure, and GC invariants;
//! - optimization transforms only verified portable MIR;
//! - text parses and prints deterministic tooling fixtures.
//!
//! Backend-specific representations and target instructions do not belong in
//! any of these modules.

mod ir;
mod lowering;
mod optimize;
mod render;
mod text;
mod verification;

pub use ir::*;
pub(crate) use lowering::local_instruction_effects;
pub use lowering::lower_hir_bubble;
pub use optimize::optimize_mir;
pub use text::{MirParseError, parse_mir_dump};
pub use verification::verify_mir_bubble;
pub(crate) use verification::{block_targets, instruction_operands};
