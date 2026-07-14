//! Typed mutator registration and bounded collector-epoch handshakes.

mod model;
mod state;

pub use model::{
    CollectorEpoch, CollectorPhase, EpochCoordinatorConfig, EpochCoordinatorConfigError,
    EpochCoordinatorError, EpochCoordinatorTelemetry, EpochProgress, MutatorExecutionState,
    MutatorId, MutatorPublication,
};
pub use state::EpochCoordinator;
