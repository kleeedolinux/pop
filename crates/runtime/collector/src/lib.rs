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
    AllocationPlacement, BackgroundWorkerConfig, BackgroundWorkerConfigError,
    BackgroundWorkerStartError, BackgroundWorkerTelemetry, CollectorEpoch, CollectorPhase,
    EpochCoordinator, EpochCoordinatorConfig, EpochCoordinatorConfigError, EpochCoordinatorError,
    EpochCoordinatorTelemetry, EpochProgress, GenerationalMemoryConfig,
    GenerationalMemoryConfigError, GenerationalMemoryTelemetry, GenerationalRuntime, HeapDomain,
    MajorCollectorConfig, MajorCyclePhase, MutatorExecutionState, MutatorId, MutatorPublication,
    NonHeapMemoryUsage, NonHeapMemoryUsageError, PageDescriptor, PageId, RegionId,
};
pub use heap::{BootstrapRuntime, CollectorMetrics, HeapLimits};
pub use relocation::{CollectorGeneration, CollectorObjectId, RelocationRuntime};
