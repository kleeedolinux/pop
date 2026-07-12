use pop_runtime_collector::{BootstrapRuntime, HeapLimits};
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, RuntimeAdapter, RuntimeFailure,
    RuntimeTypeId,
};

use super::model::{WorkloadConfiguration, WorkloadCounters, WorkloadState, collect, empty_roots};

/// Allocates beyond a small heap limit so capacity checks trigger real automatic
/// collections, followed by one explicit final reclamation.
///
/// # Errors
///
/// Returns a portable runtime failure for invalid/overflowing configuration or
/// failed allocation/collection.
pub fn run_allocation_pressure(
    configuration: WorkloadConfiguration,
) -> Result<WorkloadCounters, RuntimeFailure> {
    let configuration = configuration.validate()?;
    let maximum_objects = usize::try_from(configuration.pressure_limit)
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    let maximum_slots = maximum_objects
        .checked_mul(
            usize::try_from(configuration.slots_per_object)
                .map_err(|_| RuntimeFailure::runtime_invariant())?,
        )
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(6),
        AllocationClass::NurseryEligible,
        ObjectMap::new(configuration.slots_per_object, Vec::new())
            .map_err(|_| RuntimeFailure::runtime_invariant())?,
    );
    let mut roots = empty_roots()?;
    let mut runtime =
        BootstrapRuntime::with_limits(HeapLimits::new(maximum_objects, maximum_slots));
    let mut state = WorkloadState::default();

    for _ in 0..configuration.batches {
        for _ in 0..configuration.items_per_batch {
            runtime.allocate_object(&request)?;
            state.observe(&runtime);
        }
    }
    collect(&mut runtime, &mut roots)?;

    Ok(state.finish(
        &runtime,
        "allocation_pressure",
        "bounded_unreachable_scalar_objects",
        0,
    ))
}
