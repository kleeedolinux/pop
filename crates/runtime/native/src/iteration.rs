//! Closed native adapters for reserved nominal collection iteration.

use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RuntimeAdapter, RuntimeTypeId,
};
use pop_runtime_native_abi::{IterationCollectionKind, IterationStatus};

use crate::range::{load_range, range_iteration_step};
use crate::state::{abi_lists, abi_runtime, abi_tables};

const SOURCE_SLOT: u32 = 0;
const KIND_SLOT: u32 = 1;
const EXPECTED_LENGTH_SLOT: u32 = 2;
const POSITION_SLOT: u32 = 3;
const DONE_SLOT: u32 = 4;

struct IterationStep {
    mutation_token: u64,
    item: Option<u64>,
    next_position: u64,
    state: u64,
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_iteration_acquire(source: u64, kind: u8) -> u64 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    let state = if kind == IterationCollectionKind::Array as u8 {
        runtime
            .array_length(ManagedReference::new(source))
            .map(|length| (length, 0))
    } else if kind == IterationCollectionKind::Table as u8 {
        let Ok(tables) = abi_tables().lock() else {
            return 0;
        };
        tables
            .get(&source)
            .map(|table| (u64::from(table.length), 0))
    } else if kind == IterationCollectionKind::List as u8 {
        let Ok(lists) = abi_lists().lock() else {
            return 0;
        };
        lists.get(&source).map(|list| (u64::from(list.length), 0))
    } else if kind == IterationCollectionKind::Range as u8 {
        load_range(&runtime, ManagedReference::new(source)).map(|(first, _, _, _, _)| (0, first))
    } else {
        None
    };
    let Some((length, position)) = state else {
        return 0;
    };
    let Ok(object_map) = ObjectMap::new(5, vec![ObjectSlot::new(SOURCE_SLOT)]) else {
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
        (POSITION_SLOT, position),
        (DONE_SLOT, 0),
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
    let Ok(done) = load(DONE_SLOT) else {
        return IterationStatus::Failure as u8;
    };
    if done == 2 {
        // SAFETY: The caller supplied one writable output slot.
        unsafe { output.write(0) };
        return IterationStatus::End as u8;
    }

    let step = match iteration_item(&mut runtime, source, kind, position, done) {
        Ok(step) => step,
        Err(status) => return status as u8,
    };

    if step.mutation_token != expected_length {
        return IterationStatus::ConcurrentModification as u8;
    }
    let Some(item) = step.item else {
        let _ = runtime.store_slot_value(iterator, ObjectSlot::new(DONE_SLOT), 1);
        // SAFETY: The caller supplied one writable output slot.
        unsafe { output.write(0) };
        return IterationStatus::End as u8;
    };
    if runtime
        .store_slot_value(iterator, ObjectSlot::new(POSITION_SLOT), step.next_position)
        .is_err()
        || runtime
            .store_slot_value(iterator, ObjectSlot::new(DONE_SLOT), step.state)
            .is_err()
    {
        return IterationStatus::Failure as u8;
    }
    // SAFETY: The caller supplied one writable output slot.
    unsafe { output.write(item) };
    IterationStatus::Item as u8
}

fn iteration_item(
    runtime: &mut StableGenerationalRuntime,
    source: u64,
    kind: u64,
    position: u64,
    state: u64,
) -> Result<IterationStep, IterationStatus> {
    if kind == u64::from(IterationCollectionKind::Array as u8) {
        array_iteration_item(runtime, source, position)
    } else if kind == u64::from(IterationCollectionKind::Table as u8) {
        table_iteration_item(runtime, source, position)
    } else if kind == u64::from(IterationCollectionKind::List as u8) {
        list_iteration_item(runtime, source, position)
    } else if kind == u64::from(IterationCollectionKind::Range as u8) {
        range_iteration_item(runtime, source, position, state)
    } else {
        Err(IterationStatus::Failure)
    }
}

fn array_iteration_item(
    runtime: &StableGenerationalRuntime,
    source: u64,
    position: u64,
) -> Result<IterationStep, IterationStatus> {
    let source = ManagedReference::new(source);
    let length = runtime
        .array_length(source)
        .ok_or(IterationStatus::Failure)?;
    let item = if position < length {
        runtime
            .load_array_value(
                source,
                ObjectSlot::new(u32::try_from(position).map_err(|_| IterationStatus::Failure)?),
            )
            .ok()
    } else {
        None
    };
    Ok(IterationStep {
        mutation_token: length,
        item,
        next_position: position.saturating_add(1),
        state: 0,
    })
}

fn table_iteration_item(
    runtime: &mut StableGenerationalRuntime,
    source: u64,
    position: u64,
) -> Result<IterationStep, IterationStatus> {
    let tables = abi_tables().lock().map_err(|_| IterationStatus::Failure)?;
    let table = tables
        .get(&source)
        .copied()
        .ok_or(IterationStatus::Failure)?;
    if position >= u64::from(table.length) {
        return Ok(IterationStep {
            mutation_token: u64::from(table.length),
            item: None,
            next_position: position,
            state: 2,
        });
    }
    let entry = u32::try_from(position).map_err(|_| IterationStatus::Failure)?;
    let owner = ManagedReference::new(source);
    let key = runtime
        .load_slot_value(owner, ObjectSlot::new(entry.saturating_mul(2)))
        .map_err(|_| IterationStatus::Failure)?;
    let value = runtime
        .load_slot_value(
            owner,
            ObjectSlot::new(entry.saturating_mul(2).saturating_add(1)),
        )
        .map_err(|_| IterationStatus::Failure)?;
    let mut references = Vec::new();
    if table.key_map == pop_runtime_interface::ArrayElementMap::ManagedReference {
        references.push(ObjectSlot::new(0));
    }
    if table.value_map == pop_runtime_interface::ArrayElementMap::ManagedReference {
        references.push(ObjectSlot::new(1));
    }
    let tuple_map = ObjectMap::new(2, references).map_err(|_| IterationStatus::Failure)?;
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, tuple_map);
    let tuple = runtime
        .allocate_object(&request)
        .map_err(|_| IterationStatus::Failure)?;
    runtime
        .store_slot_value(tuple, ObjectSlot::new(0), key)
        .map_err(|_| IterationStatus::Failure)?;
    runtime
        .store_slot_value(tuple, ObjectSlot::new(1), value)
        .map_err(|_| IterationStatus::Failure)?;
    Ok(IterationStep {
        mutation_token: u64::from(table.length),
        item: Some(tuple.raw()),
        next_position: position.saturating_add(1),
        state: 0,
    })
}

fn list_iteration_item(
    runtime: &StableGenerationalRuntime,
    source: u64,
    position: u64,
) -> Result<IterationStep, IterationStatus> {
    let lists = abi_lists().lock().map_err(|_| IterationStatus::Failure)?;
    let list = lists
        .get(&source)
        .copied()
        .ok_or(IterationStatus::Failure)?;
    let item = if position < u64::from(list.length) {
        let position = u32::try_from(position).map_err(|_| IterationStatus::Failure)?;
        runtime
            .load_slot_value(
                ManagedReference::new(source),
                ObjectSlot::new(position.saturating_mul(2).saturating_add(1)),
            )
            .ok()
    } else {
        None
    };
    Ok(IterationStep {
        mutation_token: u64::from(list.length),
        item,
        next_position: position.saturating_add(1),
        state: 0,
    })
}

fn range_iteration_item(
    runtime: &StableGenerationalRuntime,
    source: u64,
    position: u64,
    state: u64,
) -> Result<IterationStep, IterationStatus> {
    let step = range_iteration_step(runtime, ManagedReference::new(source), position, state)?;
    Ok(IterationStep {
        mutation_token: 0,
        item: step.item,
        next_position: step.position,
        state: step.state,
    })
}
