//! Native typed array, table, and field storage exports.

use pop_runtime_interface::{ManagedReference, ObjectSlot};

use crate::state::abi_runtime;

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_array_get(reference: u64, index: u64) -> u64 {
    let Some(slot) = array_slot(index) else {
        return 0;
    };
    let Ok(runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .load_array_value(ManagedReference::new(reference), slot)
        .unwrap_or(0)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_array_set(reference: u64, index: u64, value: u64) -> u8 {
    let Some(slot) = array_slot(index) else {
        return 0;
    };
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(
        runtime
            .store_array_value(ManagedReference::new(reference), slot, value)
            .is_ok(),
    )
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
    let Ok(runtime) = abi_runtime().lock() else {
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
    let Ok(runtime) = abi_runtime().lock() else {
        return 0;
    };
    let Ok(value) = runtime.load_array_value(ManagedReference::new(reference), slot) else {
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
    let Ok(mut runtime) = abi_runtime().lock() else {
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
    let Ok(runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .load_slot_value(ManagedReference::new(reference), slot)
        .unwrap_or(0)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_field_set(reference: u64, field: u64, value: u64) -> u8 {
    let Some(slot) = array_slot(field) else {
        return 0;
    };
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(
        runtime
            .store_slot_value(ManagedReference::new(reference), slot, value)
            .is_ok(),
    )
}

fn array_slot(index: u64) -> Option<ObjectSlot> {
    (index > 0)
        .then(|| u32::try_from(index - 1).ok())
        .flatten()
        .map(ObjectSlot::new)
}
