//! Typed native callback registration and balanced managed entry.

use std::thread::{self, ThreadId};

use pop_runtime_interface::{
    FfiCallbackLifetime, FfiCallbackRegistrationId, FfiCallbackSiteId, FfiCallbackThread,
    FfiCallbackTransitionId, ForeignAddress, ManagedReference, ManagedThreadBindingId, RootHandle,
    RuntimeAdapter, SchedulerId,
};

use crate::binding::{pop_rt_attach_managed_thread, pop_rt_detach_managed_thread};
use crate::foreign::{enter_managed_callback_from_foreign, restore_foreign_after_managed_callback};
use crate::state::{NativeExecutionBinding, current_native_execution_binding, lock_abi_runtime};

mod state;

pub(crate) use state::has_active_callback_transition;
use state::{
    CallbackRegistration, CallbackRestoration, CallbackTransition, callback_registry,
    last_transition, next_context, next_registration_id, next_transition_id, pop_transition,
    push_transition,
};

/// Retains one callback environment and publishes a runtime-owned context.
/// Zero reports failure without changing `out_context`.
///
/// # Safety
///
/// `out_context` must point to one writable `u64` for a successful call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_callback_open(
    environment: u64,
    site: u64,
    scheduler: u32,
    lifetime: u8,
    callback_thread: u8,
    out_context: *mut u64,
) -> u64 {
    if usize::BITS != 64 || out_context.is_null() || scheduler == 0 {
        return 0;
    }
    let Some(site) = FfiCallbackSiteId::new(site) else {
        return 0;
    };
    let Some(lifetime) = FfiCallbackLifetime::from_raw(lifetime) else {
        return 0;
    };
    let Some(callback_thread) = FfiCallbackThread::from_raw(callback_thread) else {
        return 0;
    };
    if lifetime == FfiCallbackLifetime::CallScoped
        && callback_thread != FfiCallbackThread::CallingThread
    {
        return 0;
    }
    let scheduler = SchedulerId::new(scheduler);
    let Some(binding) = current_native_execution_binding() else {
        return 0;
    };
    if binding.scheduler() != scheduler || has_active_callback_transition() {
        return 0;
    }
    let Some(registration) = next_registration_id() else {
        return 0;
    };
    let Some(context) = next_context() else {
        return 0;
    };
    let environment = (environment != 0).then(|| ManagedReference::new(environment));
    let environment = if let Some(environment) = environment {
        let Ok(mut runtime) = lock_abi_runtime() else {
            return 0;
        };
        let Ok(root) = runtime.retain_root(environment) else {
            return 0;
        };
        Some(root)
    } else {
        None
    };
    let record = CallbackRegistration {
        id: registration,
        context,
        site,
        scheduler,
        lifetime,
        thread: callback_thread,
        owner_thread: thread::current().id(),
        environment,
        active: false,
    };
    let Ok(mut registry) = callback_registry().lock() else {
        release_environment(environment);
        return 0;
    };
    if registry
        .registrations
        .insert(context.raw(), record)
        .is_some()
    {
        release_environment(environment);
        return 0;
    }
    drop(registry);
    // SAFETY: The caller promised one writable output slot and all validation
    // plus root retention completed before this single publication.
    unsafe { out_context.write(context.raw()) };
    registration.raw()
}

/// Enters managed execution for one exact callback site/context pair. Zero
/// reports failure without changing `out_environment`.
///
/// # Safety
///
/// `out_environment` must point to one writable `u64` for a successful call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_callback_enter(
    context: u64,
    site: u64,
    out_environment: *mut u64,
) -> u64 {
    if usize::BITS != 64 || out_environment.is_null() || has_active_callback_transition() {
        return 0;
    }
    let Some(context) = ForeignAddress::new(context) else {
        return 0;
    };
    let Some(site) = FfiCallbackSiteId::new(site) else {
        return 0;
    };
    let plan = {
        let Ok(mut registry) = callback_registry().lock() else {
            return 0;
        };
        let Some(registration) = registry.registrations.get_mut(&context.raw()) else {
            return 0;
        };
        if registration.site != site || registration.active {
            return 0;
        }
        if registration.lifetime == FfiCallbackLifetime::CallScoped
            && registration.thread != FfiCallbackThread::CallingThread
        {
            return 0;
        }
        registration.active = true;
        (
            registration.id,
            registration.scheduler,
            registration.thread,
            registration.owner_thread,
            registration.environment,
        )
    };
    let Some(restoration) = establish_managed_entry(plan.1, plan.2, plan.3) else {
        clear_active(context, plan.0);
        return 0;
    };
    let Ok(environment) = resolve_environment(plan.4) else {
        rollback_entry(restoration);
        clear_active(context, plan.0);
        return 0;
    };
    let Some(transition) = next_transition_id() else {
        rollback_entry(restoration);
        clear_active(context, plan.0);
        return 0;
    };
    push_transition(CallbackTransition {
        id: transition,
        registration: plan.0,
        context,
        restoration,
    });
    // SAFETY: The caller promised one writable output slot and the transition
    // is fully established before this single publication.
    unsafe { out_environment.write(environment.map_or(0, ManagedReference::raw)) };
    transition.raw()
}

