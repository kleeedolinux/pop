//! Static allocation-site descriptor validation and caching.

use std::cell::UnsafeCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use pop_runtime_collector::{
    DirectReferenceStoreAccess, DirectReferenceValidation, ReservedMatureLease,
    StableGenerationalRuntime,
};
use pop_runtime_interface::{
    AllocationClass, AllocationSiteDescriptor, ManagedReference, ObjectAllocationRequest,
    ObjectMap, ObjectSlot, RuntimeAllocationSiteId, RuntimeTypeId,
};
use pop_runtime_native_abi::AllocationSiteDescriptorAbi;

use crate::state::{lock_abi_runtime, lock_abi_runtime_raw};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DescriptorFingerprint {
    bubble: u32,
    owner: u32,
    site: u32,
    runtime_type: u32,
    allocation_class: u8,
    reserved: [u8; 3],
    slot_count: u32,
    reference_count: u32,
    reference_slots: usize,
}

impl From<AllocationSiteDescriptorAbi> for DescriptorFingerprint {
    fn from(descriptor: AllocationSiteDescriptorAbi) -> Self {
        Self {
            bubble: descriptor.bubble,
            owner: descriptor.owner,
            site: descriptor.site,
            runtime_type: descriptor.runtime_type,
            allocation_class: descriptor.allocation_class,
            reserved: descriptor.reserved,
            slot_count: descriptor.slot_count,
            reference_count: descriptor.reference_count,
            reference_slots: descriptor.reference_slots as usize,
        }
    }
}

#[derive(Clone)]
struct CachedAllocationSite {
    descriptor_address: usize,
    fingerprint: DescriptorFingerprint,
    request: ObjectAllocationRequest,
}

thread_local! {
    static NATIVE_ALLOCATION_STATE: UnsafeCell<NativeAllocationState> =
        const { UnsafeCell::new(NativeAllocationState::new()) };
}

#[inline]
#[allow(unsafe_code)]
fn with_native_allocation_state<R>(operation: impl FnOnce(&mut NativeAllocationState) -> R) -> R {
    NATIVE_ALLOCATION_STATE.with(|state| {
        // SAFETY: the state is thread-local and every call completes before
        // another runtime operation may re-enter this helper.
        operation(unsafe { &mut *state.get() })
    })
}

static GLOBAL_LOOKUPS: AtomicU64 = AtomicU64::new(0);
static TLAB_REFILLS: AtomicU64 = AtomicU64::new(0);
const TLAB_RESERVATION_COUNT: usize = 2_048;

struct NativeAllocationLease {
    descriptor_address: usize,
    value_count: usize,
    reservations: ReservedMatureLease,
}

struct NativeAllocationState {
    last_site: Option<CachedAllocationSite>,
    lease: Option<NativeAllocationLease>,
    array_store: Option<DirectReferenceStoreAccess>,
    target_validation: Option<DirectReferenceValidation>,
}

impl NativeAllocationState {
    const fn new() -> Self {
        Self {
            last_site: None,
            lease: None,
            array_store: None,
            target_validation: None,
        }
    }

    #[allow(unsafe_code)]
    #[inline]
    fn allocate(
        &mut self,
        descriptor_address: usize,
        initial_values: *const u64,
        value_count: usize,
    ) -> FastAllocation {
        let Some(lease) = self
            .lease
            .as_mut()
            .filter(|lease| lease.descriptor_address == descriptor_address)
        else {
            return FastAllocation::Refill;
        };
        if value_count != lease.value_count || (value_count != 0 && initial_values.is_null()) {
            return FastAllocation::Invalid;
        }
        if lease.reservations.is_empty() {
            return FastAllocation::Refill;
        }
        let values = if value_count == 0 {
            &[]
        } else {
            // SAFETY: the exported ABI contract requires this exact readable
            // initializer array and the cached lease fixes its length.
            unsafe { std::slice::from_raw_parts(initial_values, value_count) }
        };
        let Ok(reference) = lease.reservations.initialize_next(values) else {
            return FastAllocation::FailedReservation;
        };
        FastAllocation::Allocated(reference)
    }

