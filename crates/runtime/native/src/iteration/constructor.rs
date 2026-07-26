use pop_runtime_interface::{ManagedReference, ObjectAllocationRequest, ObjectMap, RuntimeTypeId};
use pop_runtime_native_abi::IterationStatus;

use crate::state::lock_abi_runtime;

/// Atomically constructs one closed `Iteration<T>` case returned by a native
/// collection iterator.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_iteration_make(status: u8, payload: u64, managed: u8) -> u64 {
    let tag = match status {
        value if value == IterationStatus::Item as u8 => 0,
        value if value == IterationStatus::End as u8 => 1,
        _ => return 0,
    };
    let object_map = if managed == 0 {
        ObjectMap::scalar(2)
    } else if managed == 1 {
        let Some(object_map) = ObjectMap::strided_references(2, 1, 2) else {
            return 0;
        };
        object_map
    } else {
        return 0;
    };
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(0),
        crate::allocation::native_default_allocation_class(),
        object_map,
    );
    let values = [tag, if tag == 0 { payload } else { 0 }];
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .allocate_object_initialized(&request, &values)
        .map_or(0, ManagedReference::raw)
}
