//! Page-described allocation and scheduler-local pointer-bump TLAB state.

mod model;
mod state;

pub use model::{
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, HeapDomain, PageDescriptor, PageId, RegionId,
};
pub(crate) use state::AllocationInfrastructure;
