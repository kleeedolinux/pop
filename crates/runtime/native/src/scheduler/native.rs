//! Bounded synchronized M:N scheduler correctness implementation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pop_runtime_collector::SchedulerId;

use super::{
    DetachedSchedulerRuntimeTransitions, SchedulerBlockingOperationId, SchedulerConfiguration,
    SchedulerError, SchedulerRuntimeTransition, SchedulerRuntimeTransitionControl,
    SchedulerRuntimeTransitions, SchedulerTask, SchedulerTaskContext, SchedulerTaskId,
    SchedulerTaskMobility, SchedulerTaskPoll, SchedulerTaskState, SchedulerTelemetry,
    SchedulerWorkerId,
};

enum InternalTaskState {
    Ready,
    Running { notified: bool },
    Suspended,
    Completed,
    Cancelled,
    Panicked,
}

impl InternalTaskState {
    const fn public(&self) -> SchedulerTaskState {
        match self {
            Self::Ready => SchedulerTaskState::Ready,
            Self::Running { .. } => SchedulerTaskState::Running,
            Self::Suspended => SchedulerTaskState::Suspended,
            Self::Completed => SchedulerTaskState::Completed,
            Self::Cancelled => SchedulerTaskState::Cancelled,
            Self::Panicked => SchedulerTaskState::Panicked,
        }
    }

    const fn terminal(&self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Panicked)
    }
}

struct TaskRecord {
    task: Option<Box<dyn SchedulerTask>>,
    state: InternalTaskState,
    scheduler: SchedulerId,
    mobility: SchedulerTaskMobility,
    cancellation_requested: bool,
}

struct TaskCell {
    record: Mutex<TaskRecord>,
}

struct Registry {
    tasks: BTreeMap<SchedulerTaskId, Arc<TaskCell>>,
    next_task: u64,
    next_scheduler: usize,
}

struct Activity {
    ready: usize,
    running: usize,
}

struct TelemetryState {
    telemetry: SchedulerTelemetry,
    workers_used: BTreeSet<SchedulerWorkerId>,
}

struct ReadyQueues {
    local: Vec<Mutex<VecDeque<SchedulerTaskId>>>,
    injection: Mutex<VecDeque<SchedulerTaskId>>,
    idle_gate: Mutex<()>,
    work_available: Condvar,
}

struct SharedScheduler {
    configuration: SchedulerConfiguration,
    runtime_transitions: Arc<dyn SchedulerRuntimeTransitions>,
    registry: Mutex<Registry>,
    queues: ReadyQueues,
    activity: Mutex<Activity>,
    idle: Condvar,
    telemetry: Mutex<TelemetryState>,
    shutdown: AtomicBool,
    searchers: AtomicUsize,
}

#[derive(Clone, Copy)]
enum WorkSource {
    Local,
    Injection,
    Stolen(usize),
}

struct QueuedTask {
    id: SchedulerTaskId,
    source: WorkSource,
}

type StartedTask = (Arc<TaskCell>, Box<dyn SchedulerTask>, SchedulerTaskContext);

trait BlockingOperation: Send {
    fn run(self: Box<Self>);
}

impl<F: FnOnce() + Send> BlockingOperation for F {
    fn run(self: Box<Self>) {
        (*self)();
    }
}

struct BlockingJob {
    task: SchedulerTaskId,
    operation: Box<dyn BlockingOperation>,
}

struct BlockingPoolState {
    queue: VecDeque<BlockingJob>,
    active: usize,
    next_operation: u64,
    shutdown: bool,
}

struct BlockingPool {
    state: Mutex<BlockingPoolState>,
    available: Condvar,
    idle: Condvar,
    capacity: usize,
}

pub struct NativeScheduler {
    shared: Arc<SharedScheduler>,
    threads: Vec<JoinHandle<Result<(), SchedulerError>>>,
    blocking: Arc<BlockingPool>,
    blocking_threads: Vec<JoinHandle<()>>,
}

impl NativeScheduler {
    /// Starts one bounded normal worker per logical scheduler.
    ///
    /// # Errors
    ///
    /// Fails closed if any worker cannot be started.
    pub fn new(configuration: SchedulerConfiguration) -> Result<Self, SchedulerError> {
        Self::new_with_runtime_transitions(
            configuration,
            Arc::new(DetachedSchedulerRuntimeTransitions),
        )
    }

