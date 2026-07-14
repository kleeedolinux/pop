//! Incremental mature-heap conformance on top of the moving nursery.

mod adapter;
mod barrier;
mod heap;
mod major;

pub use heap::{GenerationalRuntime, MajorCollectorConfig, MajorCyclePhase};
