//! Page-described allocation and scheduler-local pointer-bump TLAB state.

mod buffer;
mod direct;
mod model;
mod region;
mod state;

pub use buffer::{PendingMatureObject, ReservedMatureLease, ReservedMatureObject};
pub(crate) use buffer::{ReservedMatureIdentity, ReservedMaturePublication};
pub(crate) use direct::DirectAccessState;
pub(crate) use direct::DirectReferenceLease;
pub use direct::{DirectPageAccess, DirectReferenceStoreAccess, DirectReferenceValidation};
pub use model::{
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, HeapDomain, PageDescriptor, PageId, RegionId,
};
pub(crate) use region::{RegionKey, RegionRecord};
pub use region::{RegionState, RegionTelemetry};
pub(crate) use state::AllocationInfrastructure;