    /// Starts workers with one explicit collector/runtime transition contract.
    ///
    /// # Errors
    ///
    /// Fails closed if any worker cannot be started. Runtime-transition
    /// failures are returned when workers are joined.
    pub fn new_with_runtime_transitions<T: SchedulerRuntimeTransitions>(
        configuration: SchedulerConfiguration,
        runtime_transitions: Arc<T>,
    ) -> Result<Self, SchedulerError> {
        let runtime_transitions: Arc<dyn SchedulerRuntimeTransitions> = runtime_transitions;
        let shared = Arc::new(SharedScheduler::new(configuration, runtime_transitions));
        let mut threads: Vec<JoinHandle<Result<(), SchedulerError>>> =
            Vec::with_capacity(configuration.worker_count);
        for index in 0..configuration.worker_count {
            let worker_shared = Arc::clone(&shared);
            let worker_raw =
                u32::try_from(index + 1).map_err(|_| SchedulerError::IdentityOverflow)?;
            let worker = SchedulerWorkerId::new(worker_raw);
            let scheduler = SchedulerId::new(worker.raw());
            let Ok(handle) = thread::Builder::new()
                .name(format!("pop-scheduler-{}", worker.raw()))
                .spawn(move || worker_loop(&worker_shared, worker, scheduler))
            else {
                shared.shutdown();
                for thread in threads {
                    let _ = thread.join();
                }
                return Err(SchedulerError::ThreadStart);
            };
            threads.push(handle);
        }
        let blocking = Arc::new(BlockingPool::new(configuration.blocking_queue_capacity));
        let mut blocking_threads: Vec<JoinHandle<()>> =
            Vec::with_capacity(configuration.blocking_worker_count);
        for index in 0..configuration.blocking_worker_count {
            let worker_pool = Arc::clone(&blocking);
            let worker_shared = Arc::clone(&shared);
            let Ok(handle) = thread::Builder::new()
                .name(format!("pop-blocking-{}", index + 1))
                .spawn(move || blocking_worker_loop(&worker_pool, &worker_shared))
            else {
                blocking.shutdown();
                for thread in blocking_threads {
                    let _ = thread.join();
                }
                shared.shutdown();
                for thread in threads {
                    let _ = thread.join();
                }
                return Err(SchedulerError::ThreadStart);
            };
            blocking_threads.push(handle);
        }
        Ok(Self {
            shared,
            threads,
            blocking,
            blocking_threads,
        })
    }

    /// Adds one already-owned movable task through the external injection path.
    ///
    /// # Errors
    ///
    /// Rejects closed state, retained-task capacity, injection capacity, or
    /// typed identity exhaustion before retaining the task.
    pub fn schedule<T: SchedulerTask>(&self, task: T) -> Result<SchedulerTaskId, SchedulerError> {
        self.shared.schedule_injected(Box::new(task))
    }

    /// Adds one already-owned task to an exact logical scheduler queue.
    ///
    /// # Errors
    ///
    /// Rejects unknown schedulers or bounded-capacity exhaustion before
    /// retaining the task.
    pub fn schedule_on<T: SchedulerTask>(
        &self,
        scheduler: SchedulerId,
        mobility: SchedulerTaskMobility,
        task: T,
    ) -> Result<SchedulerTaskId, SchedulerError> {
        self.shared
            .schedule_local(scheduler, mobility, Box::new(task))
    }

    /// Submits declared blocking work to the bounded non-mutator pool.
    /// Completion wakes the exact owning task through the normal ready path.
    ///
    /// # Errors
    ///
    /// Rejects unknown/terminal tasks, closed state, queue saturation, or
    /// typed operation-identity exhaustion before retaining the operation.
    pub fn submit_blocking<F: FnOnce() + Send + 'static>(
        &self,
        task: SchedulerTaskId,
        operation: F,
    ) -> Result<SchedulerBlockingOperationId, SchedulerError> {
        self.shared.ensure_open()?;
        if self.shared.task_state(task)?.terminal() {
            return Err(SchedulerError::UnknownTask(task));
        }
        self.blocking
            .submit(task, Box::new(operation), &self.shared)
    }

    /// Marks a suspended/running task ready exactly once.
    ///
    /// # Errors
    ///
    /// Rejects unknown tasks or closed scheduler state.
    pub fn wake(&self, id: SchedulerTaskId) -> Result<bool, SchedulerError> {
        self.shared.wake(id)
    }

    /// Requests cooperative cancellation and wakes a suspended task.
    ///
    /// # Errors
    ///
    /// Rejects unknown tasks or closed scheduler state.
    pub fn request_cancellation(&self, id: SchedulerTaskId) -> Result<bool, SchedulerError> {
        self.shared.request_cancellation(id)
    }

    /// Returns the exact retained state for one task.
    ///
    /// # Errors
    ///
    /// Rejects an unknown task identity.
    pub fn task_state(&self, id: SchedulerTaskId) -> Result<SchedulerTaskState, SchedulerError> {
        self.shared.task_state(id)
    }

    /// Releases retained scheduler state after terminal completion.
    ///
    /// # Errors
    ///
    /// Rejects unknown or nonterminal tasks.
    pub fn release_terminal_task(&self, id: SchedulerTaskId) -> Result<(), SchedulerError> {
        self.shared.release_terminal_task(id)
    }

    /// Waits until no task is ready or running. Suspended tasks may remain.
    ///
    /// # Errors
    ///
    /// Returns `WaitTimedOut` when the supplied host-side test/coordination
    /// deadline expires.
    pub fn wait_until_idle(&self, timeout: Duration) -> Result<(), SchedulerError> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or(SchedulerError::WaitTimedOut)?;
        self.shared.wait_until_idle(timeout)?;
        self.blocking.wait_until_idle(remaining_until(deadline)?)?;
        self.shared.wait_until_idle(remaining_until(deadline)?)
    }

    #[must_use]
    pub fn telemetry(&self) -> SchedulerTelemetry {
        self.shared.telemetry()
    }

    /// Stops and joins all normal workers.
    ///
    /// # Errors
    ///
    /// Reports a worker that terminated outside the scheduler panic boundary.
    pub fn shutdown(mut self) -> Result<(), SchedulerError> {
        self.shutdown_threads()
    }

    fn shutdown_threads(&mut self) -> Result<(), SchedulerError> {
        self.blocking.shutdown();
        for thread in self.blocking_threads.drain(..) {
            if thread.join().is_err() {
                self.shared.shutdown();
                for thread in self.threads.drain(..) {
                    let _ = thread.join();
                }
                return Err(SchedulerError::ThreadJoin);
            }
        }
        self.shared.shutdown();
        let mut failure = None;
        for thread in self.threads.drain(..) {
            match thread.join() {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    failure.get_or_insert(error);
                }
                Err(_) => {
                    failure.get_or_insert(SchedulerError::ThreadJoin);
                }
            }
        }
        failure.map_or(Ok(()), Err)
    }
}

