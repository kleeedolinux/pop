//! Typed mutator registration and bounded collector-epoch handshakes.

mod model;
mod runtime;
mod state;

pub use model::{
    CollectorEpoch, CollectorPhase, EpochCoordinatorConfig, EpochCoordinatorConfigError,
    EpochCoordinatorError, EpochCoordinatorTelemetry, EpochProgress, MajorCollectionHandshakeError,
    MutatorExecutionState, MutatorId, MutatorPublication,
};
pub use state::EpochCoordinator;
