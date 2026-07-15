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

// MIR owns large lowering, optimization, rendering, and verification passes
// that predate the Rust 1.96 clippy gate. Keep the baseline explicit until
// those passes are split deliberately.
#![allow(
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::redundant_closure_for_method_calls,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

mod ffi_layout;
mod ir;
mod lowering;
mod optimize;
mod render;
mod text;
mod verification;

pub use ffi_layout::*;
pub use ir::*;
pub(crate) use lowering::local_instruction_effects;
pub use lowering::{is_managed_reference_type_id, lower_hir_bubble, lower_hir_bubble_for_target};
pub use optimize::optimize_mir;
pub use text::{MirParseError, parse_mir_dump};
pub use verification::verify_mir_bubble;
pub(crate) use verification::{block_targets, instruction_operands};
