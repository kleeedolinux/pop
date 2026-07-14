//! Persistent bounded host-worker execution for mature collection jobs.

mod model;
mod state;

pub use model::{
    BackgroundWorkerConfig, BackgroundWorkerConfigError, BackgroundWorkerStartError,
    BackgroundWorkerTelemetry,
};
pub(crate) use state::{BackgroundWorkerPool, CardRefinementTask, MarkTask};
