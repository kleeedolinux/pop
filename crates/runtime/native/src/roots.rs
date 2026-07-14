//! Native root, pin, safe-point, and barrier exports.

use pop_runtime_interface::{
    ManagedReference, PinHandle, RootHandle, RootPublication, RuntimeAdapter,
};

use crate::state::abi_runtime;

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_retain_root(reference: u64) -> u64 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    runtime
        .retain_root(ManagedReference::new(reference))
        .map_or(0, RootHandle::raw)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_release_root(root: u64) -> u8 {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(runtime.release_root(RootHandle::new(root)).is_ok())
}

/// Registers an opaque scoped pin for a managed handle. Zero signals an
/// invalid reference or a runtime failure at this narrow C boundary.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_pin(reference: u64) -> u64 {
    let Ok(mut runtime) = abi_runtime().lock() else {
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
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    u8::from(runtime.unpin(PinHandle::new(pin)).is_ok())
}

#[must_use]
pub fn request_abi_collection() -> bool {
    let Ok(mut runtime) = abi_runtime().lock() else {
        return false;
    };
    runtime.request_collection();
    true
}

pub fn abi_safe_point(safe_point: u32, roots: &[u64]) -> u8 {
    if roots.len() > u32::MAX as usize {
        return 0;
    }
    let Ok(mut runtime) = abi_runtime().lock() else {
        return 0;
    };
    if !runtime.collection_requested() {
        return u8::from(
            roots
                .iter()
                .all(|root| *root == 0 || runtime.contains(ManagedReference::new(*root))),
        );
    }
    let root_slots = (0..roots.len())
        .filter_map(|index| u32::try_from(index).ok())
        .map(pop_runtime_interface::RootSlot::new)
        .collect();
    let Ok(stack_map) = pop_runtime_interface::StackMap::new(
        pop_runtime_interface::SafePointId::new(safe_point),
        root_slots,
    ) else {
        return 0;
    };
    let roots = roots
        .iter()
        .copied()
        .map(|root| (root != 0).then(|| ManagedReference::new(root)))
        .collect();
    let Ok(mut publication) = RootPublication::new(stack_map, roots) else {
        return 0;
    };
    u8::from(runtime.safe_point(&mut publication).is_ok())
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

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_satb_write_barrier(owner: u64) {
    if let Ok(runtime) = abi_runtime().lock() {
        let _ = runtime.contains(ManagedReference::new(owner));
    }
}
