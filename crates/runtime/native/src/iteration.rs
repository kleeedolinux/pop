//! Closed native adapters for reserved nominal collection iteration.

use pop_runtime_collector::BootstrapRuntime;
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RuntimeAdapter, RuntimeTypeId,
};
use pop_runtime_native_abi::{IterationCollectionKind, IterationStatus};

use crate::state::{abi_lists, abi_runtime, abi_tables};

const SOURCE_SLOT: u32 = 0;
const KIND_SLOT: u32 = 1;
const EXPECTED_LENGTH_SLOT: u32 = 2;
const POSITION_SLOT: u32 = 3;

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_iteration_acquire(source: u64, kind: u8) -> u64 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    let length = if kind == IterationCollectionKind::Array as u8 {
        runtime.array_length(ManagedReference::new(source))
    } else if kind == IterationCollectionKind::Table as u8 {
        let Ok(tables) = abi_tables().lock() else {
            return 0;
        };
        tables.get(&source).map(|table| u64::from(table.length))
    } else if kind == IterationCollectionKind::List as u8 {
        let Ok(lists) = abi_lists().lock() else {
            return 0;
        };
        lists.get(&source).map(|list| u64::from(list.length))
    } else {
        None
    };
    let Some(length) = length else {
        return 0;
    };
    let Ok(object_map) = ObjectMap::new(4, vec![ObjectSlot::new(SOURCE_SLOT)]) else {
        return 0;
    };
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, object_map);
    let Ok(iterator) = runtime.allocate_object(&request) else {
        return 0;
    };
    for (slot, value) in [
        (SOURCE_SLOT, source),
        (KIND_SLOT, u64::from(kind)),
        (EXPECTED_LENGTH_SLOT, length),
        (POSITION_SLOT, 0),
    ] {
        if runtime
            .store_slot_value(iterator, ObjectSlot::new(slot), value)
            .is_err()
        {
            return 0;
        }
    }
    iterator.raw()
}

/// Advances one reserved iterator and writes its typed raw item payload.
///
/// # Safety
///
/// `output` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_iteration_next(iterator: u64, output: *mut u64) -> u8 {
    if output.is_null() {
        return IterationStatus::Failure as u8;
    }
    let Ok(mut runtime) = abi_runtime().lock() else {
        return IterationStatus::Failure as u8;
    };
    let iterator = ManagedReference::new(iterator);
    let load = |slot| runtime.load_slot_value(iterator, ObjectSlot::new(slot));
    let Ok(source) = load(SOURCE_SLOT) else {
        return IterationStatus::Failure as u8;
    };
    let Ok(kind) = load(KIND_SLOT) else {
        return IterationStatus::Failure as u8;
    };
    let Ok(expected_length) = load(EXPECTED_LENGTH_SLOT) else {
        return IterationStatus::Failure as u8;
    };
    let Ok(position) = load(POSITION_SLOT) else {
        return IterationStatus::Failure as u8;
    };

    let Ok((length, item)) = iteration_item(&mut runtime, source, kind, position) else {
        return IterationStatus::Failure as u8;
    };

    if length != expected_length {
        return IterationStatus::ConcurrentModification as u8;
    }
    let Some(item) = item else {
        // SAFETY: The caller supplied one writable output slot.
        unsafe { output.write(0) };
        return IterationStatus::End as u8;
    };
    if runtime
        .store_slot_value(
            iterator,
            ObjectSlot::new(POSITION_SLOT),
            position.saturating_add(1),
        )
        .is_err()
    {
        return IterationStatus::Failure as u8;
    }
    // SAFETY: The caller supplied one writable output slot.
    unsafe { output.write(item) };
    IterationStatus::Item as u8
}

fn iteration_item(
    runtime: &mut BootstrapRuntime,
    source: u64,
    kind: u64,
    position: u64,
) -> Result<(u64, Option<u64>), ()> {
    if kind == u64::from(IterationCollectionKind::Array as u8) {
        array_iteration_item(runtime, source, position)
    } else if kind == u64::from(IterationCollectionKind::Table as u8) {
        table_iteration_item(runtime, source, position)
    } else if kind == u64::from(IterationCollectionKind::List as u8) {
        list_iteration_item(runtime, source, position)
    } else {
        Err(())
    }
}

fn array_iteration_item(
    runtime: &BootstrapRuntime,
    source: u64,
    position: u64,
) -> Result<(u64, Option<u64>), ()> {
    let source = ManagedReference::new(source);
    let length = runtime.array_length(source).ok_or(())?;
    let item = if position < length {
        runtime
            .load_array_value(
                source,
                ObjectSlot::new(u32::try_from(position).map_err(|_| ())?),
            )
            .ok()
    } else {
        None
    };
    Ok((length, item))
}

fn table_iteration_item(
    runtime: &mut BootstrapRuntime,
    source: u64,
    position: u64,
) -> Result<(u64, Option<u64>), ()> {
    let tables = abi_tables().lock().map_err(|_| ())?;
    let table = tables.get(&source).copied().ok_or(())?;
    if position >= u64::from(table.length) {
        return Ok((u64::from(table.length), None));
    }
    let entry = u32::try_from(position).map_err(|_| ())?;
    let owner = ManagedReference::new(source);
    let key = runtime
        .load_slot_value(owner, ObjectSlot::new(entry.saturating_mul(2)))
        .map_err(|_| ())?;
    let value = runtime
        .load_slot_value(
            owner,
            ObjectSlot::new(entry.saturating_mul(2).saturating_add(1)),
        )
        .map_err(|_| ())?;
    let mut references = Vec::new();
    if table.key_map == pop_runtime_interface::ArrayElementMap::ManagedReference {
        references.push(ObjectSlot::new(0));
    }
    if table.value_map == pop_runtime_interface::ArrayElementMap::ManagedReference {
        references.push(ObjectSlot::new(1));
    }
    let tuple_map = ObjectMap::new(2, references).map_err(|_| ())?;
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, tuple_map);
    let tuple = runtime.allocate_object(&request).map_err(|_| ())?;
    runtime
        .store_slot_value(tuple, ObjectSlot::new(0), key)
        .map_err(|_| ())?;
    runtime
        .store_slot_value(tuple, ObjectSlot::new(1), value)
        .map_err(|_| ())?;
    Ok((u64::from(table.length), Some(tuple.raw())))
}

fn list_iteration_item(
    runtime: &BootstrapRuntime,
    source: u64,
    position: u64,
) -> Result<(u64, Option<u64>), ()> {
    let lists = abi_lists().lock().map_err(|_| ())?;
    let list = lists.get(&source).copied().ok_or(())?;
    let item = if position < u64::from(list.length) {
        let position = u32::try_from(position).map_err(|_| ())?;
        runtime
            .load_slot_value(
                ManagedReference::new(source),
                ObjectSlot::new(position.saturating_mul(2).saturating_add(1)),
            )
            .ok()
    } else {
        None
    };
    Ok((u64::from(list.length), item))
}
