//! Native typed array, table, and field storage exports.

use pop_runtime_interface::{ManagedReference, ObjectSlot};

use crate::state::lock_abi_runtime;

mod direct;
mod table;

use direct::{
    CachedReferenceStore, direct_array_object_field_value,
    direct_page_store_array_reference_cached, direct_page_store_array_reference_slow,
    direct_page_store_scalar, direct_page_value,
};
pub use direct::{
    direct_page_access_miss_count, direct_page_mutation_miss_count,
    direct_reference_mutation_miss_count,
};
pub(crate) use direct::{quiesce_direct_accesses, seed_direct_array_reference_store};
pub use table::{pop_rt_table_get, pop_rt_table_get_checked, pop_rt_table_set};

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_array_get(reference: u64, index: u64) -> u64 {
    let Some(slot) = array_slot(index) else {
        return 0;
    };
    let reference = ManagedReference::new(reference);
    if let Some(value) = direct_page_value(reference, slot, true) {
        return value;
    }
    let Ok(runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime.load_array_value(reference, slot).unwrap_or(0)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_array_set(reference: u64, index: u64, value: u64) -> u8 {
    store_array_value(reference, index, value)
}

#[inline]
pub(crate) fn store_array_value(reference: u64, index: u64, value: u64) -> u8 {
    let Some(slot) = array_slot(index) else {
        return 0;
    };
    let reference = ManagedReference::new(reference);
    match direct_page_store_array_reference_cached(reference, slot, value) {
        CachedReferenceStore::Stored => return 1,
        CachedReferenceStore::Rejected => {
            return u8::from(direct_page_store_array_reference_slow(
                reference, slot, value,
            ));
        }
        CachedReferenceStore::OwnerMiss => {}
    }
    if direct_page_store_scalar(reference, slot, true, value) {
        return 1;
    }
    if direct_page_store_array_reference_slow(reference, slot, value) {
        return 1;
    }
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    u8::from(runtime.store_array_value(reference, slot, value).is_ok())
}

/// Loads one checked managed-reference array element and one statically
/// resolved object field through the ABI 1.21 adjacent-pair adapter.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_array_get_object_field_checked(
    reference: u64,
    index: u64,
    field: u64,
    output: *mut u64,
) -> u8 {
    let Some(owner_slot) = array_slot(index) else {
        return 0;
    };
    let Some(field_slot) = array_slot(field) else {
        return 0;
    };
    if output.is_null() {
        return 0;
    }
    let owner = ManagedReference::new(reference);
    if let Some(value) = direct_array_object_field_value(owner, owner_slot, field_slot) {
        // SAFETY: the caller contract requires one writable `u64`.
        unsafe { output.write(value) };
        return 1;
    }
    let target = if let Some(value) = direct_page_value(owner, owner_slot, true) {
        value
    } else {
        let Ok(runtime) = lock_abi_runtime() else {
            return 0;
        };
        let Ok(value) = runtime.load_array_value(owner, owner_slot) else {
            return 0;
        };
        value
    };
    if target == 0 {
        return 0;
    }
    let target = ManagedReference::new(target);
    let value = if let Some(value) = direct_page_value(target, field_slot, false) {
        value
    } else {
        let Ok(runtime) = lock_abi_runtime() else {
            return 0;
        };
        let Ok(value) = runtime.load_slot_value(target, field_slot) else {
            return 0;
        };
        value
    };
    // SAFETY: the caller contract requires one writable `u64`.
    unsafe { output.write(value) };
    1
}

/// Writes the fixed array length through `output` and reports success.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_array_length(reference: u64, output: *mut u64) -> u8 {
    if output.is_null() {
        return 0;
    }
    let Ok(runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Some(length) = runtime.array_length(ManagedReference::new(reference)) else {
        return 0;
    };
    // SAFETY: The caller contract requires one writable `u64`.
    unsafe { output.write(length) };
    1
}

/// Loads one array element through `output` and reports bounds/type success.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_array_get_checked(
    reference: u64,
    index: u64,
    output: *mut u64,
) -> u8 {
    let Some(slot) = array_slot(index) else {
        return 0;
    };
    if output.is_null() {
        return 0;
    }
    let reference = ManagedReference::new(reference);
    if let Some(value) = direct_page_value(reference, slot, true) {
        // SAFETY: The caller contract requires one writable `u64`.
        unsafe { output.write(value) };
        return 1;
    }
    let Ok(runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Ok(value) = runtime.load_array_value(reference, slot) else {
        return 0;
    };
    // SAFETY: The caller contract requires one writable `u64`.
    unsafe { output.write(value) };
    1
}

/// Replaces every fixed-array element with one typed scalar or managed handle.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_array_fill(reference: u64, value: u64) -> u8 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    u8::from(
        runtime
            .fill_array_value(ManagedReference::new(reference), value)
            .is_ok(),
    )
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_field_get(reference: u64, field: u64) -> u64 {
    let Some(slot) = array_slot(field) else {
        return 0;
    };
    let reference = ManagedReference::new(reference);
    if let Some(value) = direct_page_value(reference, slot, false) {
        return value;
    }
    let Ok(runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime.load_slot_value(reference, slot).unwrap_or(0)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_field_set(reference: u64, field: u64, value: u64) -> u8 {
    let Some(slot) = array_slot(field) else {
        return 0;
    };
    let reference = ManagedReference::new(reference);
    if direct_page_store_scalar(reference, slot, false, value) {
        return 1;
    }
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    u8::from(runtime.store_slot_value(reference, slot, value).is_ok())
}

fn array_slot(index: u64) -> Option<ObjectSlot> {
    (index > 0)
        .then(|| u32::try_from(index - 1).ok())
        .flatten()
        .map(ObjectSlot::new)
}