    #[inline]
    fn store_reference(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: ManagedReference,
    ) -> bool {
        let Some(store) = self.array_store.as_ref() else {
            return false;
        };
        if !store.contains(owner) {
            return false;
        }
        self.target_validation
            .as_ref()
            .is_some_and(|target| store.store_buffered(owner, slot, Some((value, target))))
    }
}

enum FastAllocation {
    Allocated(ManagedReference),
    StoreSlow(ManagedReference),
    Refill,
    Invalid,
    FailedReservation,
}

fn allocation_sites() -> &'static Mutex<BTreeMap<RuntimeAllocationSiteId, CachedAllocationSite>> {
    static SITES: OnceLock<Mutex<BTreeMap<RuntimeAllocationSiteId, CachedAllocationSite>>> =
        OnceLock::new();
    SITES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[doc(hidden)]
#[must_use]
pub fn allocation_site_descriptor_count() -> usize {
    allocation_sites().lock().map_or(0, |sites| sites.len())
}

#[doc(hidden)]
#[must_use]
pub fn allocation_site_global_lookup_count() -> u64 {
    GLOBAL_LOOKUPS.load(Ordering::Relaxed)
}

#[doc(hidden)]
#[must_use]
pub fn allocation_site_tlab_refill_count() -> u64 {
    TLAB_REFILLS.load(Ordering::Relaxed)
}

/// Allocates one atomically initialized object from an immutable allocation-site
/// descriptor.
///
/// # Safety
///
/// `descriptor` must address one immutable compiler-emitted descriptor for the
/// process lifetime. Nonzero descriptor counts require readable arrays of the
/// exact declared lengths.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_allocate_initialized_object_at_site(
    descriptor: *const AllocationSiteDescriptorAbi,
    initial_values: *const u64,
    value_count: u64,
) -> u64 {
    // SAFETY: forwarded unchanged from this exported ABI contract.
    unsafe { allocate_initialized_object_at_site(descriptor, initial_values, value_count) }
}

#[allow(unsafe_code)]
pub(crate) unsafe fn allocate_initialized_object_at_site(
    descriptor: *const AllocationSiteDescriptorAbi,
    initial_values: *const u64,
    value_count: u64,
) -> u64 {
    if descriptor.is_null() {
        return 0;
    }
    let descriptor_address = descriptor as usize;
    let Ok(value_count) = usize::try_from(value_count) else {
        return 0;
    };
    match allocate_from_cached_lease(descriptor_address, initial_values, value_count) {
        FastAllocation::Allocated(reference) => return reference.raw(),
        FastAllocation::StoreSlow(_) => unreachable!("ordinary allocation does not store"),
        FastAllocation::Invalid => return 0,
        FastAllocation::FailedReservation => {
            cancel_failed_reservation();
            return 0;
        }
        FastAllocation::Refill => {}
    }
    // SAFETY: forwarded from this function's descriptor and initializer
    // contract after the lock-free lease miss.
    unsafe { allocate_at_site_slow(descriptor, descriptor_address, initial_values, value_count) }
}

