//! Checked thread-local page-span access for native reads.

use std::cell::{RefCell, UnsafeCell};
use std::sync::atomic::{AtomicU64, Ordering};

use pop_runtime_collector::{
    DirectPageAccess, DirectReferenceStoreAccess, DirectReferenceValidation,
};
use pop_runtime_interface::{ManagedReference, ObjectSlot};

use crate::state::lock_abi_runtime;

thread_local! {
    static DIRECT_OBJECT_PAGE: RefCell<Option<DirectPageAccess>> = const { RefCell::new(None) };
    static DIRECT_ARRAY_PAGE: RefCell<Option<DirectPageAccess>> = const { RefCell::new(None) };
    static DIRECT_ADJACENT_READ: UnsafeCell<DirectAdjacentReadCache> =
        const { UnsafeCell::new(DirectAdjacentReadCache::new()) };
}

#[inline]
#[allow(unsafe_code)]
fn with_direct_adjacent_read<R>(operation: impl FnOnce(&mut DirectAdjacentReadCache) -> R) -> R {
    DIRECT_ADJACENT_READ.with(|cached| {
        // SAFETY: the cache is thread-local and adjacent reads do not re-enter
        // the native storage adapter while this operation is active.
        operation(unsafe { &mut *cached.get() })
    })
}

struct DirectAdjacentReadCache {
    array: Option<DirectPageAccess>,
    object: Option<DirectPageAccess>,
}

impl DirectAdjacentReadCache {
    const fn new() -> Self {
        Self {
            array: None,
            object: None,
        }
    }
}

static DIRECT_PAGE_MISSES: AtomicU64 = AtomicU64::new(0);
static DIRECT_PAGE_MUTATION_MISSES: AtomicU64 = AtomicU64::new(0);
static DIRECT_REFERENCE_MUTATION_MISSES: AtomicU64 = AtomicU64::new(0);

pub(super) enum CachedReferenceStore {
    Stored,
    OwnerMiss,
    Rejected,
}

enum CachedAdjacentRead {
    Value(u64),
    Target(ManagedReference),
    Miss,
}

#[doc(hidden)]
#[must_use]
pub fn direct_page_access_miss_count() -> u64 {
    DIRECT_PAGE_MISSES.load(Ordering::Relaxed)
}

#[doc(hidden)]
#[must_use]
pub fn direct_page_mutation_miss_count() -> u64 {
    DIRECT_PAGE_MUTATION_MISSES.load(Ordering::Relaxed)
}

#[doc(hidden)]
#[must_use]
pub fn direct_reference_mutation_miss_count() -> u64 {
    DIRECT_REFERENCE_MUTATION_MISSES.load(Ordering::Relaxed)
}

enum ScalarStore {
    Stored,
    SlowPath,
    Miss,
}

fn try_scalar_store(
    access: &DirectPageAccess,
    reference: ManagedReference,
    slot: ObjectSlot,
    value: u64,
) -> ScalarStore {
    match access.slot_is_scalar(reference, slot) {
        Some(true) if access.store_scalar(reference, slot, value) => ScalarStore::Stored,
        Some(true) | None => ScalarStore::Miss,
        Some(false) => ScalarStore::SlowPath,
    }
}

pub(super) fn direct_page_store_scalar(
    reference: ManagedReference,
    slot: ObjectSlot,
    array: bool,
    value: u64,
) -> bool {
    let cached = if array {
        DIRECT_ARRAY_PAGE.with(|cached| {
            cached
                .borrow()
                .as_ref()
                .map(|access| try_scalar_store(access, reference, slot, value))
        })
    } else {
        DIRECT_OBJECT_PAGE.with(|cached| {
            cached
                .borrow()
                .as_ref()
                .map(|access| try_scalar_store(access, reference, slot, value))
        })
    };
    match cached {
        Some(ScalarStore::Stored) => return true,
        Some(ScalarStore::SlowPath) => return false,
        Some(ScalarStore::Miss) | None => {}
    }

    DIRECT_PAGE_MUTATION_MISSES.fetch_add(1, Ordering::Relaxed);
    let Ok(runtime) = lock_abi_runtime() else {
        return false;
    };
    let Some(access) = (if array {
        runtime.direct_array_page_access(reference)
    } else {
        runtime.direct_object_page_access(reference)
    }) else {
        return false;
    };
    let stored = matches!(
        try_scalar_store(&access, reference, slot, value),
        ScalarStore::Stored
    );
    if array {
        DIRECT_ARRAY_PAGE.with(|cached| *cached.borrow_mut() = Some(access));
    } else {
        DIRECT_OBJECT_PAGE.with(|cached| *cached.borrow_mut() = Some(access));
    }
    stored
}

#[inline]
pub(super) fn direct_page_store_array_reference_cached(
    owner: ManagedReference,
    slot: ObjectSlot,
    value: u64,
) -> CachedReferenceStore {
    if !crate::allocation::cached_direct_reference_store_contains(owner) {
        return CachedReferenceStore::OwnerMiss;
    }
    if crate::allocation::cached_direct_reference_store(owner, slot, value) {
        CachedReferenceStore::Stored
    } else {
        CachedReferenceStore::Rejected
    }
}

