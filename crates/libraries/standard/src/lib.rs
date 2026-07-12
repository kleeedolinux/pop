//! Rust implementation foundation for the public `Pop.Standard` Bubble.
//!
//! These APIs are intentionally small, typed, and function-first. They are
//! implementation adapters for the public Pop contracts, not a second source
//! language or a universal object layer.

pub mod math;
mod native_output;
pub mod sequence;
pub mod text;

pub use native_output::{NATIVE_EXPORTS, pop_std_print_int, pop_std_print_string, print_string};
