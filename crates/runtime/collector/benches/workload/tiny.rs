use pop_runtime_collector::{BootstrapRuntime, HeapLimits};
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, RuntimeAdapter, RuntimeFailure,
    RuntimeTypeId,
};

use super::model::{WorkloadConfiguration, WorkloadCounters, WorkloadState, collect, empty_roots};

/// Executes deterministic batches of unreachable scalar-object allocation and
/// explicit Stage-1 collection.
///
/// # Errors
///
/// Returns a portable runtime failure for an invalid/overflowing configuration
/// or failed allocation/collection.
pub fn run_tiny_object_churn(
    configuration: WorkloadConfiguration,
) -> Result<WorkloadCounters, RuntimeFailure> {
    let configuration = configuration.validate()?;
    let batch_objects = usize::try_from(configuration.items_per_batch)
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    let slots_per_object = usize::try_from(configuration.slots_per_object)
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    let maximum_objects = batch_objects
        .checked_add(1)
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    let maximum_slots = maximum_objects
        .checked_mul(slots_per_object)
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    let mut runtime =
        BootstrapRuntime::with_limits(HeapLimits::new(maximum_objects, maximum_slots));
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(1),
        AllocationClass::NurseryEligible,
        ObjectMap::new(configuration.slots_per_object, Vec::new())
            .map_err(|_| RuntimeFailure::runtime_invariant())?,
    );
    let mut roots = empty_roots()?;
    let mut state = WorkloadState::default();

    for _ in 0..configuration.batches {
        for _ in 0..configuration.items_per_batch {
            runtime.allocate_object(&request)?;
        }
        state.observe(&runtime);
        collect(&mut runtime, &mut roots)?;
    }

    Ok(state.finish(&runtime, "tiny_object_churn", "isolated_scalar_objects", 0))
}
