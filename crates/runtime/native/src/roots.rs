//! Native root, pin, safe-point, and barrier exports.

use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::{
    ManagedReference, PinHandle, RootHandle, RootPublication, RuntimeAdapter,
};

use crate::state::{
    NativeExecutionBinding, current_native_execution_binding, epoch_telemetry, lock_abi_runtime,
};

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_retain_root(reference: u64) -> u64 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .retain_root(ManagedReference::new(reference))
        .map_or(0, RootHandle::raw)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_release_root(root: u64) -> u8 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    u8::from(runtime.release_root(RootHandle::new(root)).is_ok())
}

/// Registers an opaque scoped pin for a managed handle. Zero signals an
/// invalid reference or a runtime failure at this narrow C boundary.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_pin(reference: u64) -> u64 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .pin(ManagedReference::new(reference))
        .map_or(0, PinHandle::raw)
}

/// Releases an opaque scoped pin. A pin handle is single-use.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_unpin(pin: u64) -> u8 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    u8::from(runtime.unpin(PinHandle::new(pin)).is_ok())
}

#[must_use]
pub fn request_abi_collection() -> bool {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return false;
    };
    runtime.request_collection();
    true
}

#[must_use]
pub fn abi_safe_point(safe_point: u32, roots: &[u64]) -> u8 {
    if roots.len() > u32::MAX as usize {
        return 0;
    }
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let binding = current_native_execution_binding();
    if binding.is_none() && !runtime.collection_requested() {
        return u8::from(
            roots
                .iter()
                .all(|root| *root == 0 || runtime.contains(ManagedReference::new(*root))),
        );
    }
    let Some(mut publication) = abi_root_publication(safe_point, roots) else {
        return 0;
    };
    let serviced = service_root_publication(&mut runtime, binding, &mut publication);
    let releases = if serviced {
        crate::task::prune_collected_task_state(&mut runtime)
    } else {
        Vec::new()
    };
    drop(runtime);
    crate::task::release_pruned_scheduler_tasks(releases);
    u8::from(serviced)
}

fn abi_root_publication(safe_point: u32, roots: &[u64]) -> Option<RootPublication> {
    let root_slots = (0..roots.len())
        .filter_map(|index| u32::try_from(index).ok())
        .map(pop_runtime_interface::RootSlot::new)
        .collect();
    let Ok(stack_map) = pop_runtime_interface::StackMap::new(
        pop_runtime_interface::SafePointId::new(safe_point),
        root_slots,
    ) else {
        return None;
    };
    let roots = roots
        .iter()
        .copied()
        .map(|root| (root != 0).then(|| ManagedReference::new(root)))
        .collect();
    RootPublication::new(stack_map, roots).ok()
}

fn service_root_publication(
    runtime: &mut StableGenerationalRuntime,
    binding: Option<NativeExecutionBinding>,
    publication: &mut RootPublication,
) -> bool {
    if let Some(binding) = binding {
        runtime
            .scheduler_mutator_safe_point(binding.mutator(), binding.scheduler(), publication)
            .is_ok()
    } else {
        runtime.safe_point(publication).is_ok()
    }
}

fn abi_safe_point_v2(safe_point: u32, roots: &mut [u64]) -> u8 {
    if roots.len() > u32::MAX as usize {
        return 0;
    }
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let binding = current_native_execution_binding();
    if binding.is_none() && !runtime.collection_requested() {
        return u8::from(
            roots
                .iter()
                .all(|root| *root == 0 || runtime.contains(ManagedReference::new(*root))),
        );
    }
    let Some(mut publication) = abi_root_publication(safe_point, roots) else {
        return 0;
    };
    if !service_root_publication(&mut runtime, binding, &mut publication)
        || publication.stack_map().root_slots().len() != roots.len()
    {
        return 0;
    }
    for ((_, relocated), root) in publication.root_values().zip(roots.iter_mut()) {
        *root = relocated.map_or(0, ManagedReference::raw);
    }
    let releases = crate::task::prune_collected_task_state(&mut runtime);
    drop(runtime);
    crate::task::release_pruned_scheduler_tasks(releases);
    1
}

/// Publishes exact live roots through the ABI 2 writable-root transition.
///
/// This typed Rust entry is used by the native scheduler integration and its
/// conformance tests. The stable facade may leave tokens unchanged, but it
/// still validates the managed worker binding, participates in the current
/// collector epoch, and writes the complete publication back atomically.
#[must_use]
pub fn abi_safe_point_writable(safe_point: u32, roots: &mut [u64]) -> u8 {
    abi_safe_point_v2(safe_point, roots)
}

#[must_use]
pub fn native_epoch_telemetry() -> pop_runtime_collector::EpochCoordinatorTelemetry {
    epoch_telemetry()
}

/// Publishes exact live managed handles for one native safe point.
///
/// # Safety
///
/// When `root_count` is nonzero, `roots` must address that many readable `u64`
/// managed handles for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_gc_safe_point(
    safe_point: u32,
    roots: *const u64,
    root_count: u64,
) -> u8 {
    let Ok(root_count) = usize::try_from(root_count) else {
        return 0;
    };
    if root_count == 0 {
        return abi_safe_point(safe_point, &[]);
    }
    if roots.is_null() {
        return 0;
    }
    // SAFETY: The backend passes a stack array containing the declared number
    // of live managed handles.
    let roots = unsafe { std::slice::from_raw_parts(roots, root_count) };
    abi_safe_point(safe_point, roots)
}

/// Publishes and reloads exact live managed tokens for one ABI 2 safe point.
///
/// # Safety
///
/// When `root_count` is nonzero, `roots` must address that many writable `u64`
/// managed-reference slots for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_gc_safe_point_v2(
    safe_point: u32,
    roots: *mut u64,
    root_count: u64,
) -> u8 {
    let Ok(root_count) = usize::try_from(root_count) else {
        return 0;
    };
    if root_count == 0 {
        return abi_safe_point_v2(safe_point, &mut []);
    }
    if roots.is_null() {
        return 0;
    }
    // SAFETY: The ABI 2 backend passes a writable stack array containing the
    // declared number of exact live managed-reference tokens.
    let roots = unsafe { std::slice::from_raw_parts_mut(roots, root_count) };
    abi_safe_point_v2(safe_point, roots)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_satb_write_barrier(owner: u64) {
    if let Ok(runtime) = lock_abi_runtime() {
        let _ = runtime.contains(ManagedReference::new(owner));
    }
}
