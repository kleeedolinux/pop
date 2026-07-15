//! Process-global native stable-generational composition state.

use std::cell::Cell;
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use pop_runtime_collector::{
    EpochCoordinatorTelemetry, MutatorExecutionState, MutatorId, StableGenerationalRuntime,
    TaskFrameRootError,
};
use pop_runtime_interface::{
    ArrayElementMap, RootPublication, RuntimeFailure, SchedulerId, StackMap, TaskFrameRootId,
};

static ABI_RUNTIME: OnceLock<Mutex<StableGenerationalRuntime>> = OnceLock::new();
static ABI_TABLES: OnceLock<Mutex<BTreeMap<u64, TableMetadata>>> = OnceLock::new();
static ABI_LISTS: OnceLock<Mutex<BTreeMap<u64, ListMetadata>>> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct NativeExecutionBinding {
    scheduler: SchedulerId,
    mutator: MutatorId,
}

thread_local! {
    static NATIVE_EXECUTION_BINDING: Cell<Option<NativeExecutionBinding>> = const { Cell::new(None) };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct TableMetadata {
    pub(crate) length: u32,
    pub(crate) capacity: u32,
    pub(crate) key_map: ArrayElementMap,
    pub(crate) value_map: ArrayElementMap,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ListMetadata {
    pub(crate) length: u32,
    pub(crate) capacity: u32,
    pub(crate) element_map: ArrayElementMap,
}

pub(crate) fn abi_runtime() -> &'static Mutex<StableGenerationalRuntime> {
    ABI_RUNTIME.get_or_init(|| Mutex::new(StableGenerationalRuntime::new()))
}

pub(crate) fn lock_abi_runtime()
-> Result<MutexGuard<'static, StableGenerationalRuntime>, RuntimeFailure> {
    let mut runtime = abi_runtime()
        .lock()
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    let scheduler = current_native_execution_binding()
        .map_or(SchedulerId::new(1), NativeExecutionBinding::scheduler);
    runtime.select_scheduler(scheduler);
    Ok(runtime)
}

pub(crate) fn register_scheduler_mutator(
    scheduler: SchedulerId,
) -> Result<MutatorId, RuntimeFailure> {
    let mut runtime = abi_runtime()
        .lock()
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    runtime
        .register_scheduler_mutator(scheduler, MutatorExecutionState::Detached)
        .map_err(|_| RuntimeFailure::runtime_invariant())
}

pub(crate) fn enter_native_managed_execution(
    scheduler: SchedulerId,
    mutator: MutatorId,
) -> Result<(), RuntimeFailure> {
    if current_native_execution_binding().is_some() {
        return Err(RuntimeFailure::runtime_invariant());
    }
    let mut runtime = abi_runtime()
        .lock()
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    runtime
        .transition_scheduler_mutator(mutator, scheduler, MutatorExecutionState::Managed)
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    NATIVE_EXECUTION_BINDING.with(|binding| {
        binding.set(Some(NativeExecutionBinding { scheduler, mutator }));
    });
    Ok(())
}

pub(crate) fn leave_native_managed_execution(
    scheduler: SchedulerId,
    mutator: MutatorId,
) -> Result<(), RuntimeFailure> {
    let binding = NATIVE_EXECUTION_BINDING.with(|binding| binding.replace(None));
    if binding != Some(NativeExecutionBinding { scheduler, mutator }) {
        return Err(RuntimeFailure::runtime_invariant());
    }
    let mut runtime = abi_runtime()
        .lock()
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    runtime
        .transition_scheduler_mutator(mutator, scheduler, MutatorExecutionState::Detached)
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    Ok(())
}

pub(crate) fn clear_native_execution_binding() {
    NATIVE_EXECUTION_BINDING.with(|binding| binding.set(None));
}

pub(crate) fn unregister_scheduler_mutator(
    scheduler: SchedulerId,
    mutator: MutatorId,
) -> Result<(), RuntimeFailure> {
    let mut runtime = abi_runtime()
        .lock()
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    runtime
        .unregister_scheduler_mutator(mutator, scheduler)
        .map_err(|_| RuntimeFailure::runtime_invariant())
}

pub(crate) fn current_native_execution_binding() -> Option<NativeExecutionBinding> {
    NATIVE_EXECUTION_BINDING.with(Cell::get)
}

impl NativeExecutionBinding {
    pub(crate) const fn scheduler(self) -> SchedulerId {
        self.scheduler
    }

    pub(crate) const fn mutator(self) -> MutatorId {
        self.mutator
    }
}

pub(crate) fn epoch_telemetry() -> EpochCoordinatorTelemetry {
    abi_runtime()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .epoch_coordinator_telemetry()
}

#[cfg(test)]
pub(crate) fn allocation_scheduler(
    reference: pop_runtime_interface::ManagedReference,
) -> Option<SchedulerId> {
    lock_abi_runtime().ok()?.allocation_scheduler(reference)
}

pub(crate) fn abi_tables() -> &'static Mutex<BTreeMap<u64, TableMetadata>> {
    ABI_TABLES.get_or_init(|| Mutex::new(BTreeMap::new()))
}

pub(crate) fn abi_lists() -> &'static Mutex<BTreeMap<u64, ListMetadata>> {
    ABI_LISTS.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn task_root_runtime()
-> Result<std::sync::MutexGuard<'static, StableGenerationalRuntime>, TaskFrameRootError> {
    abi_runtime()
        .lock()
        .map_err(|_| TaskFrameRootError::Runtime(RuntimeFailure::runtime_invariant()))
}

pub(crate) fn retain_scheduler_task_roots(
    scheduler: SchedulerId,
    publication: &RootPublication,
) -> Result<TaskFrameRootId, TaskFrameRootError> {
    task_root_runtime()?.retain_task_frame_roots(scheduler, publication)
}

pub(crate) fn prepare_scheduler_task_root_restore(
    identity: TaskFrameRootId,
    scheduler: SchedulerId,
    expected: &StackMap,
) -> Result<RootPublication, TaskFrameRootError> {
    task_root_runtime()?.prepare_task_frame_root_restore(identity, scheduler, expected)
}

pub(crate) fn complete_scheduler_task_root_restore(
    identity: TaskFrameRootId,
    scheduler: SchedulerId,
) -> Result<(), TaskFrameRootError> {
    task_root_runtime()?.complete_task_frame_root_restore(identity, scheduler)
}

pub(crate) fn release_scheduler_task_roots(
    identity: TaskFrameRootId,
    scheduler: SchedulerId,
) -> Result<(), TaskFrameRootError> {
    task_root_runtime()?.release_task_frame_roots(identity, scheduler)
}

pub(crate) fn transfer_scheduler_task_roots(
    identity: TaskFrameRootId,
    from: SchedulerId,
    to: SchedulerId,
) -> Result<(), TaskFrameRootError> {
    task_root_runtime()?.transfer_task_frame_roots(identity, from, to)
}
