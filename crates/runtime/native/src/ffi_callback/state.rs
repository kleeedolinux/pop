use std::cell::RefCell;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread::ThreadId;

use pop_runtime_collector::MutatorExecutionState;
use pop_runtime_interface::{
    FfiCallbackLifetime, FfiCallbackRegistrationId, FfiCallbackSiteId, FfiCallbackThread,
    FfiCallbackTransitionId, ForeignAddress, ManagedThreadBindingId, RootHandle, SchedulerId,
};

use crate::state::NativeExecutionBinding;

static NEXT_CALLBACK_REGISTRATION: AtomicU64 = AtomicU64::new(1);
static NEXT_CALLBACK_CONTEXT: AtomicU64 = AtomicU64::new(0x1_0000_0000);
static NEXT_CALLBACK_TRANSITION: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
pub(super) struct CallbackRegistration {
    pub(super) id: FfiCallbackRegistrationId,
    pub(super) context: ForeignAddress,
    pub(super) site: FfiCallbackSiteId,
    pub(super) scheduler: SchedulerId,
    pub(super) lifetime: FfiCallbackLifetime,
    pub(super) thread: FfiCallbackThread,
    pub(super) owner_thread: ThreadId,
    pub(super) environment: Option<RootHandle>,
    pub(super) active: bool,
}

#[derive(Default)]
pub(super) struct CallbackRegistry {
    pub(super) registrations: BTreeMap<u64, CallbackRegistration>,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum CallbackRestoration {
    Foreign {
        binding: NativeExecutionBinding,
        state: MutatorExecutionState,
    },
    Attached {
        binding: ManagedThreadBindingId,
    },
}

#[derive(Clone, Copy, Debug)]
pub(super) struct CallbackTransition {
    pub(super) id: FfiCallbackTransitionId,
    pub(super) registration: FfiCallbackRegistrationId,
    pub(super) context: ForeignAddress,
    pub(super) restoration: CallbackRestoration,
}

thread_local! {
    static CALLBACK_TRANSITIONS: RefCell<Vec<CallbackTransition>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn callback_registry() -> &'static Mutex<CallbackRegistry> {
    static REGISTRY: OnceLock<Mutex<CallbackRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(CallbackRegistry::default()))
}

pub(crate) fn has_active_callback_transition() -> bool {
    CALLBACK_TRANSITIONS.with(|transitions| !transitions.borrow().is_empty())
}

pub(super) fn last_transition(id: FfiCallbackTransitionId) -> Option<CallbackTransition> {
    CALLBACK_TRANSITIONS.with(|transitions| {
        transitions
            .borrow()
            .last()
            .filter(|record| record.id == id)
            .copied()
    })
}

pub(super) fn push_transition(transition: CallbackTransition) {
    CALLBACK_TRANSITIONS.with(|transitions| transitions.borrow_mut().push(transition));
}

pub(super) fn pop_transition() -> Option<CallbackTransition> {
    CALLBACK_TRANSITIONS.with(|transitions| transitions.borrow_mut().pop())
}

pub(super) fn next_registration_id() -> Option<FfiCallbackRegistrationId> {
    next_nonzero(&NEXT_CALLBACK_REGISTRATION).and_then(FfiCallbackRegistrationId::new)
}

pub(super) fn next_context() -> Option<ForeignAddress> {
    next_nonzero(&NEXT_CALLBACK_CONTEXT).and_then(ForeignAddress::new)
}

pub(super) fn next_transition_id() -> Option<FfiCallbackTransitionId> {
    next_nonzero(&NEXT_CALLBACK_TRANSITION).and_then(FfiCallbackTransitionId::new)
}

fn next_nonzero(counter: &AtomicU64) -> Option<u64> {
    counter
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
            current.checked_add(1)
        })
        .ok()
}
