//! Portable collector implementations of the PLRI garbage-collection contract.

pub use pop_runtime_interface::{SchedulerId, TaskFrameRootId};

mod access;
mod adapter;
mod arena;
mod generational;
mod heap;
mod ownership;
mod relocation;
mod table;
mod trace;

pub use arena::{
    ArenaAllocationRequest, ArenaCloseStatistics, ArenaConfig, ArenaConfigError, ArenaId,
    ArenaLayoutError, ArenaReference, ArenaSlotValue, ArenaTelemetry,
};
pub use generational::{
    ActiveStackWatermark, AdaptivePretenuringConfig, AllocationInfrastructureConfig,
    AllocationInfrastructureError, AllocationMetrics, AllocationPlacement, BackgroundWorkerConfig,
    BackgroundWorkerConfigError, BackgroundWorkerStartError, BackgroundWorkerTelemetry,
    CollectorEpoch, CollectorPhase, DirectPageAccess, DirectReferenceStoreAccess,
    DirectReferenceValidation, EpochCoordinator, EpochCoordinatorConfig,
    EpochCoordinatorConfigError, EpochCoordinatorError, EpochCoordinatorTelemetry, EpochProgress,
    EvacuationCandidate, EvacuationSelectionConfig, EvacuationSelectionConfigError,
    EvacuationStatistics, GenerationalMemoryConfig, GenerationalMemoryConfigError,
    GenerationalMemoryTelemetry, GenerationalRuntime, HeapDomain, MajorCollectionHandshakeError,
    MajorCollectionTelemetry, MajorCollectorConfig, MajorCyclePhase, MutatorExecutionState,
    MutatorId, MutatorPublication, NonHeapMemoryUsage, NonHeapMemoryUsageError, PageDescriptor,
    PageId, ParallelSchedulerLocalConfigError, ParallelSchedulerLocalRuntime,
    ParallelSchedulerLocalTelemetry, PendingMatureObject, PinningConfig, PinningTelemetry,
    RegionId, RegionState, RegionTelemetry, ReservedMatureLease, ReservedMatureObject,
    SchedulerLocalContext, StableGenerationalRuntime, TaskFrameRootConfig,
    TaskFrameRootConfigError, TaskFrameRootError, TaskFrameRootTelemetry,
};
pub use heap::{BootstrapRuntime, CollectorMetrics, HeapLimits};
pub use ownership::{
    FreezeStatistics, IsolatedRegionId, IsolationStatistics, IsolationTelemetry, ObjectMutability,
    ObjectOwnership, PublicationStatistics,
};
pub use relocation::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
