//! Incremental mature-heap conformance on top of the moving nursery.

mod adapter;
mod allocation;
mod barrier;
mod coordination;
mod heap;
mod major;
mod memory;
mod workers;

pub use allocation::{
    AllocationInfrastructureConfig, AllocationInfrastructureError, AllocationMetrics,
    AllocationPlacement, HeapDomain, PageDescriptor, PageId, RegionId,
};
pub use coordination::{
    CollectorEpoch, CollectorPhase, EpochCoordinator, EpochCoordinatorConfig,
    EpochCoordinatorConfigError, EpochCoordinatorError, EpochCoordinatorTelemetry, EpochProgress,
    MutatorExecutionState, MutatorId, MutatorPublication,
};
pub use heap::{GenerationalRuntime, MajorCollectorConfig, MajorCyclePhase};
pub use memory::{
    GenerationalMemoryConfig, GenerationalMemoryConfigError, GenerationalMemoryTelemetry,
    NonHeapMemoryUsage, NonHeapMemoryUsageError,
};
pub use workers::{
    BackgroundWorkerConfig, BackgroundWorkerConfigError, BackgroundWorkerStartError,
    BackgroundWorkerTelemetry,
};
