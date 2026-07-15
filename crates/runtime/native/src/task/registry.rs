pub type NativeTaskPollCallback = extern "C" fn(u64, u64, u8) -> u8;

struct NativeAbiTask {
    lifecycle: TaskLifecycle,
    frame: Option<NativeTaskFrame>,
    cold_roots: Vec<u64>,
    callback: NativeTaskPollCallback,
    scheduler_task: Option<crate::SchedulerTaskId>,
    completion_stored: bool,
    waiter: Option<crate::SchedulerTaskId>,
}

struct NativeAbiTaskGroup {
    lifecycle: TaskGroupLifecycle,
    children: BTreeMap<TaskId, (u64, u64)>,
}

#[derive(Default)]
struct NativeTaskRegistry {
    tasks: BTreeMap<u64, NativeAbiTask>,
    groups: BTreeMap<u64, NativeAbiTaskGroup>,
}

fn native_task_registry() -> &'static Mutex<NativeTaskRegistry> {
    static REGISTRY: OnceLock<Mutex<NativeTaskRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(NativeTaskRegistry::default()))
}

fn native_task_scheduler() -> &'static crate::NativeScheduler {
    static SCHEDULER: OnceLock<crate::NativeScheduler> = OnceLock::new();
    SCHEDULER.get_or_init(|| {
        let configuration = crate::SchedulerConfiguration::new(4, 4, 65_536, 65_536, 65_536, 1)
            .expect("native task ABI scheduler configuration is valid");
        crate::NativeScheduler::new(configuration)
            .expect("native task ABI scheduler initializes once")
    })
}

thread_local! {
    static CURRENT_NATIVE_TASK: Cell<Option<(u64, crate::SchedulerTaskId)>> = const { Cell::new(None) };
}

fn task_status(record: &NativeAbiTask) -> NativeTaskStatus {
    match record.lifecycle.state() {
        TaskState::Created | TaskState::Ready | TaskState::Running => NativeTaskStatus::Ready,
        TaskState::Suspended => NativeTaskStatus::Pending,
        TaskState::Completed => NativeTaskStatus::Completed,
        TaskState::Cancelled => NativeTaskStatus::Cancelled,
        TaskState::Panicked => NativeTaskStatus::Panicked,
    }
}

#[allow(clippy::too_many_lines)]
fn schedule_started_task(task_handle: u64) -> bool {
    let (frame, callback) = {
        let Ok(registry) = native_task_registry().lock() else {
            return false;
        };
        let Some(record) = registry.tasks.get(&task_handle) else {
            return false;
        };
        let Some(frame) = record.frame.clone() else {
            return false;
        };
        (frame, record.callback)
    };
    let compiler_task = NativeCompilerTask::new(
        frame,
        move |frame: &mut NativeTaskFrame, context: &SchedulerTaskContext| {
            let requested = native_task_registry()
                .lock()
                .ok()
                .and_then(|mut registry| {
                    registry.tasks.get_mut(&task_handle).map(|record| {
                        let requested = record.lifecycle.cancellation_observation(false)
                            == CancellationObservation::Requested;
                        record.lifecycle.begin_poll().ok().map(|()| requested)
                    })
                })
                .flatten();
            let Some(requested) = requested else {
                return SchedulerTaskPoll::Cancelled;
            };
            let previous = CURRENT_NATIVE_TASK.replace(Some((task_handle, context.task())));
            let status = callback(
                task_handle,
                std::ptr::from_mut(frame) as usize as u64,
                u8::from(context.cancellation_requested() || requested),
            );
            CURRENT_NATIVE_TASK.set(previous);
            let status = NativeTaskStatus::from_raw(status).unwrap_or(NativeTaskStatus::Panicked);
            let completion = match status {
                NativeTaskStatus::Ready => TaskPollCompletion::Ready,
                NativeTaskStatus::Pending => TaskPollCompletion::Pending,
                NativeTaskStatus::Completed => TaskPollCompletion::Completed,
                NativeTaskStatus::Cancelled => TaskPollCompletion::Cancelled,
                NativeTaskStatus::Failure | NativeTaskStatus::Panicked => {
                    TaskPollCompletion::Panicked
                }
            };
            let waiter = native_task_registry().lock().ok().and_then(|mut registry| {
                let record = registry.tasks.get_mut(&task_handle)?;
                if record.lifecycle.finish_poll(completion).is_err() {
                    return None;
                }
                record
                    .lifecycle
                    .state()
                    .terminal()
                    .then(|| record.waiter.take())
                    .flatten()
            });
            if let Some(waiter) = waiter {
                let _ = native_task_scheduler().wake(waiter);
            }
            match status {
                NativeTaskStatus::Ready => SchedulerTaskPoll::Ready,
                NativeTaskStatus::Pending => SchedulerTaskPoll::Pending,
                NativeTaskStatus::Completed => SchedulerTaskPoll::Complete,
                NativeTaskStatus::Cancelled => SchedulerTaskPoll::Cancelled,
                NativeTaskStatus::Failure | NativeTaskStatus::Panicked => {
                    panic!("compiler task reported a terminal runtime failure")
                }
            }
        },
    );
    let scheduled = native_task_scheduler().schedule_on(
        SchedulerId::new(1),
        crate::SchedulerTaskMobility::Affine,
        compiler_task,
    );
    match scheduled {
        Ok(scheduler_task) => {
            let Ok(mut registry) = native_task_registry().lock() else {
                return false;
            };
            if let Some(record) = registry.tasks.get_mut(&task_handle) {
                record.frame = None;
                record.scheduler_task = Some(scheduler_task);
                let cold_roots = std::mem::take(&mut record.cold_roots);
                if record.lifecycle.cancellation_observation(false)
                    == CancellationObservation::Requested
                {
                    let _ = native_task_scheduler().request_cancellation(scheduler_task);
                }
                drop(registry);
                for root in cold_roots {
                    let _ = crate::pop_rt_release_root(root);
                }
                true
            } else {
                false
            }
        }
        Err(_) => false,
    }
}
