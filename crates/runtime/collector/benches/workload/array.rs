use pop_runtime_collector::BootstrapRuntime;
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ObjectAllocationRequest, ObjectMap,
    ObjectSlot, RuntimeAdapter, RuntimeFailure, RuntimeTypeId,
};

use super::model::{WorkloadConfiguration, WorkloadCounters, WorkloadState, collect, empty_roots};

/// Allocates rooted managed-reference arrays and their child objects, traces the
/// precise elements, then reclaims the complete graph.
///
/// # Errors
///
/// Returns a portable runtime failure for invalid configuration or any
/// allocation, element store, root transition, or collection failure.
pub fn run_managed_array(
    configuration: WorkloadConfiguration,
) -> Result<WorkloadCounters, RuntimeFailure> {
    let configuration = configuration.validate()?;
    let array_request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(3),
        AllocationClass::NurseryEligible,
        configuration.items_per_batch,
        ArrayElementMap::ManagedReference,
    );
    let child_request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(4),
        AllocationClass::NurseryEligible,
        ObjectMap::new(0, Vec::new()).map_err(|_| RuntimeFailure::runtime_invariant())?,
    );
    let mut roots = empty_roots()?;
    let mut runtime = BootstrapRuntime::new();
    let mut state = WorkloadState::default();

    for _ in 0..configuration.batches {
        let array = runtime.allocate_array(&array_request)?;
        let root = runtime.retain_root(array)?;
        state.root_transitions += 1;
        for index in 0..configuration.items_per_batch {
            let child = runtime.allocate_object(&child_request)?;
            runtime.store_array_value(array, ObjectSlot::new(index), child.raw())?;
            state.reference_stores += 1;
        }
        state.observe(&runtime);
        collect(&mut runtime, &mut roots)?;
        runtime.release_root(root)?;
        state.root_transitions += 1;
        collect(&mut runtime, &mut roots)?;
    }

    Ok(state.finish(
        &runtime,
        "managed_array",
        "rooted_managed_reference_array",
        1,
    ))
}
