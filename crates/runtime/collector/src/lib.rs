//! Portable collector implementations of the PLRI garbage-collection contract.

mod access;
mod adapter;
mod generational;
mod heap;
mod relocation;
mod table;
mod trace;

pub use generational::{
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, GenerationalRuntime, HeapDomain, MajorCollectorConfig, MajorCyclePhase,
    PageDescriptor, PageId, RegionId,
};
pub use heap::{BootstrapRuntime, CollectorMetrics, HeapLimits};
pub use relocation::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
