//! Native ABI 1.9 storage for the distinct growable `List<T>` abstraction.

use pop_runtime_interface::{
    AllocationClass, ArrayElementMap, ManagedReference, ObjectSlot, RuntimeAdapter, RuntimeTypeId,
    TableAllocationRequest,
};

use crate::state::{ListMetadata, abi_lists, abi_runtime};

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_list_create(capacity: u64, managed_elements: u8) -> u64 {
    let Ok(capacity) = u32::try_from(capacity) else {
        return 0;
    };
    let element_map = if managed_elements == 0 {
        ArrayElementMap::Scalar
    } else {
        ArrayElementMap::ManagedReference
    };
    let Ok(request) = TableAllocationRequest::new(
        RuntimeTypeId::new(0),
        AllocationClass::Mature,
        capacity,
        ArrayElementMap::Scalar,
        element_map,
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
    let Ok(mut lists) = abi_lists().lock() else {
        return 0;
    };
    lists.insert(
        reference.raw(),
        ListMetadata {
            length: 0,
            capacity,
            element_map,
        },
    );
    reference.raw()
}

/// Writes the current List length through `output` and reports success.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_list_length(reference: u64, output: *mut u64) -> u8 {
    if output.is_null() {
        return 0;
    }
    let Ok(lists) = abi_lists().lock() else {
        return 0;
    };
    let Some(list) = lists.get(&reference) else {
        return 0;
    };
    // SAFETY: The caller contract requires one writable `u64`.
    unsafe { output.write(u64::from(list.length)) };
    1
}

fn list_value(reference: u64, index: u64) -> Option<u64> {
    let zero_based = index.checked_sub(1)?;
    let zero_based = u32::try_from(zero_based).ok()?;
    let Ok(runtime) = abi_runtime().lock() else {
        return None;
    };
    let Ok(lists) = abi_lists().lock() else {
        return None;
    };
    let list = lists.get(&reference)?;
    if zero_based >= list.length {
        return None;
    }
    runtime
        .load_slot_value(
            ManagedReference::new(reference),
            ObjectSlot::new(zero_based * 2 + 1),
        )
        .ok()
}

/// Loads one optional List element through `output` and reports presence.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_list_get(reference: u64, index: u64, output: *mut u64) -> u8 {
    if output.is_null() {
        return 0;
    }
    let Some(value) = list_value(reference, index) else {
        return 0;
    };
    // SAFETY: The caller contract requires one writable `u64`.
    unsafe { output.write(value) };
    1
}

/// Loads one checked List element through `output` and reports bounds success.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_list_get_checked(
    reference: u64,
    index: u64,
    output: *mut u64,
) -> u8 {
    // SAFETY: This function forwards the identical output-pointer contract.
    unsafe { pop_rt_list_get(reference, index, output) }
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_list_set(
    reference: u64,
    index: u64,
    value: u64,
    managed_elements: u8,
) -> u8 {
    let Some(zero_based) = index
        .checked_sub(1)
        .and_then(|value| u32::try_from(value).ok())
    else {
        return 0;
    };
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    let Ok(lists) = abi_lists().lock() else {
        return 0;
    };
    let Some(list) = lists.get(&reference) else {
        return 0;
    };
    if zero_based >= list.length
        || u8::from(list.element_map == ArrayElementMap::ManagedReference)
            != u8::from(managed_elements != 0)
    {
        return 0;
    }
    u8::from(
        runtime
            .store_slot_value(
                ManagedReference::new(reference),
                ObjectSlot::new(zero_based * 2 + 1),
                value,
            )
            .is_ok(),
    )
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_list_add(reference: u64, value: u64, managed_elements: u8) -> u8 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    let Ok(mut lists) = abi_lists().lock() else {
        return 0;
    };
    let Some(list) = lists.get_mut(&reference) else {
        return 0;
    };
    if u8::from(list.element_map == ArrayElementMap::ManagedReference)
        != u8::from(managed_elements != 0)
    {
        return 0;
    }
    if list.length == list.capacity {
        let Some(new_capacity) = list.capacity.max(1).checked_mul(2) else {
            return 0;
        };
        if runtime
            .grow_table(
                ManagedReference::new(reference),
                list.capacity,
                new_capacity,
                ArrayElementMap::Scalar,
                list.element_map,
            )
            .is_err()
        {
            return 0;
        }
        list.capacity = new_capacity;
    }
    if runtime
        .store_slot_value(
            ManagedReference::new(reference),
            ObjectSlot::new(list.length * 2 + 1),
            value,
        )
        .is_err()
    {
        return 0;
    }
    list.length += 1;
    1
}
