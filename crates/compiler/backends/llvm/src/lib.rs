//! LLVM-only lowering and native artifact emission boundary.
//!
//! This backend owns only target/private lowering and artifact emission. Its
//! modules separate public artifact API, MIR analysis, private LLVM-shaped IR,
//! function lowering, and instruction lowering so backend mechanics cannot
//! become canonical HIR/MIR semantics.

// The LLVM backend contains large lowering/emission passes that predate the
// Rust 1.96 clippy gate. Keep the baseline explicit until those passes are
// split deliberately.
#![allow(
    clippy::cast_possible_truncation,
    clippy::comparison_chain,
    clippy::format_push_string,
    clippy::match_same_arms,
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

mod api;
mod async_lowering;
mod bpf;
mod ffi_buffer;
mod ffi_unsafe;
mod instruction_lowering;
mod lowering;
mod module_lowering;

pub use api::*;
pub use bpf::*;
pub use lowering::*;