impl Drop for NativeScheduler {
    fn drop(&mut self) {
        let _ = self.shutdown_threads();
    }
}

impl BlockingPool {
    fn new(capacity: usize) -> Self {
        Self {
            state: Mutex::new(BlockingPoolState {
                queue: VecDeque::new(),
                active: 0,
                next_operation: 1,
                shutdown: false,
            }),
            available: Condvar::new(),
            idle: Condvar::new(),
            capacity,
        }
    }

    fn submit(
        &self,
        task: SchedulerTaskId,
        operation: Box<dyn BlockingOperation>,
        scheduler: &SharedScheduler,
    ) -> Result<SchedulerBlockingOperationId, SchedulerError> {
        let mut state = lock(&self.state);
        if state.shutdown {
            return Err(SchedulerError::Closed);
        }
        if state.queue.len() >= self.capacity {
            let mut telemetry = lock(&scheduler.telemetry);
            telemetry.telemetry.blocking_queue_rejections = telemetry
                .telemetry
                .blocking_queue_rejections
                .saturating_add(1);
            return Err(SchedulerError::BlockingQueueCapacity);
        }
        let id = SchedulerBlockingOperationId::new(state.next_operation);
        state.next_operation = state
            .next_operation
            .checked_add(1)
            .ok_or(SchedulerError::IdentityOverflow)?;
        state.queue.push_back(BlockingJob { task, operation });
        {
            let mut telemetry = lock(&scheduler.telemetry);
            telemetry.telemetry.blocking_submissions =
                telemetry.telemetry.blocking_submissions.saturating_add(1);
        }
        drop(state);
        self.available.notify_one();
        Ok(id)
    }

    fn shutdown(&self) {
        let mut state = lock(&self.state);
        state.shutdown = true;
        drop(state);
        self.available.notify_all();
    }

