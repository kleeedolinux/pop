//! Incremental mature-heap conformance on top of the moving nursery.

mod access;
mod adapter;
mod allocation;
mod arena;
mod barrier;
mod coordination;
mod evacuation;
mod ffi_bytes;
mod heap;
mod major;
mod memory;
mod ownership;
mod parallel;
mod pinning;
mod pretenuring;
mod stable;
mod task_roots;
mod workers;

pub use allocation::{
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, DirectPageAccess, DirectReferenceStoreAccess, DirectReferenceValidation,
    HeapDomain, PageDescriptor, PageId, PendingMatureObject, RegionId, RegionState,
    RegionTelemetry, ReservedMatureLease, ReservedMatureObject,
};
pub(crate) use allocation::{ReservedMatureIdentity, ReservedMaturePublication};
pub use coordination::{
    ActiveStackWatermark, CollectorEpoch, CollectorPhase, EpochCoordinator, EpochCoordinatorConfig,
    EpochCoordinatorConfigError, EpochCoordinatorError, EpochCoordinatorTelemetry, EpochProgress,
    MajorCollectionHandshakeError, MutatorExecutionState, MutatorId, MutatorPublication,
};
pub use evacuation::{
    EvacuationCandidate, EvacuationSelectionConfig, EvacuationSelectionConfigError,
    EvacuationStatistics,
};
pub use heap::{
    GenerationalRuntime, MajorCollectionTelemetry, MajorCollectorConfig, MajorCyclePhase,
};
pub use memory::{
    GenerationalMemoryConfig, GenerationalMemoryConfigError, GenerationalMemoryTelemetry,
    NonHeapMemoryUsage, NonHeapMemoryUsageError,
};
pub use parallel::{
    ParallelSchedulerLocalConfigError, ParallelSchedulerLocalRuntime,
    ParallelSchedulerLocalTelemetry, SchedulerLocalContext,
};
pub use pinning::{PinningConfig, PinningTelemetry};
pub use pretenuring::AdaptivePretenuringConfig;
pub use stable::StableGenerationalRuntime;
pub use task_roots::{
    TaskFrameRootConfig, TaskFrameRootConfigError, TaskFrameRootError, TaskFrameRootTelemetry,
};
pub use workers::{
    BackgroundWorkerConfig, BackgroundWorkerConfigError, BackgroundWorkerStartError,
    BackgroundWorkerTelemetry,
};