/// Restores the exact prior execution state and consumes one callback entry.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_ffi_callback_leave(transition: u64) -> u8 {
    let Some(transition) = FfiCallbackTransitionId::new(transition) else {
        return 0;
    };
    let Some(record) = last_transition(transition) else {
        return 0;
    };
    let consumed = pop_transition();
    if !restore_entry(record.restoration) {
        push_transition(record);
        return 0;
    }
    clear_active(record.context, record.registration);
    u8::from(consumed.is_some_and(|consumed| consumed.id == transition))
}

/// Invalidates one registration before releasing its managed environment.
/// Active close fails without consuming the registration.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_ffi_callback_close(registration: u64, context: u64, site: u64) -> u8 {
    let Some(registration) = FfiCallbackRegistrationId::new(registration) else {
        return 0;
    };
    let Some(context) = ForeignAddress::new(context) else {
        return 0;
    };
    let Some(site) = FfiCallbackSiteId::new(site) else {
        return 0;
    };
    let current_scheduler =
        current_native_execution_binding().map(NativeExecutionBinding::scheduler);
    let environment = {
        let Ok(mut registry) = callback_registry().lock() else {
            return 0;
        };
        let Some(record) = registry.registrations.get(&context.raw()) else {
            return 0;
        };
        if record.id != registration
            || record.context != context
            || record.site != site
            || record.active
            || current_scheduler != Some(record.scheduler)
        {
            return 0;
        }
        let Some(record) = registry.registrations.remove(&context.raw()) else {
            return 0;
        };
        record.environment
    };
    u8::from(release_environment(environment))
}

fn establish_managed_entry(
    scheduler: SchedulerId,
    callback_thread: FfiCallbackThread,
    owner_thread: ThreadId,
) -> Option<CallbackRestoration> {
    match callback_thread {
        FfiCallbackThread::CallingThread => {
            if thread::current().id() != owner_thread
                || current_native_execution_binding().map(NativeExecutionBinding::scheduler)
                    != Some(scheduler)
            {
                return None;
            }
            let (binding, state) = enter_managed_callback_from_foreign()?;
            Some(CallbackRestoration::Foreign { binding, state })
        }
        FfiCallbackThread::AttachedThread => {
            if let Some(binding) = current_native_execution_binding() {
                if binding.scheduler() != scheduler {
                    return None;
                }
                let (binding, state) = enter_managed_callback_from_foreign()?;
                return Some(CallbackRestoration::Foreign { binding, state });
            }
            let binding = pop_rt_attach_managed_thread(scheduler.raw());
            Some(CallbackRestoration::Attached {
                binding: ManagedThreadBindingId::new(binding)?,
            })
        }
    }
}

fn resolve_environment(root: Option<RootHandle>) -> Result<Option<ManagedReference>, ()> {
    let Some(root) = root else {
        return Ok(None);
    };
    Ok(Some(
        lock_abi_runtime()
            .map_err(|_| ())?
            .resolve_root(root)
            .map_err(|_| ())?,
    ))
}

fn restore_entry(restoration: CallbackRestoration) -> bool {
    match restoration {
        CallbackRestoration::Foreign { binding, state } => {
            restore_foreign_after_managed_callback(binding, state)
        }
        CallbackRestoration::Attached { binding } => {
            pop_rt_detach_managed_thread(binding.raw()) == 1
        }
    }
}

fn rollback_entry(restoration: CallbackRestoration) {
    let _ = restore_entry(restoration);
}

fn clear_active(context: ForeignAddress, registration: FfiCallbackRegistrationId) {
    if let Ok(mut registry) = callback_registry().lock()
        && let Some(record) = registry.registrations.get_mut(&context.raw())
        && record.id == registration
    {
        record.active = false;
    }
}

fn release_environment(environment: Option<RootHandle>) -> bool {
    let Some(environment) = environment else {
        return true;
    };
    lock_abi_runtime().is_ok_and(|mut runtime| runtime.release_root(environment).is_ok())
}
