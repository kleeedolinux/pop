//! Page-described allocation and scheduler-local pointer-bump TLAB state.

mod model;
mod region;
mod state;

pub use model::{
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, HeapDomain, PageDescriptor, PageId, RegionId,
};
pub(crate) use region::{RegionKey, RegionRecord};
pub use region::{RegionState, RegionTelemetry};
pub(crate) use state::AllocationInfrastructure;
