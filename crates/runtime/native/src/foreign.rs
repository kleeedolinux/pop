//! Balanced native foreign-call transitions and precise root retention.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, Ordering};

use pop_runtime_collector::MutatorExecutionState;
use pop_runtime_interface::{
    ForeignCallMode, ForeignTransitionId, ManagedReference, RootHandle, RootPublication,
    RuntimeAdapter,
};

use crate::roots::{abi_root_publication, service_root_publication};
use crate::state::{NativeExecutionBinding, current_native_execution_binding, lock_abi_runtime};

static NEXT_FOREIGN_TRANSITION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
struct ForeignTransition {
    id: ForeignTransitionId,
    binding: NativeExecutionBinding,
    publication: RootPublication,
    retained_roots: Vec<Option<RootHandle>>,
}

thread_local! {
    static FOREIGN_TRANSITIONS: RefCell<Vec<ForeignTransition>> = const { RefCell::new(Vec::new()) };
}

/// Enters one statically resolved foreign call after publishing its exact live
/// managed roots. Zero reports a closed native-boundary failure.
///
/// # Safety
///
/// When `root_count` is nonzero, `roots` must address that many writable `u64`
/// managed-reference slots for both the enter and matching leave calls.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_enter_foreign(
    safe_point: u32,
    roots: *mut u64,
    root_count: u64,
    mode: u8,
) -> u64 {
    let Some(mode) = ForeignCallMode::from_raw(mode) else {
        return 0;
    };
    let Some(binding) = current_native_execution_binding() else {
        return 0;
    };
    let Some(roots) = writable_roots(roots, root_count) else {
        return 0;
    };
    let Some(mut publication) = abi_root_publication(safe_point, roots) else {
        return 0;
    };
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    if !service_root_publication(&mut runtime, Some(binding), &mut publication) {
        return 0;
    }
    install_root_values(roots, &publication);

    let retained_roots = match mode {
        ForeignCallMode::Blocking => match retain_publication(&mut runtime, &publication) {
            Some(handles) => handles,
            None => return 0,
        },
        ForeignCallMode::BoundedNonblocking => vec![None; roots.len()],
    };
    let state = match mode {
        ForeignCallMode::Blocking => MutatorExecutionState::HandlesOnly,
        ForeignCallMode::BoundedNonblocking => MutatorExecutionState::BoundedForeign,
    };
    if runtime
        .transition_scheduler_mutator(binding.mutator(), binding.scheduler(), state)
        .is_err()
    {
        release_retained_roots(&mut runtime, &retained_roots);
        return 0;
    }
    let Some(id) = next_transition_id() else {
        let _ = runtime.transition_scheduler_mutator(
            binding.mutator(),
            binding.scheduler(),
            MutatorExecutionState::Managed,
        );
        release_retained_roots(&mut runtime, &retained_roots);
        return 0;
    };
    drop(runtime);
    FOREIGN_TRANSITIONS.with(|transitions| {
        transitions.borrow_mut().push(ForeignTransition {
            id,
            binding,
            publication,
            retained_roots,
        });
    });
    id.raw()
}

/// Leaves the most recent foreign call and restores its exact roots. A zero
/// status leaves token/count mismatches unconsumed for correct cleanup.
///
/// # Safety
///
/// When `root_count` is nonzero, `roots` must address the same writable root
/// slots supplied to the matching enter call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_leave_foreign(
    transition: u64,
    roots: *mut u64,
    root_count: u64,
) -> u8 {
    let Some(id) = ForeignTransitionId::new(transition) else {
        return 0;
    };
    let Some(binding) = current_native_execution_binding() else {
        return 0;
    };
    let Some(roots) = writable_roots(roots, root_count) else {
        return 0;
    };
    let Some(record) = FOREIGN_TRANSITIONS.with(|transitions| {
        transitions
            .borrow()
            .last()
            .filter(|record| {
                record.id == id
                    && record.binding == binding
                    && record.publication.stack_map().root_slots().len() == roots.len()
            })
            .cloned()
    }) else {
        return 0;
    };
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    if runtime
        .transition_scheduler_mutator(
            binding.mutator(),
            binding.scheduler(),
            MutatorExecutionState::Managed,
        )
        .is_err()
    {
        return 0;
    }
    let mut publication = record.publication.clone();
    if !service_root_publication(&mut runtime, Some(binding), &mut publication) {
        return 0;
    }
    install_root_values(roots, &publication);
    if !release_retained_roots(&mut runtime, &record.retained_roots) {
        return 0;
    }
    drop(runtime);
    let consumed = FOREIGN_TRANSITIONS.with(|transitions| {
        transitions
            .borrow_mut()
            .pop()
            .is_some_and(|record| record.id == id)
    });
    u8::from(consumed)
}

fn next_transition_id() -> Option<ForeignTransitionId> {
    let raw = NEXT_FOREIGN_TRANSITION
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .ok()?;
    ForeignTransitionId::new(raw)
}

fn retain_publication(
    runtime: &mut pop_runtime_collector::StableGenerationalRuntime,
    publication: &RootPublication,
) -> Option<Vec<Option<RootHandle>>> {
    let mut retained = Vec::with_capacity(publication.stack_map().root_slots().len());
    for (_, reference) in publication.root_values() {
        let handle = match reference {
            Some(reference) => {
                let Ok(handle) = runtime.retain_root(reference) else {
                    release_retained_roots(runtime, &retained);
                    return None;
                };
                Some(handle)
            }
            None => None,
        };
        retained.push(handle);
    }
    Some(retained)
}

fn release_retained_roots(
    runtime: &mut pop_runtime_collector::StableGenerationalRuntime,
    retained: &[Option<RootHandle>],
) -> bool {
    let mut released = true;
    for root in retained.iter().flatten().copied() {
        released &= runtime.release_root(root).is_ok();
    }
    released
}

fn install_root_values(roots: &mut [u64], publication: &RootPublication) {
    for (root, (_, reference)) in roots.iter_mut().zip(publication.root_values()) {
        *root = reference.map_or(0, ManagedReference::raw);
    }
}

#[allow(unsafe_code)]
fn writable_roots<'a>(roots: *mut u64, root_count: u64) -> Option<&'a mut [u64]> {
    let root_count = usize::try_from(root_count).ok()?;
    if root_count == 0 {
        return Some(&mut []);
    }
    if roots.is_null() {
        return None;
    }
    // SAFETY: The caller provides a writable array with the exact declared
    // count for the duration of the balanced transition operation.
    Some(unsafe { std::slice::from_raw_parts_mut(roots, root_count) })
}