/// Allocates one statically described object and installs every declared
/// self-reference before publishing its managed token.
///
/// # Safety
///
/// `descriptor` and `initial_values` follow
/// [`pop_rt_allocate_initialized_object_at_site`]. A nonzero `self_count`
/// requires a readable canonical array of zero-based object slots.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_allocate_initialized_self_referential_object_at_site(
    descriptor: *const AllocationSiteDescriptorAbi,
    initial_values: *const u64,
    value_count: u64,
    self_slots: *const u32,
    self_count: u64,
) -> u64 {
    if descriptor.is_null() {
        return 0;
    }
    let descriptor_address = descriptor as usize;
    let (Ok(value_count), Ok(self_count)) =
        (usize::try_from(value_count), usize::try_from(self_count))
    else {
        return 0;
    };
    if (value_count != 0 && initial_values.is_null()) || self_count == 0 || self_slots.is_null() {
        return 0;
    }
    // SAFETY: the exported contract requires these exact readable arrays.
    let values = unsafe { std::slice::from_raw_parts(initial_values, value_count) };
    // SAFETY: the exported contract requires these exact readable arrays.
    let self_slots = unsafe { std::slice::from_raw_parts(self_slots, self_count) };
    // SAFETY: the exported contract requires one immutable readable
    // descriptor for the process lifetime.
    let Some(request) =
        (unsafe { validate_or_cache_site(descriptor, descriptor_address, value_count) })
    else {
        return 0;
    };
    if self_slots.windows(2).any(|pair| pair[0] >= pair[1])
        || self_slots.iter().copied().any(|slot| {
            slot >= request.object_map().slot_count()
                || !request
                    .object_map()
                    .is_reference_slot(ObjectSlot::new(slot))
                || values.get(slot as usize).copied() != Some(0)
        })
    {
        return 0;
    }
    let slots = self_slots
        .iter()
        .copied()
        .map(ObjectSlot::new)
        .collect::<Vec<_>>();
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .allocate_object_initialized_self_referential(&request, values, &slots)
        .map_or(0, ManagedReference::raw)
}

#[allow(unsafe_code)]
pub(crate) unsafe fn allocate_initialized_object_at_site_and_store_array(
    descriptor: *const AllocationSiteDescriptorAbi,
    initial_values: *const u64,
    value_count: u64,
    array: u64,
    index: u64,
) -> u64 {
    if descriptor.is_null() {
        return 0;
    }
    let Some(slot) = index
        .checked_sub(1)
        .and_then(|index| u32::try_from(index).ok())
        .map(ObjectSlot::new)
    else {
        return 0;
    };
    let descriptor_address = descriptor as usize;
    let Ok(value_count) = usize::try_from(value_count) else {
        return 0;
    };
    let owner = ManagedReference::new(array);
    let fast = with_native_allocation_state(|state| {
        match state.allocate(descriptor_address, initial_values, value_count) {
            FastAllocation::Allocated(reference)
                if state.store_reference(owner, slot, reference) =>
            {
                FastAllocation::Allocated(reference)
            }
            FastAllocation::Allocated(reference) => FastAllocation::StoreSlow(reference),
            other => other,
        }
    });
    let reference = match fast {
        FastAllocation::Allocated(reference) => return reference.raw(),
        FastAllocation::StoreSlow(reference) => reference.raw(),
        FastAllocation::Invalid => return 0,
        FastAllocation::FailedReservation => {
            cancel_failed_reservation();
            return 0;
        }
        FastAllocation::Refill => {
            // SAFETY: forwarded from this function's descriptor and initializer
            // contract after the lock-free lease miss.
            unsafe {
                allocate_at_site_slow(descriptor, descriptor_address, initial_values, value_count)
            }
        }
    };
    if reference == 0 || crate::storage::store_array_value(array, index, reference) == 0 {
        return 0;
    }
    reference
}

#[cold]
#[inline(never)]
#[allow(unsafe_code)]
unsafe fn allocate_at_site_slow(
    descriptor: *const AllocationSiteDescriptorAbi,
    descriptor_address: usize,
    initial_values: *const u64,
    value_count: usize,
) -> u64 {
    if refill_cached_site(descriptor_address).is_ok() {
        return finish_refilled_allocation(descriptor_address, initial_values, value_count);
    }
    // SAFETY: forwarded from this function's immutable descriptor contract.
    let Some(request) =
        (unsafe { validate_or_cache_site(descriptor, descriptor_address, value_count) })
    else {
        return 0;
    };
    if value_count != 0 && initial_values.is_null() {
        return 0;
    }
    let values = if value_count == 0 {
        &[]
    } else {
        // SAFETY: the caller contract requires this exact readable array, and
        // the validated descriptor fixes its length.
        unsafe { std::slice::from_raw_parts(initial_values, value_count) }
    };
    allocate_slow(descriptor_address, &request, values)
}

