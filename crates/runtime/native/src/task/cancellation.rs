#[derive(Default)]
struct CancellationRegistry {
    sources: BTreeMap<u64, u64>,
    tokens: BTreeMap<u64, bool>,
}

fn cancellation_registry() -> &'static Mutex<CancellationRegistry> {
    static REGISTRY: OnceLock<Mutex<CancellationRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(CancellationRegistry::default()))
}

pub(crate) fn prune_collected_task_state(
    runtime: &mut StableGenerationalRuntime,
) -> Vec<crate::SchedulerTaskId> {
    let mut scheduler_releases = Vec::new();
    if let Ok(mut registry) = native_task_registry().lock() {
        let removed = registry
            .tasks
            .iter()
            .filter_map(|(handle, task)| {
                (!runtime.contains(ManagedReference::new(*handle))
                    && (task.lifecycle.state() == TaskState::Created
                        || task.lifecycle.state().terminal()))
                .then_some(*handle)
            })
            .collect::<Vec<_>>();
        for handle in removed {
            if let Some(task) = registry.tasks.remove(&handle) {
                for root in task.cold_roots {
                    let _ = runtime.release_root(RootHandle::new(root));
                }
                if let Some(scheduler_task) = task.scheduler_task {
                    scheduler_releases.push(scheduler_task);
                }
            }
        }
        registry.groups.retain(|handle, group| {
            runtime.contains(ManagedReference::new(*handle)) || !group.children.is_empty()
        });
    }
    if let Ok(mut registry) = cancellation_registry().lock() {
        registry
            .sources
            .retain(|source, _| runtime.contains(ManagedReference::new(*source)));
        registry
            .tokens
            .retain(|token, _| runtime.contains(ManagedReference::new(*token)));
    }
    scheduler_releases
}

pub(crate) fn release_pruned_scheduler_tasks(tasks: Vec<crate::SchedulerTaskId>) {
    for task in tasks {
        let _ = native_task_scheduler().release_terminal_task(task);
    }
}

/// Awaits one scalar task handle at the native ABI boundary.
///
/// The current bootstrap representation stores scalar task completion directly
/// in the handle. A full coroutine scheduler can replace this boundary without
/// changing generated native call sites.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_suspend(task: u64) -> u64 {
    if task == 0 {
        pop_rt_trap();
    }
    task
}

/// Resumes one task handle and returns its scalar completion.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_resume(task: u64) -> u64 {
    pop_rt_suspend(task)
}

/// Creates one cancellation authority and its distinct immutable token.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_cancel_source_create() -> u64 {
    let token = crate::pop_rt_allocate_object(0);
    if token == 0 {
        return 0;
    }
    let roots = [0_u32];
    let source = unsafe { crate::pop_rt_allocate_mapped_object(1, roots.as_ptr(), 1) };
    if source == 0 || crate::pop_rt_field_set(source, 1, token) == 0 {
        return 0;
    }
    let Ok(mut registry) = cancellation_registry().lock() else {
        return 0;
    };
    registry.tokens.insert(token, false);
    registry.sources.insert(source, token);
    source
}

/// Returns the immutable token owned by one exact cancellation source.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_cancel_source_token(source: u64) -> u64 {
    cancellation_registry()
        .lock()
        .ok()
        .and_then(|registry| registry.sources.get(&source).copied())
        .unwrap_or(0)
}

/// Releases cancellation authority without invalidating copied tokens.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_cancel_source_release(source: u64) -> u8 {
    cancellation_registry()
        .lock()
        .ok()
        .is_some_and(|mut registry| registry.sources.remove(&source).is_some())
        .into()
}

/// Releases one retained immutable cancellation token.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_cancel_token_release(token: u64) -> u8 {
    cancellation_registry()
        .lock()
        .ok()
        .is_some_and(|mut registry| registry.tokens.remove(&token).is_some())
        .into()
}

/// Requests cancellation through an owning source, never through its token.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_cancel(source: u64) -> u8 {
    let token = {
        let Ok(mut registry) = cancellation_registry().lock() else {
            return 0;
        };
        let Some(token) = registry.sources.get(&source).copied() else {
            return 0;
        };
        let Some(requested) = registry.tokens.get_mut(&token) else {
            return 0;
        };
        *requested = true;
        token
    };
    let scheduler_tasks = native_task_registry()
        .lock()
        .ok()
        .map(|mut tasks| {
            tasks
                .tasks
                .values_mut()
                .filter_map(|task| {
                    (task.lifecycle.cancellation_token() == Some(CancellationTokenId::new(token))
                        && !task.lifecycle.state().terminal())
                    .then(|| {
                        let _ = task
                            .lifecycle
                            .request_cancellation(CancellationTokenId::new(token));
                        task.scheduler_task
                    })
                    .flatten()
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    for task in scheduler_tasks {
        let _ = native_task_scheduler().request_cancellation(task);
    }
    1
}

/// Returns whether a cancellation token has been cancelled.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_cancellation_requested(token: u64) -> u8 {
    cancellation_registry()
        .lock()
        .ok()
        .and_then(|registry| registry.tokens.get(&token).copied())
        .unwrap_or(false)
        .into()
}