    fn wait_until_idle(&self, timeout: Duration) -> Result<(), SchedulerError> {
        let state = lock(&self.state);
        let (state, result) = self
            .idle
            .wait_timeout_while(state, timeout, |state| {
                state.active != 0 || !state.queue.is_empty()
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if result.timed_out() && (state.active != 0 || !state.queue.is_empty()) {
            Err(SchedulerError::WaitTimedOut)
        } else {
            Ok(())
        }
    }
}

impl SharedScheduler {
    fn new(
        configuration: SchedulerConfiguration,
        runtime_transitions: Arc<dyn SchedulerRuntimeTransitions>,
    ) -> Self {
        Self {
            configuration,
            runtime_transitions,
            registry: Mutex::new(Registry {
                tasks: BTreeMap::new(),
                next_task: 1,
                next_scheduler: 0,
            }),
            queues: ReadyQueues {
                local: (0..configuration.scheduler_count)
                    .map(|_| Mutex::new(VecDeque::new()))
                    .collect(),
                injection: Mutex::new(VecDeque::new()),
                idle_gate: Mutex::new(()),
                work_available: Condvar::new(),
            },
            activity: Mutex::new(Activity {
                ready: 0,
                running: 0,
            }),
            idle: Condvar::new(),
            telemetry: Mutex::new(TelemetryState {
                telemetry: SchedulerTelemetry::default(),
                workers_used: BTreeSet::new(),
            }),
            shutdown: AtomicBool::new(false),
            searchers: AtomicUsize::new(0),
        }
    }

    fn schedule_injected(
        &self,
        task: Box<dyn SchedulerTask>,
    ) -> Result<SchedulerTaskId, SchedulerError> {
        self.ensure_open()?;
        if lock(&self.registry).tasks.len() >= self.configuration.task_capacity {
            return Err(SchedulerError::TaskCapacity);
        }
        let mut injection = lock(&self.queues.injection);
        if injection.len() >= self.configuration.injection_queue_capacity {
            return Err(SchedulerError::InjectionQueueCapacity);
        }
        let mut registry = lock(&self.registry);
        if registry.tasks.len() >= self.configuration.task_capacity {
            return Err(SchedulerError::TaskCapacity);
        }
        let scheduler_index = registry.next_scheduler;
        registry.next_scheduler =
            (registry.next_scheduler + 1) % self.configuration.scheduler_count;
        let scheduler = SchedulerId::new(
            u32::try_from(scheduler_index + 1).expect("validated scheduler identity range"),
        );
        let id = next_task_id(&mut registry)?;
        registry.tasks.insert(
            id,
            Arc::new(TaskCell {
                record: Mutex::new(TaskRecord {
                    task: Some(task),
                    state: InternalTaskState::Ready,
                    scheduler,
                    mobility: SchedulerTaskMobility::Movable,
                    cancellation_requested: false,
                }),
            }),
        );
        injection.push_back(id);
        self.record_scheduled(&registry);
        drop(registry);
        self.increment_ready();
        drop(injection);
        self.notify_work();
        Ok(id)
    }

    fn schedule_local(
        &self,
        scheduler: SchedulerId,
        mobility: SchedulerTaskMobility,
        task: Box<dyn SchedulerTask>,
    ) -> Result<SchedulerTaskId, SchedulerError> {
        self.ensure_open()?;
        let index = self.scheduler_index(scheduler)?;
        let mut queue = lock(&self.queues.local[index]);
        if queue.len() >= self.configuration.local_queue_capacity {
            return Err(SchedulerError::LocalQueueCapacity);
        }
        let mut registry = lock(&self.registry);
        if registry.tasks.len() >= self.configuration.task_capacity {
            return Err(SchedulerError::TaskCapacity);
        }
        let id = next_task_id(&mut registry)?;
        registry.tasks.insert(
            id,
            Arc::new(TaskCell {
                record: Mutex::new(TaskRecord {
                    task: Some(task),
                    state: InternalTaskState::Ready,
                    scheduler,
                    mobility,
                    cancellation_requested: false,
                }),
            }),
        );
        queue.push_back(id);
        self.record_scheduled(&registry);
        drop(registry);
        self.increment_ready();
        drop(queue);
        self.notify_work();
        Ok(id)
    }

    fn wake(&self, id: SchedulerTaskId) -> Result<bool, SchedulerError> {
        self.ensure_open()?;
        let cell = self.task(id)?;
        let mut record = lock(&cell.record);
        let suspended_scheduler = match &mut record.state {
            InternalTaskState::Suspended => Some(record.scheduler),
            InternalTaskState::Running { notified: false } => {
                record.state = InternalTaskState::Running { notified: true };
                None
            }
            InternalTaskState::Ready
            | InternalTaskState::Running { notified: true }
            | InternalTaskState::Completed
            | InternalTaskState::Cancelled
            | InternalTaskState::Panicked => {
                let mut telemetry = lock(&self.telemetry);
                telemetry.telemetry.wake_requests =
                    telemetry.telemetry.wake_requests.saturating_add(1);
                telemetry.telemetry.coalesced_wakes =
                    telemetry.telemetry.coalesced_wakes.saturating_add(1);
                return Ok(false);
            }
        };
        drop(record);

        if let Some(scheduler) = suspended_scheduler {
            let index = self.scheduler_index(scheduler)?;
            let mut queue = lock(&self.queues.local[index]);
            let mut record = lock(&cell.record);
            if matches!(record.state, InternalTaskState::Suspended) {
                if queue.len() >= self.configuration.local_queue_capacity {
                    return Err(SchedulerError::LocalQueueCapacity);
                }
                self.require_runtime_transition(SchedulerRuntimeTransition::TaskResumed {
                    task: id,
                    scheduler,
                })?;
                record.state = InternalTaskState::Ready;
                queue.push_back(id);
                drop(record);
                drop(queue);
                self.increment_ready();
                self.notify_work();
            } else {
                let mut telemetry = lock(&self.telemetry);
                telemetry.telemetry.wake_requests =
                    telemetry.telemetry.wake_requests.saturating_add(1);
                telemetry.telemetry.coalesced_wakes =
                    telemetry.telemetry.coalesced_wakes.saturating_add(1);
                return Ok(false);
            }
        }
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.wake_requests = telemetry.telemetry.wake_requests.saturating_add(1);
        Ok(true)
    }

    fn request_cancellation(&self, id: SchedulerTaskId) -> Result<bool, SchedulerError> {
        self.ensure_open()?;
        let cell = self.task(id)?;
        let mut record = lock(&cell.record);
        if record.state.terminal() || record.cancellation_requested {
            return Ok(false);
        }
        let scheduler = record.scheduler;
        let suspended = matches!(record.state, InternalTaskState::Suspended);
        if !suspended {
            record.cancellation_requested = true;
        }
        match &mut record.state {
            InternalTaskState::Running { notified } => {
                *notified = true;
            }
            InternalTaskState::Ready | InternalTaskState::Suspended => {}
            InternalTaskState::Completed
            | InternalTaskState::Cancelled
            | InternalTaskState::Panicked => return Ok(false),
        }
        drop(record);

        if suspended {
            let index = self.scheduler_index(scheduler)?;
            let mut queue = lock(&self.queues.local[index]);
            let mut record = lock(&cell.record);
            if record.state.terminal() || record.cancellation_requested {
                return Ok(false);
            }
            record.cancellation_requested = true;
            if matches!(record.state, InternalTaskState::Suspended) {
                if queue.len() >= self.configuration.local_queue_capacity {
                    record.cancellation_requested = false;
                    return Err(SchedulerError::LocalQueueCapacity);
                }
                self.require_runtime_transition(SchedulerRuntimeTransition::TaskResumed {
                    task: id,
                    scheduler,
                })?;
                record.state = InternalTaskState::Ready;
                queue.push_back(id);
                drop(record);
                drop(queue);
                self.increment_ready();
                self.notify_work();
            }
        }
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.cancellations_requested = telemetry
            .telemetry
            .cancellations_requested
            .saturating_add(1);
        Ok(true)
    }

    fn task_state(&self, id: SchedulerTaskId) -> Result<SchedulerTaskState, SchedulerError> {
        let cell = self.task(id)?;
        let state = lock(&cell.record).state.public();
        Ok(state)
    }

    fn release_terminal_task(&self, id: SchedulerTaskId) -> Result<(), SchedulerError> {
        let mut registry = lock(&self.registry);
        let cell = registry
            .tasks
            .get(&id)
            .cloned()
            .ok_or(SchedulerError::UnknownTask(id))?;
        if !lock(&cell.record).state.terminal() {
            return Err(SchedulerError::TaskNotTerminal(id));
        }
        registry.tasks.remove(&id);
        self.refresh_retained(&registry);
        Ok(())
    }

    fn wait_until_idle(&self, timeout: Duration) -> Result<(), SchedulerError> {
        let activity = lock(&self.activity);
        let (activity, result) = self
            .idle
            .wait_timeout_while(activity, timeout, |state| {
                state.ready != 0 || state.running != 0
            })
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if result.timed_out() && (activity.ready != 0 || activity.running != 0) {
            Err(SchedulerError::WaitTimedOut)
        } else {
            Ok(())
        }
    }

    fn telemetry(&self) -> SchedulerTelemetry {
        let telemetry = lock(&self.telemetry);
        let mut snapshot = telemetry.telemetry;
        snapshot.worker_threads_used = telemetry.workers_used.len();
        snapshot
    }

    fn task(&self, id: SchedulerTaskId) -> Result<Arc<TaskCell>, SchedulerError> {
        lock(&self.registry)
            .tasks
            .get(&id)
            .cloned()
            .ok_or(SchedulerError::UnknownTask(id))
    }

    fn take_work(
        &self,
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
        local_polls: usize,
    ) -> Result<Option<QueuedTask>, SchedulerError> {
        let index = self.scheduler_index(scheduler)?;
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                return Ok(None);
            }
            if let Some(task) = self.try_take(index, local_polls) {
                return Ok(Some(task));
            }
            let idle = lock(&self.queues.idle_gate);
            if self.shutdown.load(Ordering::Acquire) {
                return Ok(None);
            }
            if let Some(task) = self.try_take(index, local_polls) {
                return Ok(Some(task));
            }
            self.record_worker_used(worker);
            self.require_runtime_transition(SchedulerRuntimeTransition::WorkerParked {
                worker,
                scheduler,
            })?;
            drop(
                self.queues
                    .work_available
                    .wait(idle)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            );
            self.require_runtime_transition(SchedulerRuntimeTransition::WorkerUnparked {
                worker,
                scheduler,
            })?;
        }
    }

