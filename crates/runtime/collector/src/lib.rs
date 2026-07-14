//! Portable collector implementations of the PLRI garbage-collection contract.

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
    EvacuationSelectionConfigError, GenerationalMemoryConfig, GenerationalMemoryConfigError,
    GenerationalMemoryTelemetry, GenerationalRuntime, HeapDomain, MajorCollectionTelemetry,
    MajorCollectorConfig, MajorCyclePhase, MutatorExecutionState, MutatorId, MutatorPublication,
    NonHeapMemoryUsage, NonHeapMemoryUsageError, PageDescriptor, PageId, PinningConfig,
    PinningTelemetry, RegionId, RegionState, RegionTelemetry,
};
pub use heap::{BootstrapRuntime, CollectorMetrics, HeapLimits};
pub use ownership::{
    IsolatedRegionId, IsolationStatistics, IsolationTelemetry, ObjectOwnership,
    PublicationStatistics, SchedulerId,
};
pub use relocation::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
