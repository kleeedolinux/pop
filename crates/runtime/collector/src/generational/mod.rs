//! Incremental mature-heap conformance on top of the moving nursery.

mod adapter;
mod allocation;
mod barrier;
mod heap;
mod major;

pub use allocation::{
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, HeapDomain, PageDescriptor, PageId, RegionId,
};
pub use heap::{GenerationalRuntime, MajorCollectorConfig, MajorCyclePhase};
