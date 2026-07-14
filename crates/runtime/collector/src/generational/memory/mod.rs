//! Hard-limit admission, adaptive pacing, protected reserves, and telemetry.

mod model;
mod state;

pub use model::{
    GenerationalMemoryConfig, GenerationalMemoryConfigError, GenerationalMemoryTelemetry,
    NonHeapMemoryUsage, NonHeapMemoryUsageError,
};
pub(crate) use state::MemoryController;
