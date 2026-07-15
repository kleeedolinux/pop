//! Balanced attachment for native threads entering managed execution.

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use pop_runtime_interface::{ManagedThreadBindingId, SchedulerId};

use crate::foreign::has_active_foreign_transition;
use crate::state::{
    NativeExecutionBinding, current_native_execution_binding, enter_native_managed_execution,
    leave_native_managed_execution, register_scheduler_mutator, unregister_scheduler_mutator,
};

static NEXT_MANAGED_BINDING: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AttachedManagedBinding {
    id: ManagedThreadBindingId,
    binding: NativeExecutionBinding,
}

thread_local! {
    static ATTACHED_MANAGED_BINDING: Cell<Option<AttachedManagedBinding>> = const { Cell::new(None) };
}

/// Registers the current native thread as one managed mutator. Zero reports a
/// closed native-boundary failure or duplicate attachment.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_attach_managed_thread(scheduler: u32) -> u64 {
    if scheduler == 0 || current_native_execution_binding().is_some() {
        return 0;
    }
    let scheduler = SchedulerId::new(scheduler);
    let Ok(mutator) = register_scheduler_mutator(scheduler) else {
        return 0;
    };
    if enter_native_managed_execution(scheduler, mutator).is_err() {
        let _ = unregister_scheduler_mutator(scheduler, mutator);
        return 0;
    }
    let Some(id) = next_binding_id() else {
        let _ = leave_native_managed_execution(scheduler, mutator);
        let _ = unregister_scheduler_mutator(scheduler, mutator);
        return 0;
    };
    let Some(binding) = current_native_execution_binding() else {
        let _ = leave_native_managed_execution(scheduler, mutator);
        let _ = unregister_scheduler_mutator(scheduler, mutator);
        return 0;
    };
    ATTACHED_MANAGED_BINDING.with(|attached| {
        attached.set(Some(AttachedManagedBinding { id, binding }));
    });
    id.raw()
}

/// Detaches and unregisters the exact current attachment. Active foreign
/// transitions and stale or wrong-thread identities fail without consuming it.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_detach_managed_thread(binding: u64) -> u8 {
    let Some(id) = ManagedThreadBindingId::new(binding) else {
        return 0;
    };
    if has_active_foreign_transition() {
        return 0;
    }
    let Some(attached) = ATTACHED_MANAGED_BINDING.with(Cell::get) else {
        return 0;
    };
    if attached.id != id || current_native_execution_binding() != Some(attached.binding) {
        return 0;
    }
    if leave_native_managed_execution(attached.binding.scheduler(), attached.binding.mutator())
        .is_err()
    {
        return 0;
    }
    if unregister_scheduler_mutator(attached.binding.scheduler(), attached.binding.mutator())
        .is_err()
    {
        return 0;
    }
    ATTACHED_MANAGED_BINDING.with(|current| current.set(None));
    1
}

fn next_binding_id() -> Option<ManagedThreadBindingId> {
    let raw = NEXT_MANAGED_BINDING
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .ok()?;
    ManagedThreadBindingId::new(raw)
}
