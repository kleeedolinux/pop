use pop_runtime_interface::{ManagedReference, ObjectSlot};

use crate::state::{abi_tables, lock_abi_runtime};

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_table_get(reference: u64, key: u64, managed_key: u8) -> u64 {
    table_value(reference, key, managed_key).unwrap_or(0)
}

fn table_value(reference: u64, key: u64, managed_key: u8) -> Option<u64> {
    let Ok(runtime) = lock_abi_runtime() else {
        return None;
    };
    let Ok(tables) = abi_tables().lock() else {
        return None;
    };
    let table = tables.get(&reference)?;
    if u8::from(table.key_map == pop_runtime_interface::ArrayElementMap::ManagedReference)
        != u8::from(managed_key != 0)
    {
        return None;
    }
    let owner = ManagedReference::new(reference);
    for entry in 0..table.length {
        let key_slot = ObjectSlot::new(entry * 2);
        let Ok(candidate) = runtime.load_slot_value(owner, key_slot) else {
            return None;
        };
        let equal = if managed_key == 0 || candidate == 0 || key == 0 {
            candidate == key
        } else {
            runtime.strings_equal(ManagedReference::new(candidate), ManagedReference::new(key))
        };
        if equal {
            return runtime
                .load_slot_value(owner, ObjectSlot::new(entry * 2 + 1))
                .ok();
        }
    }
    None
}

/// Loads one table value through `output` and reports key presence.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_table_get_checked(
    reference: u64,
    key: u64,
    managed_key: u8,
    output: *mut u64,
) -> u8 {
    if output.is_null() {
        return 0;
    }
    let Some(value) = table_value(reference, key, managed_key) else {
        return 0;
    };
    // SAFETY: The caller contract requires one writable `u64`.
    unsafe { output.write(value) };
    1
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_table_set(
    reference: u64,
    key: u64,
    value: u64,
    managed_key: u8,
    managed_value: u8,
) -> u8 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Ok(mut tables) = abi_tables().lock() else {
        return 0;
    };
    let Some(table) = tables.get_mut(&reference) else {
        return 0;
    };
    if u8::from(table.key_map == pop_runtime_interface::ArrayElementMap::ManagedReference)
        != u8::from(managed_key != 0)
        || u8::from(table.value_map == pop_runtime_interface::ArrayElementMap::ManagedReference)
            != u8::from(managed_value != 0)
    {
        return 0;
    }
    let owner = ManagedReference::new(reference);
    for entry in 0..table.length {
        let key_slot = ObjectSlot::new(entry * 2);
        let Ok(candidate) = runtime.load_slot_value(owner, key_slot) else {
            return 0;
        };
        let equal = if managed_key == 0 || candidate == 0 || key == 0 {
            candidate == key
        } else {
            runtime.strings_equal(ManagedReference::new(candidate), ManagedReference::new(key))
        };
        if equal {
            return u8::from(
                runtime
                    .store_slot_value(owner, ObjectSlot::new(entry * 2 + 1), value)
                    .is_ok(),
            );
        }
    }
    if table.length == table.capacity {
        let Some(new_capacity) = table.capacity.max(2).checked_mul(2) else {
            return 0;
        };
        if runtime
            .grow_table(
                owner,
                table.capacity,
                new_capacity,
                table.key_map,
                table.value_map,
            )
            .is_err()
        {
            return 0;
        }
        table.capacity = new_capacity;
    }
    let key_slot = ObjectSlot::new(table.length * 2);
    if runtime.store_slot_value(owner, key_slot, key).is_err()
        || runtime
            .store_slot_value(owner, ObjectSlot::new(table.length * 2 + 1), value)
            .is_err()
    {
        return 0;
    }
    table.length += 1;
    1
}
