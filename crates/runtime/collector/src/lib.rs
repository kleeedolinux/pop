//! Portable collector implementations of the PLRI garbage-collection contract.

mod access;
mod adapter;
mod heap;
mod relocation;
mod trace;

pub use heap::{BootstrapRuntime, CollectorMetrics, HeapLimits};
pub use relocation::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
