use pop_runtime_collector::BootstrapRuntime;
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, ObjectSlot, RuntimeAdapter,
    RuntimeFailure, RuntimeTypeId,
};

use super::model::{WorkloadConfiguration, WorkloadCounters, WorkloadState, collect, empty_roots};

/// Allocates rooted reference chains, traces them once, then releases and
/// reclaims them.
///
/// # Errors
///
/// Returns a portable runtime failure for invalid configuration or any
/// allocation, reference store, root transition, or collection failure.
pub fn run_rooted_chain(
    configuration: WorkloadConfiguration,
) -> Result<WorkloadCounters, RuntimeFailure> {
    let configuration = configuration.validate()?;
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(2),
        AllocationClass::NurseryEligible,
        ObjectMap::new(1, vec![ObjectSlot::new(0)])
            .map_err(|_| RuntimeFailure::runtime_invariant())?,
    );
    let mut roots = empty_roots()?;
    let mut runtime = BootstrapRuntime::new();
    let mut state = WorkloadState::default();

    for _ in 0..configuration.batches {
        let mut nodes = Vec::with_capacity(
            usize::try_from(configuration.items_per_batch)
                .map_err(|_| RuntimeFailure::runtime_invariant())?,
        );
        for _ in 0..configuration.items_per_batch {
            nodes.push(runtime.allocate_object(&request)?);
        }
        for pair in nodes.windows(2) {
            runtime.store_reference(pair[0], ObjectSlot::new(0), Some(pair[1]))?;
            state.reference_stores += 1;
        }
        let root = runtime.retain_root(nodes[0])?;
        state.root_transitions += 1;
        state.observe(&runtime);
        collect(&mut runtime, &mut roots)?;
        runtime.release_root(root)?;
        state.root_transitions += 1;
        collect(&mut runtime, &mut roots)?;
    }

    Ok(state.finish(&runtime, "rooted_chain", "single_reference_chain", 1))
}
