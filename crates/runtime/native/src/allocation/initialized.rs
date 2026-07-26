use pop_runtime_interface::{
    ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot, RuntimeAdapter, RuntimeTypeId,
};
use pop_runtime_native_abi::AllocationSiteDescriptorAbi;

use super::native_default_allocation_class;
use crate::state::lock_abi_runtime;

/// Atomically initializes one object and stores its token into one checked
/// managed-reference array slot through the ABI 1.21 adjacent-pair adapter.
///
/// # Safety
///
/// `descriptor` and `initial_values` follow
/// [`super::pop_rt_allocate_initialized_object_at_site`] exactly.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_allocate_initialized_object_at_site_and_store_array(
    descriptor: *const AllocationSiteDescriptorAbi,
    initial_values: *const u64,
    value_count: u64,
    array: u64,
    index: u64,
) -> u64 {
    // SAFETY: forwarded unchanged from this exported ABI contract.
    unsafe {
        super::site::allocate_initialized_object_at_site_and_store_array(
            descriptor,
            initial_values,
            value_count,
            array,
            index,
        )
    }
}

/// Allocates an object using explicit zero-based managed-reference slots.
#[must_use]
pub fn allocate_mapped_object(slot_count: u64, reference_slots: &[u32]) -> u64 {
    let Ok(slot_count) = u32::try_from(slot_count) else {
        return 0;
    };
    let object_map = if reference_slots.is_empty() {
        ObjectMap::scalar(slot_count)
    } else {
        let slots = reference_slots
            .iter()
            .copied()
            .map(ObjectSlot::new)
            .collect();
        let Ok(object_map) = ObjectMap::new(slot_count, slots) else {
            return 0;
        };
        object_map
    };
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(0),
        native_default_allocation_class(),
        object_map,
    );
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .allocate_object(&request)
        .map_or(0, ManagedReference::raw)
}

/// C-compatible mapped-object allocation boundary used by native LLVM code.
///
/// # Safety
///
/// When `reference_count` is nonzero, `reference_slots` must address that many
/// readable `u32` slot indices for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_allocate_mapped_object(
    slot_count: u64,
    reference_slots: *const u32,
    reference_count: u64,
) -> u64 {
    let Ok(reference_count) = usize::try_from(reference_count) else {
        return 0;
    };
    if reference_count == 0 {
        return allocate_mapped_object(slot_count, &[]);
    }
    if reference_slots.is_null() {
        return 0;
    }
    // SAFETY: The backend passes a stack array containing exactly the declared
    // number of immutable slot indices.
    let reference_slots = unsafe { std::slice::from_raw_parts(reference_slots, reference_count) };
    allocate_mapped_object(slot_count, reference_slots)
}

/// Allocates and publishes one object with its complete precisely mapped
/// payload.
///
/// # Safety
///
/// Nonzero counts require readable arrays of exactly the corresponding length.
/// Initial values use the physical slot representation selected by LLVM.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_allocate_initialized_object(
    slot_count: u64,
    reference_slots: *const u32,
    reference_count: u64,
    initial_values: *const u64,
    value_count: u64,
) -> u64 {
    let Ok(slot_count) = u32::try_from(slot_count) else {
        return 0;
    };
    let Ok(reference_count) = usize::try_from(reference_count) else {
        return 0;
    };
    let Ok(value_count) = usize::try_from(value_count) else {
        return 0;
    };
    if value_count != slot_count as usize
        || (reference_count != 0 && reference_slots.is_null())
        || (value_count != 0 && initial_values.is_null())
    {
        return 0;
    }
    let references = if reference_count == 0 {
        &[]
    } else {
        // SAFETY: The caller contract requires this exact readable array.
        unsafe { std::slice::from_raw_parts(reference_slots, reference_count) }
    };
    let values = if value_count == 0 {
        &[]
    } else {
        // SAFETY: The caller contract requires this exact readable array.
        unsafe { std::slice::from_raw_parts(initial_values, value_count) }
    };
    let object_map = if references.is_empty() {
        ObjectMap::scalar(slot_count)
    } else {
        let slots = references.iter().copied().map(ObjectSlot::new).collect();
        let Ok(object_map) = ObjectMap::new(slot_count, slots) else {
            return 0;
        };
        object_map
    };
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(0),
        native_default_allocation_class(),
        object_map,
    );
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .allocate_object_initialized(&request, values)
        .map_or(0, ManagedReference::raw)
}