pub(super) fn direct_page_store_array_reference_slow(
    owner: ManagedReference,
    slot: ObjectSlot,
    value: u64,
) -> bool {
    DIRECT_REFERENCE_MUTATION_MISSES.fetch_add(1, Ordering::Relaxed);
    let Ok(runtime) = lock_abi_runtime() else {
        return false;
    };
    let Some(store) = runtime.direct_array_reference_store_access(owner) else {
        return false;
    };
    let target = (value != 0)
        .then(|| runtime.direct_reference_validation(ManagedReference::new(value)))
        .flatten();
    let stored = try_reference_store(Some(&store), target.as_ref(), owner, slot, value);
    drop(runtime);
    crate::allocation::seed_direct_array_reference_store(store);
    if let Some(target) = target {
        crate::allocation::seed_direct_reference_validation(target);
    }
    stored
}

#[inline]
pub(super) fn direct_array_object_field_value(
    owner: ManagedReference,
    owner_slot: ObjectSlot,
    field_slot: ObjectSlot,
) -> Option<u64> {
    let cached = with_direct_adjacent_read(|cached| {
        let Some(target) = cached
            .array
            .as_ref()
            .and_then(|access| access.load(owner, owner_slot))
            .filter(|target| *target != 0)
            .map(ManagedReference::new)
        else {
            return CachedAdjacentRead::Miss;
        };
        if let Some(value) = cached
            .object
            .as_ref()
            .and_then(|access| access.load(target, field_slot))
        {
            CachedAdjacentRead::Value(value)
        } else {
            CachedAdjacentRead::Target(target)
        }
    });
    match cached {
        CachedAdjacentRead::Value(value) => return Some(value),
        CachedAdjacentRead::Target(target) => {
            DIRECT_PAGE_MISSES.fetch_add(1, Ordering::Relaxed);
            let runtime = lock_abi_runtime().ok()?;
            let object = runtime.direct_object_page_access(target)?;
            let value = object.load(target, field_slot)?;
            drop(runtime);
            with_direct_adjacent_read(|cached| cached.object = Some(object));
            return Some(value);
        }
        CachedAdjacentRead::Miss => {}
    }

    DIRECT_PAGE_MISSES.fetch_add(2, Ordering::Relaxed);
    let runtime = lock_abi_runtime().ok()?;
    let array = runtime.direct_array_page_access(owner)?;
    let target = ManagedReference::new(array.load(owner, owner_slot)?);
    if target.raw() == 0 {
        return None;
    }
    let object = runtime.direct_object_page_access(target)?;
    let value = object.load(target, field_slot)?;
    drop(runtime);
    with_direct_adjacent_read(|cached| {
        *cached = DirectAdjacentReadCache {
            array: Some(array),
            object: Some(object),
        };
    });
    Some(value)
}

pub(crate) fn seed_direct_array_reference_store(store: DirectReferenceStoreAccess) {
    crate::allocation::seed_direct_array_reference_store(store);
}

pub(crate) fn quiesce_direct_accesses() {
    crate::allocation::quiesce_direct_reference_store();
}

#[inline]
fn try_reference_store(
    store: Option<&DirectReferenceStoreAccess>,
    target: Option<&DirectReferenceValidation>,
    owner: ManagedReference,
    slot: ObjectSlot,
    value: u64,
) -> bool {
    let Some(store) = store else {
        return false;
    };
    if value == 0 {
        return store.store_buffered(owner, slot, None);
    }
    let reference = ManagedReference::new(value);
    target.is_some_and(|target| store.store_buffered(owner, slot, Some((reference, target))))
}

#[inline]
pub(super) fn direct_page_value(
    reference: ManagedReference,
    slot: ObjectSlot,
    array: bool,
) -> Option<u64> {
    let cached = if array {
        DIRECT_ARRAY_PAGE.with(|cached| {
            cached
                .borrow()
                .as_ref()
                .and_then(|access| access.load(reference, slot))
        })
    } else {
        DIRECT_OBJECT_PAGE.with(|cached| {
            cached
                .borrow()
                .as_ref()
                .and_then(|access| access.load(reference, slot))
        })
    };
    if cached.is_some() {
        return cached;
    }

    DIRECT_PAGE_MISSES.fetch_add(1, Ordering::Relaxed);
    let runtime = lock_abi_runtime().ok()?;
    let access = if array {
        runtime.direct_array_page_access(reference)?
    } else {
        runtime.direct_object_page_access(reference)?
    };
    let value = access.load(reference, slot)?;
    if array {
        DIRECT_ARRAY_PAGE.with(|cached| *cached.borrow_mut() = Some(access));
    } else {
        DIRECT_OBJECT_PAGE.with(|cached| *cached.borrow_mut() = Some(access));
    }
    Some(value)
}
