//! Process-global native stable-generational composition state.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

use pop_runtime_collector::{StableGenerationalRuntime, TaskFrameRootError};
use pop_runtime_interface::{
    ArrayElementMap, RootPublication, RuntimeFailure, SchedulerId, StackMap, TaskFrameRootId,
};

static ABI_RUNTIME: OnceLock<Mutex<StableGenerationalRuntime>> = OnceLock::new();
static ABI_TABLES: OnceLock<Mutex<BTreeMap<u64, TableMetadata>>> = OnceLock::new();
static ABI_LISTS: OnceLock<Mutex<BTreeMap<u64, ListMetadata>>> = OnceLock::new();

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
