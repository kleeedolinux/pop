//! Native managed allocation exports.

mod initialized;
mod site;

use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ManagedReference,
    ObjectAllocationRequest, ObjectMap, RuntimeAdapter, RuntimeTypeId, TableAllocationRequest,
};

use crate::state::{TableMetadata, abi_tables, lock_abi_runtime};

#[must_use]
pub(crate) const fn native_default_allocation_class() -> AllocationClass {
    if cfg!(feature = "production-generational") {
        AllocationClass::NurseryEligible
    } else {
        AllocationClass::Mature
    }
}

#[doc(hidden)]
#[must_use]
pub fn native_allocation_class(reference: u64) -> Option<AllocationClass> {
    lock_abi_runtime()
        .ok()?
        .allocation_class(ManagedReference::new(reference))
}

pub use initialized::{
    allocate_mapped_object, pop_rt_allocate_initialized_object,
    pop_rt_allocate_initialized_object_at_site_and_store_array, pop_rt_allocate_mapped_object,
};
pub(crate) use site::flush_thread_local_allocations;
pub use site::{
    allocation_site_descriptor_count, allocation_site_global_lookup_count,
    allocation_site_tlab_refill_count, pop_rt_allocate_initialized_object_at_site,
    pop_rt_allocate_initialized_self_referential_object_at_site,
};
pub(crate) use site::{
    cached_direct_reference_store, cached_direct_reference_store_contains,
    quiesce_direct_reference_store, seed_direct_array_reference_store,
    seed_direct_reference_validation,
};

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
        native_default_allocation_class(),
        length,
        if managed == 0 {
            ArrayElementMap::Scalar
        } else {
            ArrayElementMap::ManagedReference
        },
    );
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let reference = runtime
        .allocate_array(&request)
        .map_or(0, ManagedReference::raw);
    if managed != 0
        && reference != 0
        && let Some(store) =
            runtime.direct_array_reference_store_access(ManagedReference::new(reference))
    {
        crate::storage::seed_direct_array_reference_store(store);
    }
    reference
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
        native_default_allocation_class(),
        length,
        if managed == 0 {
            ArrayElementMap::Scalar
        } else {
            ArrayElementMap::ManagedReference
        },
    );
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let reference = runtime
        .allocate_array_filled(&request, initial_value)
        .map_or(0, ManagedReference::raw);
    if managed != 0
        && reference != 0
        && let Some(store) =
            runtime.direct_array_reference_store_access(ManagedReference::new(reference))
    {
        crate::storage::seed_direct_array_reference_store(store);
    }
    reference
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_allocate_object(slot_count: u64) -> u64 {
    let Ok(slot_count) = u32::try_from(slot_count) else {
        return 0;
    };
    abi_allocate_object(slot_count)
}

fn abi_allocate_object(slot_count: u32) -> u64 {
    let object_map = ObjectMap::scalar(slot_count);
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
        native_default_allocation_class(),
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
    let Ok(mut runtime) = lock_abi_runtime() else {
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
