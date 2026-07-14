//! Single-mutator moving-nursery conformance collector.

mod adapter;
mod collect;
mod heap;

pub(crate) use heap::RelocationAllocation;
pub use heap::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
