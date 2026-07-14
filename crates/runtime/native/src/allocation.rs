//! Native managed allocation exports.

use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ManagedReference,
    ObjectAllocationRequest, ObjectMap, ObjectSlot, RuntimeAdapter, RuntimeTypeId,
    TableAllocationRequest,
};

use crate::state::{TableMetadata, abi_runtime, abi_tables};

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
    runtime
        .allocate_array_filled(&request, initial_value)
        .map_or(0, ManagedReference::raw)
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
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, object_map);
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .allocate_object_initialized(&request, values)
        .map_or(0, ManagedReference::raw)
}

fn abi_allocate_object(slot_count: u32) -> u64 {
    let object_map = ObjectMap::scalar(slot_count);
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
    let Ok(reference) = runtime.allocate_table(&request) else {
        return 0;
    };
    drop(runtime);
    let Ok(mut tables) = abi_tables().lock() else {
        return 0;
    };
    tables.insert(
        reference.raw(),
        TableMetadata {
            length: 0,
            capacity: entry_count,
            key_map: request.key_map(),
            value_map: request.value_map(),
        },
    );
    reference.raw()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_tuple_make(length: u64) -> u64 {
    let Ok(length) = u32::try_from(length) else {
        return 0;
    };
    abi_allocate_object(length)
}
