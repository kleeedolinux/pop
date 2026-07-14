//! Native ABI 1.10 storage for immutable first-class integer ranges.

use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, ObjectSlot, RuntimeAdapter, RuntimeTypeId,
};
use pop_runtime_native_abi::IterationStatus;

use crate::state::abi_runtime;

pub(crate) const FIRST_SLOT: u32 = 0;
pub(crate) const LAST_SLOT: u32 = 1;
pub(crate) const STEP_SLOT: u32 = 2;
pub(crate) const SIGNED_SLOT: u32 = 3;
pub(crate) const BIT_WIDTH_SLOT: u32 = 4;

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_range_create(
    first: u64,
    last: u64,
    step: u64,
    signed: bool,
    bit_width: u8,
) -> u64 {
    if !matches!(bit_width, 8 | 16 | 32 | 64) {
        return 0;
    }
    let mask = width_mask(bit_width);
    if step & mask == 0 {
        return 0;
    }
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    let Ok(object_map) = ObjectMap::new(5, Vec::new()) else {
        return 0;
    };
    let request =
        ObjectAllocationRequest::new(RuntimeTypeId::new(0), AllocationClass::Mature, object_map);
    let Ok(range) = runtime.allocate_object(&request) else {
        return 0;
    };
    for (slot, value) in [
        (FIRST_SLOT, first & mask),
        (LAST_SLOT, last & mask),
        (STEP_SLOT, step & mask),
        (SIGNED_SLOT, u64::from(signed)),
        (BIT_WIDTH_SLOT, u64::from(bit_width)),
    ] {
        if runtime
            .store_slot_value(range, ObjectSlot::new(slot), value)
            .is_err()
        {
            return 0;
        }
    }
    range.raw()
}

pub(crate) const fn width_mask(bit_width: u8) -> u64 {
    if bit_width == 64 {
        u64::MAX
    } else {
        (1_u64 << bit_width) - 1
    }
}

pub(crate) const fn signed_value(raw: u64, bit_width: u8) -> i64 {
    let shift = 64_u32 - bit_width as u32;
    i64::from_ne_bytes((raw << shift).to_ne_bytes()) >> shift
}

pub(crate) fn load_range(
    runtime: &StableGenerationalRuntime,
    source: pop_runtime_interface::ManagedReference,
) -> Option<(u64, u64, u64, bool, u8)> {
    let load = |slot| runtime.load_slot_value(source, ObjectSlot::new(slot)).ok();
    let first = load(FIRST_SLOT)?;
    let last = load(LAST_SLOT)?;
    let step = load(STEP_SLOT)?;
    let signed = load(SIGNED_SLOT)? != 0;
    let bit_width = u8::try_from(load(BIT_WIDTH_SLOT)?).ok()?;
    matches!(bit_width, 8 | 16 | 32 | 64).then_some((first, last, step, signed, bit_width))
}

pub(crate) struct RangeIterationStep {
    pub(crate) item: Option<u64>,
    pub(crate) position: u64,
    pub(crate) state: u64,
}

pub(crate) fn range_iteration_step(
    runtime: &StableGenerationalRuntime,
    source: pop_runtime_interface::ManagedReference,
    position: u64,
    state: u64,
) -> Result<RangeIterationStep, IterationStatus> {
    let (_, last, step, signed, bit_width) =
        load_range(runtime, source).ok_or(IterationStatus::Failure)?;
    let mask = width_mask(bit_width);
    let current = if state == 0 {
        position & mask
    } else {
        checked_range_add(position & mask, step & mask, signed, bit_width)?
    };
    let last = last & mask;
    let step = step & mask;
    let (in_range, at_end) = if signed {
        let current = signed_value(current, bit_width);
        let last = signed_value(last, bit_width);
        let step = signed_value(step, bit_width);
        (
            if step > 0 {
                current <= last
            } else {
                current >= last
            },
            current == last,
        )
    } else {
        (current <= last, current == last)
    };
    Ok(RangeIterationStep {
        item: in_range.then_some(current),
        position: current,
        state: if in_range && !at_end { 1 } else { 2 },
    })
}

fn checked_range_add(
    current: u64,
    step: u64,
    signed: bool,
    bit_width: u8,
) -> Result<u64, IterationStatus> {
    if signed {
        let result = i128::from(signed_value(current, bit_width))
            .checked_add(i128::from(signed_value(step, bit_width)))
            .ok_or(IterationStatus::IntegerOverflow)?;
        let minimum = -(1_i128 << (bit_width - 1));
        let maximum = (1_i128 << (bit_width - 1)) - 1;
        if result < minimum || result > maximum {
            return Err(IterationStatus::IntegerOverflow);
        }
        let result = i64::try_from(result).map_err(|_| IterationStatus::IntegerOverflow)?;
        Ok(u64::from_ne_bytes(result.to_ne_bytes()) & width_mask(bit_width))
    } else {
        let result = u128::from(current)
            .checked_add(u128::from(step))
            .ok_or(IterationStatus::IntegerOverflow)?;
        if result > u128::from(width_mask(bit_width)) {
            return Err(IterationStatus::IntegerOverflow);
        }
        u64::try_from(result).map_err(|_| IterationStatus::IntegerOverflow)
    }
}
