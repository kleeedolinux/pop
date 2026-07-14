//! Rust implementation foundation for the public `Pop.Standard` Bubble.
//!
//! These APIs are intentionally small, typed, and function-first. They are
//! implementation adapters for the public Pop contracts, not a second source
//! language or a universal object layer.

mod baseline;
mod native_output;
pub mod text;

pub use baseline::{
    ApiBaselineError, ApiKind, ApiStatus, ApiTier, StandardApiBaseline, StandardApiEntry,
    parse_standard_api_baseline, standard_api_baseline,
};
pub use native_output::{NATIVE_EXPORTS, pop_std_print_int, pop_std_print_string, print_string};
