//! LLVM-only lowering and native artifact emission boundary.
//!
//! This backend owns only target/private lowering and artifact emission. Its
//! modules separate public artifact API, MIR analysis, private LLVM-shaped IR,
//! function lowering, and instruction lowering so backend mechanics cannot
//! become canonical HIR/MIR semantics.

mod api;
mod instruction_lowering;
mod lowering;
mod module_lowering;

pub use api::*;
pub use lowering::*;
