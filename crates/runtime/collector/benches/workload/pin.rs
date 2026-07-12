use pop_runtime_collector::BootstrapRuntime;
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, RuntimeAdapter, RuntimeFailure,
    RuntimeTypeId,
};

use super::model::{WorkloadConfiguration, WorkloadCounters, WorkloadState, collect, empty_roots};

/// Exercises scoped pins as precise strong roots across one collection, then
/// unpins and reclaims every object.
///
/// # Errors
///
/// Returns a portable runtime failure for invalid configuration or any
/// allocation, pin transition, or collection failure.
pub fn run_pin_pressure(
    configuration: WorkloadConfiguration,
) -> Result<WorkloadCounters, RuntimeFailure> {
    let configuration = configuration.validate()?;
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(5),
        AllocationClass::Pinned,
        ObjectMap::new(0, Vec::new()).map_err(|_| RuntimeFailure::runtime_invariant())?,
    );
    let mut roots = empty_roots()?;
    let mut runtime = BootstrapRuntime::new();
    let mut state = WorkloadState::default();

    for _ in 0..configuration.batches {
        let mut pins = Vec::with_capacity(
            usize::try_from(configuration.items_per_batch)
                .map_err(|_| RuntimeFailure::runtime_invariant())?,
        );
        for _ in 0..configuration.items_per_batch {
            let object = runtime.allocate_object(&request)?;
            pins.push(runtime.pin(object)?);
            state.pin_transitions += 1;
        }
        state.observe(&runtime);
        collect(&mut runtime, &mut roots)?;
        for pin in pins {
            runtime.unpin(pin)?;
            state.pin_transitions += 1;
        }
        collect(&mut runtime, &mut roots)?;
    }

    Ok(state.finish(
        &runtime,
        "pin_pressure",
        "independent_pinned_objects",
        u64::from(configuration.items_per_batch),
    ))
}