#[cold]
#[allow(unsafe_code)]
unsafe fn validate_or_cache_site(
    descriptor: *const AllocationSiteDescriptorAbi,
    descriptor_address: usize,
    value_count: usize,
) -> Option<ObjectAllocationRequest> {
    if let Some(request) = cached_site_request(descriptor_address) {
        return (value_count == request.object_map().slot_count() as usize).then_some(request);
    }
    // SAFETY: the caller contract requires one readable immutable descriptor.
    let descriptor = unsafe { *descriptor };
    let fingerprint = DescriptorFingerprint::from(descriptor);
    if value_count != descriptor.slot_count as usize || descriptor.reserved != [0; 3] {
        return None;
    }
    let site = RuntimeAllocationSiteId::new(descriptor.bubble, descriptor.owner, descriptor.site);
    GLOBAL_LOOKUPS.fetch_add(1, Ordering::Relaxed);
    let Ok(mut sites) = allocation_sites().lock() else {
        return None;
    };
    let request = if let Some(cached) = sites.get(&site) {
        if cached.descriptor_address != descriptor_address || cached.fingerprint != fingerprint {
            return None;
        }
        cached.request.clone()
    } else {
        let class = allocation_class(descriptor.allocation_class)?;
        let Ok(reference_count) = usize::try_from(descriptor.reference_count) else {
            return None;
        };
        if reference_count != 0 && descriptor.reference_slots.is_null() {
            return None;
        }
        let reference_slots = if reference_count == 0 {
            &[]
        } else {
            // SAFETY: the caller contract requires this exact immutable array.
            unsafe { std::slice::from_raw_parts(descriptor.reference_slots, reference_count) }
        };
        if reference_slots
            .iter()
            .copied()
            .any(|slot| slot >= descriptor.slot_count)
            || reference_slots.windows(2).any(|pair| pair[0] >= pair[1])
        {
            return None;
        }
        let slots = reference_slots
            .iter()
            .copied()
            .map(ObjectSlot::new)
            .collect();
        let Ok(object_map) = ObjectMap::new(descriptor.slot_count, slots) else {
            return None;
        };
        let layout = AllocationSiteDescriptor::new(
            site,
            RuntimeTypeId::new(descriptor.runtime_type),
            class,
            object_map,
        );
        let cached = CachedAllocationSite {
            descriptor_address,
            fingerprint,
            request: ObjectAllocationRequest::from_descriptor(&layout),
        };
        sites.insert(site, cached.clone());
        cached.request
    };
    drop(sites);
    with_native_allocation_state(|state| {
        state.last_site = Some(CachedAllocationSite {
            descriptor_address,
            fingerprint,
            request: request.clone(),
        });
    });
    Some(request)
}

#[inline]
fn allocate_from_cached_lease(
    descriptor_address: usize,
    initial_values: *const u64,
    value_count: usize,
) -> FastAllocation {
    with_native_allocation_state(|state| {
        state.allocate(descriptor_address, initial_values, value_count)
    })
}

#[cold]
fn refill_cached_site(
    descriptor_address: usize,
) -> Result<(), pop_runtime_interface::RuntimeFailure> {
    let request = cached_site_request(descriptor_address);
    let Some(request) = request else {
        return Err(pop_runtime_interface::RuntimeFailure::runtime_invariant());
    };
    refill_tlab(descriptor_address, &request)
}

#[inline]
fn cached_site_request(descriptor_address: usize) -> Option<ObjectAllocationRequest> {
    with_native_allocation_state(|state| {
        state
            .last_site
            .as_ref()
            .filter(|site| site.descriptor_address == descriptor_address)
            .map(|site| site.request.clone())
    })
}

