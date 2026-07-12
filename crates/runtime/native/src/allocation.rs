//! Native managed allocation exports.

use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ManagedReference,
    ObjectAllocationRequest, ObjectMap, ObjectSlot, RuntimeAdapter, RuntimeTypeId,
    TableAllocationRequest,
};

use crate::state::abi_runtime;

/// Allocates a scalar array and returns its opaque managed handle, or zero on
/// a typed runtime failure.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_array(length: u64, managed: u8) -> u64 {
    let Ok(length) = u32::try_from(length) else {
        return 0;
    };
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(0),
        AllocationClass::Mature,
        length,
        if managed == 0 {
            ArrayElementMap::Scalar
        } else {
            ArrayElementMap::ManagedReference
        },
    );
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .allocate_array(&request)
        .map_or(0, ManagedReference::raw)
}

/// Allocates one fixed array and initializes every element before publication.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_array_filled(
    length: u64,
    managed: u8,
    initial_value: u64,
) -> u64 {
    let Ok(length) = u32::try_from(length) else {
        return 0;
    };
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(0),
        AllocationClass::Mature,
        length,
        if managed == 0 {
            ArrayElementMap::Scalar
        } else {
            ArrayElementMap::ManagedReference
        },
    );
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    let Ok(reference) = runtime.allocate_array(&request) else {
        return 0;
    };
    if runtime.fill_array_value(reference, initial_value).is_err() {
        return 0;
    }
    reference.raw()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_object(slot_count: u64) -> u64 {
    let Ok(slot_count) = u32::try_from(slot_count) else {
        return 0;
    };
    abi_allocate_object(slot_count)
}

/// Allocates an object using explicit zero-based managed-reference slots.
#[must_use]
pub fn allocate_mapped_object(slot_count: u64, reference_slots: &[u32]) -> u64 {
    let Ok(slot_count) = u32::try_from(slot_count) else {
        return 0;
    };
    let slots = reference_slots
        .iter()
        .copied()
        .map(ObjectSlot::new)
        .collect();
    let Ok(object_map) = ObjectMap::new(slot_count, slots) else {
        return 0;
    };
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, object_map);
    let Ok(mut runtime) = abi_runtime().lock() else {
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

fn abi_allocate_object(slot_count: u32) -> u64 {
    let Ok(object_map) = ObjectMap::new(slot_count, Vec::new()) else {
        return 0;
    };
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, object_map);
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .allocate_object(&request)
        .map_or(0, ManagedReference::raw)
}

/// Allocates interleaved typed table storage with homogeneous key/value maps.
/// Zero signals an invalid capacity or allocation failure.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_table(
    entry_count: u64,
    managed_keys: u8,
    managed_values: u8,
) -> u64 {
    let Ok(entry_count) = u32::try_from(entry_count) else {
        return 0;
    };
    let Ok(request) = TableAllocationRequest::new(
        RuntimeTypeId::new(0),
        AllocationClass::Mature,
        entry_count,
        if managed_keys == 0 {
            ArrayElementMap::Scalar
        } else {
            ArrayElementMap::ManagedReference
        },
        if managed_values == 0 {
            ArrayElementMap::Scalar
        } else {
            ArrayElementMap::ManagedReference
        },
    ) else {
        return 0;
    };
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .allocate_table(&request)
        .map_or(0, ManagedReference::raw)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_tuple_make(length: u64) -> u64 {
    let Ok(length) = u32::try_from(length) else {
        return 0;
    };
    abi_allocate_object(length)
}
