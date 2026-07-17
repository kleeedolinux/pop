//! Single-mutator moving-nursery conformance collector.

mod access;
mod adapter;
mod allocation;
mod cards;
mod collect;
mod heap;
pub(crate) mod table;

pub(crate) use heap::RelocationAllocation;
pub use heap::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
