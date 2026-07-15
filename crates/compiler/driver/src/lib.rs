//! Build orchestration owned by the unified Pop Lang driver.
//!
//! Driver code coordinates existing compiler contracts; it does not own syntax,
//! resolution, typing, compile-time, HIR, MIR, or backend semantics. The module
//! map keeps those phase transitions visible to contributors:
//!
//! - public API types describe immutable analysis inputs and results;
//! - front-end orchestration orders parse, index, resolve, type, compile-time,
//!   and HIR publication;
//! - reference metadata projects only accepted public Bubble contracts;
//! - attribute and compile-time helpers remain isolated phase mechanics;
//! - diagnostic helpers provide deterministic structured reporting.

// The driver aggregates long phase-orchestration routines that predate the
// Rust 1.96 clippy gate. Keep the baseline explicit until those modules are
// split deliberately.
#![allow(
    clippy::cast_possible_truncation,
    clippy::collapsible_if,
    clippy::format_collect,
    clippy::items_after_test_module,
    clippy::match_same_arms,
    clippy::redundant_closure_for_method_calls,
    clippy::similar_names,
    clippy::too_many_lines,
    clippy::wildcard_imports
)]

mod api;
mod artifact;
mod attributes;
mod compile_time;
mod front_end;
mod reference;
mod work;

pub use api::*;
pub use artifact::*;
pub use front_end::*;