#[cold]
fn finish_refilled_allocation(
    descriptor_address: usize,
    initial_values: *const u64,
    value_count: usize,
) -> u64 {
    match allocate_from_cached_lease(descriptor_address, initial_values, value_count) {
        FastAllocation::Allocated(reference) => reference.raw(),
        FastAllocation::StoreSlow(_) => unreachable!("ordinary allocation does not store"),
        FastAllocation::FailedReservation => {
            cancel_failed_reservation();
            0
        }
        FastAllocation::Refill | FastAllocation::Invalid => 0,
    }
}

#[cold]
fn allocate_slow(
    _descriptor_address: usize,
    request: &ObjectAllocationRequest,
    values: &[u64],
) -> u64 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .allocate_object_initialized(request, values)
        .map_or(0, ManagedReference::raw)
}

#[cold]
fn cancel_failed_reservation() {
    if let Ok(mut runtime) = lock_abi_runtime_raw() {
        let _ = flush_thread_local_allocations(&mut runtime);
    }
}

fn refill_tlab(
    descriptor_address: usize,
    request: &ObjectAllocationRequest,
) -> Result<(), pop_runtime_interface::RuntimeFailure> {
    if request.allocation_class() != AllocationClass::Mature {
        return Err(pop_runtime_interface::RuntimeFailure::runtime_invariant());
    }
    let mut runtime = lock_abi_runtime_raw()?;
    flush_thread_local_allocations(&mut runtime)?;
    let reservations =
        runtime.reserve_pointer_free_mature_objects(request, TLAB_RESERVATION_COUNT)?;
    let validation = reservations.direct_validation();
    drop(runtime);
    with_native_allocation_state(|state| {
        state.target_validation = Some(validation);
        state.lease = Some(NativeAllocationLease {
            descriptor_address,
            value_count: request.object_map().slot_count() as usize,
            reservations,
        });
    });
    TLAB_REFILLS.fetch_add(1, Ordering::Relaxed);
    Ok(())
}

pub(crate) fn seed_direct_array_reference_store(store: DirectReferenceStoreAccess) {
    with_native_allocation_state(|state| state.array_store = Some(store));
}

pub(crate) fn seed_direct_reference_validation(target: DirectReferenceValidation) {
    with_native_allocation_state(|state| state.target_validation = Some(target));
}

pub(crate) fn cached_direct_reference_store(
    owner: ManagedReference,
    slot: ObjectSlot,
    value: u64,
) -> bool {
    if value == 0 {
        return with_native_allocation_state(|state| {
            state
                .array_store
                .as_ref()
                .filter(|store| store.contains(owner))
                .is_some_and(|store| store.store_buffered(owner, slot, None))
        });
    }
    let value = ManagedReference::new(value);
    with_native_allocation_state(|state| state.store_reference(owner, slot, value))
}

pub(crate) fn cached_direct_reference_store_contains(owner: ManagedReference) -> bool {
    with_native_allocation_state(|state| {
        state
            .array_store
            .as_ref()
            .is_some_and(|store| store.contains(owner))
    })
}

pub(crate) fn quiesce_direct_reference_store() {
    with_native_allocation_state(|state| {
        if let Some(store) = state.array_store.as_ref() {
            store.quiesce();
        }
    });
}

pub(crate) fn flush_thread_local_allocations(
    runtime: &mut StableGenerationalRuntime,
) -> Result<(), pop_runtime_interface::RuntimeFailure> {
    let Some(lease) = with_native_allocation_state(|state| state.lease.take()) else {
        return Ok(());
    };
    runtime.publish_reserved_mature_lease(lease.reservations)
}

const fn allocation_class(raw: u8) -> Option<AllocationClass> {
    Some(match raw {
        0 => AllocationClass::NurseryEligible,
        1 => AllocationClass::Mature,
        2 => AllocationClass::Large,
        3 => AllocationClass::Pinned,
        _ => return None,
    })
}
