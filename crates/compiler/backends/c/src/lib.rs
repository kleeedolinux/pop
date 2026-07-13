//! Experimental verified MIR-to-C11 backend boundary.
//!
//! Module ownership is intentionally narrow:
//! - `api` defines options, artifacts, and structured failures;
//! - `lowering` owns verification and phase order;
//! - `validation` defines the explicitly supported runtime-free MIR subset;
//! - `emission` renders validated MIR as deterministic ISO C11.
//!
//! Extend the supported subset in validation and conformance tests before adding
//! emission. The C backend must never bypass canonical MIR or invent fallbacks.

mod api;
mod emission;
mod lowering;
mod validation;

pub use api::*;
pub use lowering::*;