    fn try_take(&self, index: usize, local_polls: usize) -> Option<QueuedTask> {
        let check_injection =
            local_polls.is_multiple_of(self.configuration.injection_poll_interval);
        if check_injection && let Some(id) = lock(&self.queues.injection).pop_front() {
            if self.assign_scheduler(id, index) {
                return Some(QueuedTask {
                    id,
                    source: WorkSource::Injection,
                });
            }
            lock(&self.queues.injection).push_back(id);
        }
        if let Some(id) = lock(&self.queues.local[index]).pop_front() {
            return Some(QueuedTask {
                id,
                source: WorkSource::Local,
            });
        }
        if !check_injection && let Some(id) = lock(&self.queues.injection).pop_front() {
            if self.assign_scheduler(id, index) {
                return Some(QueuedTask {
                    id,
                    source: WorkSource::Injection,
                });
            }
            lock(&self.queues.injection).push_back(id);
        }
        self.try_steal(index)
    }

    fn try_steal(&self, thief: usize) -> Option<QueuedTask> {
        let maximum_searchers = self.configuration.worker_count.div_ceil(2);
        let entered = self
            .searchers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |searchers| {
                (searchers < maximum_searchers).then_some(searchers + 1)
            })
            .is_ok();
        if !entered {
            return None;
        }
        let mut result = None;
        for offset in 1..self.configuration.scheduler_count {
            let victim = (thief + offset) % self.configuration.scheduler_count;
            let mut victim_queue = lock(&self.queues.local[victim]);
            if victim_queue.is_empty() {
                continue;
            }
            let registry = lock(&self.registry);
            let eligible = victim_queue
                .iter()
                .filter(|id| {
                    registry.tasks.get(id).is_some_and(|cell| {
                        lock(&cell.record).mobility == SchedulerTaskMobility::Movable
                    })
                })
                .count();
            let count = eligible.div_ceil(2).max(1);
            let mut stolen = Vec::with_capacity(count);
            let candidates = victim_queue.iter().copied().rev().collect::<Vec<_>>();
            for id in candidates {
                if stolen.len() >= count {
                    break;
                }
                let Some(cell) = registry.tasks.get(&id) else {
                    continue;
                };
                let mut record = lock(&cell.record);
                if record.mobility != SchedulerTaskMobility::Movable
                    || !self.migration_allowed(id, record.scheduler, scheduler_id(thief))
                {
                    continue;
                }
                let Some(position) = victim_queue.iter().position(|candidate| *candidate == id)
                else {
                    continue;
                };
                victim_queue.remove(position);
                record.scheduler = scheduler_id(thief);
                stolen.push(id);
            }
            drop(registry);
            drop(victim_queue);
            if stolen.is_empty() {
                continue;
            }
            let first = stolen.remove(0);
            let batch = stolen.len() + 1;
            if !stolen.is_empty() {
                let mut local = lock(&self.queues.local[thief]);
                for id in stolen {
                    local.push_back(id);
                }
            }
            {
                let mut telemetry = lock(&self.telemetry);
                telemetry.telemetry.tasks_stolen = telemetry
                    .telemetry
                    .tasks_stolen
                    .saturating_add(u64::try_from(batch).unwrap_or(u64::MAX));
            }
            self.notify_work();
            result = Some(QueuedTask {
                id: first,
                source: WorkSource::Stolen(batch),
            });
            break;
        }
        self.searchers.fetch_sub(1, Ordering::AcqRel);
        result
    }

    fn assign_scheduler(&self, id: SchedulerTaskId, index: usize) -> bool {
        if let Ok(cell) = self.task(id) {
            let mut record = lock(&cell.record);
            if record.mobility == SchedulerTaskMobility::Movable {
                let destination = scheduler_id(index);
                if record.scheduler != destination
                    && !self.migration_allowed(id, record.scheduler, destination)
                {
                    return false;
                }
                record.scheduler = destination;
            }
        }
        true
    }

    fn begin_poll(
        &self,
        queued: &QueuedTask,
        worker: SchedulerWorkerId,
    ) -> Result<Option<StartedTask>, SchedulerError> {
        let cell = self.task(queued.id)?;
        let mut record = lock(&cell.record);
        if !matches!(record.state, InternalTaskState::Ready) {
            return Ok(None);
        }
        self.require_runtime_transition(SchedulerRuntimeTransition::TaskDispatched {
            task: queued.id,
            worker,
            scheduler: record.scheduler,
        })?;
        record.state = InternalTaskState::Running { notified: false };
        let Some(task) = record.task.take() else {
            return Ok(None);
        };
        let context = SchedulerTaskContext::new(
            queued.id,
            record.scheduler,
            worker,
            record.cancellation_requested,
        );
        drop(record);
        {
            let mut activity = lock(&self.activity);
            activity.ready = activity.ready.saturating_sub(1);
            activity.running = activity.running.saturating_add(1);
        }
        {
            let mut telemetry = lock(&self.telemetry);
            telemetry.telemetry.polls = telemetry.telemetry.polls.saturating_add(1);
            telemetry.workers_used.insert(worker);
            if let WorkSource::Stolen(batch) = queued.source {
                let _ = batch;
            }
        }
        self.refresh_state_counts();
        Ok(Some((cell, task, context)))
    }

    fn finish_poll(
        &self,
        id: SchedulerTaskId,
        cell: &TaskCell,
        task: Box<dyn SchedulerTask>,
        result: Result<SchedulerTaskPoll, Box<dyn std::any::Any + Send>>,
    ) -> Result<(), SchedulerError> {
        let mut record = lock(&cell.record);
        let notified = matches!(record.state, InternalTaskState::Running { notified: true });
        let suspended = matches!(&result, Ok(SchedulerTaskPoll::Pending));
        let mut enqueue = false;
        let terminal_state = match result {
            Ok(SchedulerTaskPoll::Ready) => {
                record.task = Some(task);
                record.state = InternalTaskState::Ready;
                enqueue = true;
                None
            }
            Ok(SchedulerTaskPoll::Pending) if notified => {
                record.task = Some(task);
                record.state = InternalTaskState::Ready;
                enqueue = true;
                None
            }
            Ok(SchedulerTaskPoll::Pending) => {
                record.task = Some(task);
                record.state = InternalTaskState::Suspended;
                let mut telemetry = lock(&self.telemetry);
                telemetry.telemetry.suspensions = telemetry.telemetry.suspensions.saturating_add(1);
                None
            }
            Ok(SchedulerTaskPoll::Complete) => {
                record.state = InternalTaskState::Completed;
                let mut telemetry = lock(&self.telemetry);
                telemetry.telemetry.completions = telemetry.telemetry.completions.saturating_add(1);
                Some(SchedulerTaskState::Completed)
            }
            Ok(SchedulerTaskPoll::Cancelled) => {
                record.state = InternalTaskState::Cancelled;
                let mut telemetry = lock(&self.telemetry);
                telemetry.telemetry.cancellations_observed =
                    telemetry.telemetry.cancellations_observed.saturating_add(1);
                Some(SchedulerTaskState::Cancelled)
            }
            Err(panic) => {
                drop(panic);
                record.state = InternalTaskState::Panicked;
                let mut telemetry = lock(&self.telemetry);
                telemetry.telemetry.panics = telemetry.telemetry.panics.saturating_add(1);
                Some(SchedulerTaskState::Panicked)
            }
        };
        let scheduler = record.scheduler;
        drop(record);
        {
            let mut activity = lock(&self.activity);
            activity.running = activity.running.saturating_sub(1);
            if enqueue {
                activity.ready = activity.ready.saturating_add(1);
            }
            if activity.ready == 0 && activity.running == 0 {
                self.idle.notify_all();
            }
        }
        if enqueue {
            let index = self
                .scheduler_index(scheduler)
                .expect("task scheduler remains configured");
            lock(&self.queues.local[index]).push_back(id);
            self.notify_work();
        }
        if suspended && !notified {
            self.require_runtime_transition(SchedulerRuntimeTransition::TaskSuspended {
                task: id,
                scheduler,
            })?;
        } else if suspended && notified {
            self.require_runtime_transition(SchedulerRuntimeTransition::TaskResumed {
                task: id,
                scheduler,
            })?;
        }
        if let Some(state) = terminal_state {
            self.require_runtime_transition(SchedulerRuntimeTransition::TaskTerminal {
                task: id,
                scheduler,
                state,
            })?;
        }
        self.refresh_state_counts();
        Ok(())
    }

    fn scheduler_index(&self, scheduler: SchedulerId) -> Result<usize, SchedulerError> {
        let raw = scheduler.raw();
        if raw == 0 || raw as usize > self.configuration.scheduler_count {
            Err(SchedulerError::UnknownScheduler(scheduler))
        } else {
            Ok(raw as usize - 1)
        }
    }

    fn ensure_open(&self) -> Result<(), SchedulerError> {
        if self.shutdown.load(Ordering::Acquire) {
            Err(SchedulerError::Closed)
        } else {
            Ok(())
        }
    }

    fn increment_ready(&self) {
        let mut activity = lock(&self.activity);
        activity.ready = activity.ready.saturating_add(1);
        drop(activity);
        self.refresh_state_counts();
    }

    fn notify_work(&self) {
        let _idle = lock(&self.queues.idle_gate);
        self.queues.work_available.notify_one();
    }

    fn record_scheduled(&self, registry: &Registry) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.tasks_scheduled = telemetry.telemetry.tasks_scheduled.saturating_add(1);
        telemetry.telemetry.retained_tasks = registry.tasks.len();
    }

    fn refresh_retained(&self, registry: &Registry) {
        lock(&self.telemetry).telemetry.retained_tasks = registry.tasks.len();
    }

    fn refresh_state_counts(&self) {
        let registry = lock(&self.registry);
        let mut ready = 0;
        let mut running = 0;
        let mut suspended = 0;
        let mut terminal = 0;
        for cell in registry.tasks.values() {
            match lock(&cell.record).state.public() {
                SchedulerTaskState::Ready => ready += 1,
                SchedulerTaskState::Running => running += 1,
                SchedulerTaskState::Suspended => suspended += 1,
                SchedulerTaskState::Completed
                | SchedulerTaskState::Cancelled
                | SchedulerTaskState::Panicked => terminal += 1,
            }
        }
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.retained_tasks = registry.tasks.len();
        telemetry.telemetry.ready_tasks = ready;
        telemetry.telemetry.running_tasks = running;
        telemetry.telemetry.suspended_tasks = suspended;
        telemetry.telemetry.terminal_tasks = terminal;
    }

    fn record_worker_used(&self, worker: SchedulerWorkerId) {
        lock(&self.telemetry).workers_used.insert(worker);
    }

    fn apply_runtime_transition(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerError> {
        self.runtime_transitions
            .apply(transition)
            .map_err(|failure| SchedulerError::RuntimeTransition {
                transition,
                failure,
            })
    }

    fn require_runtime_transition(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<(), SchedulerError> {
        match self.apply_runtime_transition(transition)? {
            SchedulerRuntimeTransitionControl::Continue => Ok(()),
            SchedulerRuntimeTransitionControl::RefuseMigration => {
                Err(SchedulerError::RuntimeTransition {
                    transition,
                    failure: super::SchedulerRuntimeTransitionFailure::CollectorState,
                })
            }
        }
    }

    fn migration_allowed(&self, task: SchedulerTaskId, from: SchedulerId, to: SchedulerId) -> bool {
        if from == to {
            return true;
        }
        let transition = SchedulerRuntimeTransition::TaskMigration { task, from, to };
        let allowed = matches!(
            self.apply_runtime_transition(transition),
            Ok(SchedulerRuntimeTransitionControl::Continue)
        );
        if !allowed {
            let mut telemetry = lock(&self.telemetry);
            telemetry.telemetry.gc_delayed_migrations =
                telemetry.telemetry.gc_delayed_migrations.saturating_add(1);
        }
        allowed
    }

    fn shutdown(&self) {
        let _idle = lock(&self.queues.idle_gate);
        self.shutdown.store(true, Ordering::Release);
        self.queues.work_available.notify_all();
        self.idle.notify_all();
    }
}

fn worker_loop(
    shared: &SharedScheduler,
    worker: SchedulerWorkerId,
    scheduler: SchedulerId,
) -> Result<(), SchedulerError> {
    shared.require_runtime_transition(SchedulerRuntimeTransition::WorkerStarted {
        worker,
        scheduler,
    })?;
    let work_result = (|| {
        let mut local_polls = 0;
        while let Some(queued) = shared.take_work(worker, scheduler, local_polls)? {
            let local = matches!(queued.source, WorkSource::Local);
            let Some((cell, mut task, context)) = shared.begin_poll(&queued, worker)? else {
                continue;
            };
            let result = catch_unwind(AssertUnwindSafe(|| task.poll(&context)));
            shared.finish_poll(queued.id, &cell, task, result)?;
            if local {
                local_polls = local_polls.saturating_add(1);
            } else {
                local_polls = 0;
            }
        }
        Ok(())
    })();
    let stop_result =
        shared.require_runtime_transition(SchedulerRuntimeTransition::WorkerStopped {
            worker,
            scheduler,
        });
    work_result.and(stop_result)
}

fn blocking_worker_loop(pool: &BlockingPool, scheduler: &SharedScheduler) {
    loop {
        let mut state = lock(&pool.state);
        while state.queue.is_empty() && !state.shutdown {
            state = pool
                .available
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        let Some(job) = state.queue.pop_front() else {
            return;
        };
        state.active = state.active.saturating_add(1);
        drop(state);

        let panicked = catch_unwind(AssertUnwindSafe(|| job.operation.run())).is_err();
        let _ = scheduler.wake(job.task);
        let mut telemetry = lock(&scheduler.telemetry);
        telemetry.telemetry.blocking_completions =
            telemetry.telemetry.blocking_completions.saturating_add(1);
        if panicked {
            telemetry.telemetry.blocking_panics =
                telemetry.telemetry.blocking_panics.saturating_add(1);
        }
        drop(telemetry);
        let mut state = lock(&pool.state);
        state.active = state.active.saturating_sub(1);
        if state.active == 0 && state.queue.is_empty() {
            pool.idle.notify_all();
        }
    }
}

fn remaining_until(deadline: Instant) -> Result<Duration, SchedulerError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(SchedulerError::WaitTimedOut)
}

fn next_task_id(registry: &mut Registry) -> Result<SchedulerTaskId, SchedulerError> {
    let id = SchedulerTaskId::new(registry.next_task);
    registry.next_task = registry
        .next_task
        .checked_add(1)
        .ok_or(SchedulerError::IdentityOverflow)?;
    Ok(id)
}

fn scheduler_id(index: usize) -> SchedulerId {
    SchedulerId::new(u32::try_from(index + 1).expect("validated scheduler identity range"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}
