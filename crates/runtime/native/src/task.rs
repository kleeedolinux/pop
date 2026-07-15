//! Native task and cancellation ABI boundary.

// Focused implementation slices share one private module so scheduler
// transition ownership stays internal without widening the PLRI or native ABI.
include!("task/implementation.rs");
