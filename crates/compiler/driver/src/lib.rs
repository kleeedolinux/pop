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

mod api;
mod attributes;
mod compile_time;
mod front_end;
mod reference;
mod work;

pub use api::*;
pub use front_end::*;
