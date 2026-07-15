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
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, BackgroundWorkerConfig, BackgroundWorkerConfigError,
    BackgroundWorkerStartError, BackgroundWorkerTelemetry, CollectorEpoch, CollectorPhase,
    EpochCoordinator, EpochCoordinatorConfig, EpochCoordinatorConfigError, EpochCoordinatorError,
    EpochCoordinatorTelemetry, EpochProgress, EvacuationCandidate, EvacuationSelectionConfig,
    EvacuationSelectionConfigError, EvacuationStatistics, GenerationalMemoryConfig,
    GenerationalMemoryConfigError, GenerationalMemoryTelemetry, GenerationalRuntime, HeapDomain,
    MajorCollectionHandshakeError, MajorCollectionTelemetry, MajorCollectorConfig, MajorCyclePhase,
    MutatorExecutionState, MutatorId, MutatorPublication, NonHeapMemoryUsage,
    NonHeapMemoryUsageError, PageDescriptor, PageId, PinningConfig, PinningTelemetry, RegionId,
    RegionState, RegionTelemetry, StableGenerationalRuntime, TaskFrameRootConfig,
    TaskFrameRootConfigError, TaskFrameRootError, TaskFrameRootTelemetry,
};
pub use heap::{BootstrapRuntime, CollectorMetrics, HeapLimits};
pub use ownership::{
    IsolatedRegionId, IsolationStatistics, IsolationTelemetry, ObjectOwnership,
    PublicationStatistics,
};
pub use relocation::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
