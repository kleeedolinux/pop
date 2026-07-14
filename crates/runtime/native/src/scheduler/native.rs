//! Bounded synchronized M:N scheduler correctness implementation.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, TryLockError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use pop_runtime_collector::{MutatorId, SchedulerId};
use pop_runtime_interface::TaskFrameRootId;

use super::{
    DetachedSchedulerRuntimeTransitions, SchedulerBlockingOperationId,
    SchedulerCollectorBindingFailure, SchedulerConfiguration, SchedulerError,
    SchedulerExternalEventId, SchedulerRuntimeTransition, SchedulerRuntimeTransitionControl,
    SchedulerRuntimeTransitions, SchedulerTask, SchedulerTaskContext, SchedulerTaskFrameFailure,
    SchedulerTaskId, SchedulerTaskMobility, SchedulerTaskPoll, SchedulerTaskState,
    SchedulerTelemetry, SchedulerTimerId, SchedulerWorkerId,
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
    frame_roots: Option<TaskFrameRootId>,
    ready_since_work: u64,
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
    migration_enabled: bool,
    submissions_active: AtomicUsize,
    observation_work: AtomicU64,
}

#[derive(Clone, Copy)]
enum WorkSource {
    Local,
    Injection,
    Stolen(usize),
}

#[derive(Clone, Copy)]
enum NonterminalPollState {
    Ready,
    Resumed,
    Suspended,
}

struct QueuedTask {
    id: SchedulerTaskId,
    source: WorkSource,
}

struct SubmissionGuard<'a>(&'a AtomicUsize);

impl<'a> SubmissionGuard<'a> {
    fn enter(active: &'a AtomicUsize, searchers: &AtomicUsize) -> Self {
        active.fetch_add(1, Ordering::AcqRel);
        while searchers.load(Ordering::Acquire) != 0 {
            thread::yield_now();
        }
        Self(active)
    }
}

impl Drop for SubmissionGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
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

struct ExternalEventRegistration {
    task: SchedulerTaskId,
    signalled: bool,
    delivered: bool,
}

struct EventDriverState {
    events: BTreeMap<SchedulerExternalEventId, ExternalEventRegistration>,
    timers: BTreeMap<(Instant, SchedulerTimerId), SchedulerTaskId>,
    deliveries: VecDeque<SchedulerExternalEventId>,
    next_event: u64,
    next_timer: u64,
    shutdown: bool,
}

struct EventDriver {
    state: Mutex<EventDriverState>,
    changed: Condvar,
    event_capacity: usize,
    timer_capacity: usize,
    delivery_capacity: usize,
}

enum HostDelivery {
    ExternalEvent(SchedulerTaskId),
    Timer(SchedulerTaskId),
}

pub struct NativeScheduler {
    shared: Arc<SharedScheduler>,
    threads: Vec<JoinHandle<Result<(), SchedulerError>>>,
    blocking: Arc<BlockingPool>,
    blocking_threads: Vec<JoinHandle<()>>,
    event_driver: Arc<EventDriver>,
    event_driver_thread: Option<JoinHandle<()>>,
}

impl NativeScheduler {
    /// Starts one bounded normal worker per logical scheduler.
    ///
    /// # Errors
    ///
    /// Fails closed if any worker cannot be started.
    pub fn new(configuration: SchedulerConfiguration) -> Result<Self, SchedulerError> {
        Self::start(
            configuration,
            Arc::new(DetachedSchedulerRuntimeTransitions),
            false,
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
        Self::start(configuration, runtime_transitions, true)
    }

    fn start(
        configuration: SchedulerConfiguration,
        runtime_transitions: Arc<dyn SchedulerRuntimeTransitions>,
        migration_enabled: bool,
    ) -> Result<Self, SchedulerError> {
        let shared = Arc::new(SharedScheduler::new(
            configuration,
            runtime_transitions,
            migration_enabled,
        ));
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
        let event_driver = Arc::new(EventDriver::new(configuration));
        let driver_state = Arc::clone(&event_driver);
        let driver_scheduler = Arc::clone(&shared);
        let Ok(event_driver_thread) = thread::Builder::new()
            .name("pop-event-driver".to_owned())
            .spawn(move || event_driver_loop(&driver_state, &driver_scheduler))
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
        Ok(Self {
            shared,
            threads,
            blocking,
            blocking_threads,
            event_driver,
            event_driver_thread: Some(event_driver_thread),
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

    /// Adds one bounded batch to an exact logical scheduler atomically.
    ///
    /// # Errors
    ///
    /// Rejects unknown schedulers, retained/local capacity, or identity
    /// exhaustion before retaining any task in the batch.
    pub fn schedule_batch_on<T: SchedulerTask>(
        &self,
        scheduler: SchedulerId,
        mobility: SchedulerTaskMobility,
        tasks: Vec<T>,
    ) -> Result<Vec<SchedulerTaskId>, SchedulerError> {
        self.shared.schedule_local_batch(
            scheduler,
            mobility,
            tasks
                .into_iter()
                .map(|task| Box::new(task) as Box<dyn SchedulerTask>)
                .collect(),
        )
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

    /// Registers one exact one-shot external readiness source.
    ///
    /// # Errors
    ///
    /// Rejects unknown/terminal tasks, closed state, registration capacity,
    /// or typed identity exhaustion before retaining the source.
    pub fn register_external_event(
        &self,
        task: SchedulerTaskId,
    ) -> Result<SchedulerExternalEventId, SchedulerError> {
        self.shared.ensure_open()?;
        if self.shared.task_state(task)?.terminal() {
            return Err(SchedulerError::UnknownTask(task));
        }
        self.event_driver.register_event(task, &self.shared)
    }

    /// Signals one exact external readiness source at most once.
    ///
    /// # Errors
    ///
    /// Rejects unknown sources, closed state, or delivery-queue saturation.
    pub fn signal_external_event(
        &self,
        event: SchedulerExternalEventId,
    ) -> Result<bool, SchedulerError> {
        self.shared.ensure_open()?;
        self.event_driver.signal_event(event, &self.shared)
    }

    /// Releases one retained external source and any pending delivery.
    ///
    /// # Errors
    ///
    /// Rejects an unknown source.
    pub fn release_external_event(
        &self,
        event: SchedulerExternalEventId,
    ) -> Result<(), SchedulerError> {
        self.event_driver.release_event(event)
    }

    /// Registers a bounded one-shot host timer.
    ///
    /// # Errors
    ///
    /// Rejects unknown/terminal tasks, closed state, deadline overflow, timer
    /// capacity, or typed identity exhaustion before retaining the timer.
    pub fn schedule_wake_after(
        &self,
        task: SchedulerTaskId,
        delay: Duration,
    ) -> Result<SchedulerTimerId, SchedulerError> {
        self.shared.ensure_open()?;
        if self.shared.task_state(task)?.terminal() {
            return Err(SchedulerError::UnknownTask(task));
        }
        self.event_driver.schedule_timer(task, delay, &self.shared)
    }

    /// Cancels one retained one-shot timer before delivery.
    ///
    /// # Errors
    ///
    /// Rejects an unknown or already delivered timer.
    pub fn cancel_timer(&self, timer: SchedulerTimerId) -> Result<(), SchedulerError> {
        self.event_driver.cancel_timer(timer)
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

    /// Stops and joins all workers, returning the final scheduler telemetry.
    ///
    /// # Errors
    ///
    /// Reports a worker that terminated outside the scheduler panic boundary.
    pub fn shutdown_with_telemetry(mut self) -> Result<SchedulerTelemetry, SchedulerError> {
        self.shutdown_threads()?;
        Ok(self.shared.telemetry())
    }

    fn shutdown_threads(&mut self) -> Result<(), SchedulerError> {
        let mut failure = None;
        self.event_driver.shutdown();
        if self
            .event_driver_thread
            .take()
            .is_some_and(|thread| thread.join().is_err())
        {
            failure = Some(SchedulerError::ThreadJoin);
        }
        self.blocking.shutdown();
        for thread in self.blocking_threads.drain(..) {
            if thread.join().is_err() {
                failure.get_or_insert(SchedulerError::ThreadJoin);
            }
        }
        self.shared.shutdown();
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
        if let Err(error) = self.shared.release_all_task_frames() {
            failure.get_or_insert(error);
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
            let depth = telemetry.telemetry.blocking_queue_depth.saturating_add(1);
            telemetry.telemetry.blocking_queue_depth = depth;
            telemetry.telemetry.maximum_blocking_queue_depth =
                telemetry.telemetry.maximum_blocking_queue_depth.max(depth);
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

impl EventDriver {
    fn new(configuration: SchedulerConfiguration) -> Self {
        Self {
            state: Mutex::new(EventDriverState {
                events: BTreeMap::new(),
                timers: BTreeMap::new(),
                deliveries: VecDeque::new(),
                next_event: 1,
                next_timer: 1,
                shutdown: false,
            }),
            changed: Condvar::new(),
            event_capacity: configuration.external_event_capacity,
            timer_capacity: configuration.timer_capacity,
            delivery_capacity: configuration.event_delivery_capacity,
        }
    }

    fn register_event(
        &self,
        task: SchedulerTaskId,
        scheduler: &SharedScheduler,
    ) -> Result<SchedulerExternalEventId, SchedulerError> {
        let mut state = lock(&self.state);
        if state.shutdown {
            return Err(SchedulerError::Closed);
        }
        if state.events.len() >= self.event_capacity {
            return Err(SchedulerError::ExternalEventCapacity);
        }
        let event = SchedulerExternalEventId::new(state.next_event);
        state.next_event = state
            .next_event
            .checked_add(1)
            .ok_or(SchedulerError::IdentityOverflow)?;
        state.events.insert(
            event,
            ExternalEventRegistration {
                task,
                signalled: false,
                delivered: false,
            },
        );
        let mut telemetry = lock(&scheduler.telemetry);
        telemetry.telemetry.external_events_registered = telemetry
            .telemetry
            .external_events_registered
            .saturating_add(1);
        Ok(event)
    }

    fn signal_event(
        &self,
        event: SchedulerExternalEventId,
        scheduler: &SharedScheduler,
    ) -> Result<bool, SchedulerError> {
        let mut state = lock(&self.state);
        if state.shutdown {
            return Err(SchedulerError::Closed);
        }
        let registration = state
            .events
            .get(&event)
            .ok_or(SchedulerError::UnknownExternalEvent(event))?;
        if registration.signalled || registration.delivered {
            let mut telemetry = lock(&scheduler.telemetry);
            telemetry.telemetry.external_event_signals_coalesced = telemetry
                .telemetry
                .external_event_signals_coalesced
                .saturating_add(1);
            return Ok(false);
        }
        if state.deliveries.len() >= self.delivery_capacity {
            return Err(SchedulerError::EventDeliveryCapacity);
        }
        state
            .events
            .get_mut(&event)
            .expect("validated event remains registered")
            .signalled = true;
        state.deliveries.push_back(event);
        drop(state);
        self.changed.notify_one();
        Ok(true)
    }

    fn release_event(&self, event: SchedulerExternalEventId) -> Result<(), SchedulerError> {
        let mut state = lock(&self.state);
        state
            .events
            .remove(&event)
            .ok_or(SchedulerError::UnknownExternalEvent(event))?;
        state.deliveries.retain(|pending| *pending != event);
        Ok(())
    }

    fn schedule_timer(
        &self,
        task: SchedulerTaskId,
        delay: Duration,
        scheduler: &SharedScheduler,
    ) -> Result<SchedulerTimerId, SchedulerError> {
        let deadline = Instant::now()
            .checked_add(delay)
            .ok_or(SchedulerError::IdentityOverflow)?;
        let mut state = lock(&self.state);
        if state.shutdown {
            return Err(SchedulerError::Closed);
        }
        if state.timers.len() >= self.timer_capacity {
            return Err(SchedulerError::TimerCapacity);
        }
        let timer = SchedulerTimerId::new(state.next_timer);
        state.next_timer = state
            .next_timer
            .checked_add(1)
            .ok_or(SchedulerError::IdentityOverflow)?;
        state.timers.insert((deadline, timer), task);
        let mut telemetry = lock(&scheduler.telemetry);
        telemetry.telemetry.timers_scheduled =
            telemetry.telemetry.timers_scheduled.saturating_add(1);
        drop(telemetry);
        drop(state);
        self.changed.notify_one();
        Ok(timer)
    }

    fn cancel_timer(&self, timer: SchedulerTimerId) -> Result<(), SchedulerError> {
        let mut state = lock(&self.state);
        let key = state
            .timers
            .keys()
            .find(|(_, candidate)| *candidate == timer)
            .copied()
            .ok_or(SchedulerError::UnknownTimer(timer))?;
        state.timers.remove(&key);
        Ok(())
    }

    fn shutdown(&self) {
        let mut state = lock(&self.state);
        state.shutdown = true;
        state.deliveries.clear();
        state.timers.clear();
        state.events.clear();
        drop(state);
        self.changed.notify_all();
    }
}

impl SharedScheduler {
    fn new(
        configuration: SchedulerConfiguration,
        runtime_transitions: Arc<dyn SchedulerRuntimeTransitions>,
        migration_enabled: bool,
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
            migration_enabled,
            submissions_active: AtomicUsize::new(0),
            observation_work: AtomicU64::new(0),
        }
    }

    fn advance_observation_work(&self) -> u64 {
        self.observation_work
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |work| {
                Some(work.saturating_add(1))
            })
            .unwrap_or(u64::MAX)
            .saturating_add(1)
    }

    fn task_frame_error(
        &self,
        task: SchedulerTaskId,
        failure: SchedulerTaskFrameFailure,
    ) -> SchedulerError {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.frame_root_failures =
            telemetry.telemetry.frame_root_failures.saturating_add(1);
        SchedulerError::TaskFrame { task, failure }
    }

    const fn collector_binding_error(
        worker: SchedulerWorkerId,
        failure: SchedulerCollectorBindingFailure,
    ) -> SchedulerError {
        SchedulerError::CollectorBinding { worker, failure }
    }

    fn register_worker_mutator(
        &self,
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
    ) -> Result<MutatorId, SchedulerError> {
        let mutator = crate::state::register_scheduler_mutator(scheduler).map_err(|_| {
            Self::collector_binding_error(worker, SchedulerCollectorBindingFailure::Registration)
        })?;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.mutator_registrations =
            telemetry.telemetry.mutator_registrations.saturating_add(1);
        Ok(mutator)
    }

    fn enter_worker_managed(
        &self,
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
        mutator: MutatorId,
    ) -> Result<(), SchedulerError> {
        crate::state::enter_native_managed_execution(scheduler, mutator).map_err(|_| {
            Self::collector_binding_error(
                worker,
                SchedulerCollectorBindingFailure::ManagedTransition,
            )
        })?;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.managed_mutator_transitions = telemetry
            .telemetry
            .managed_mutator_transitions
            .saturating_add(1);
        Ok(())
    }

    fn leave_worker_managed(
        &self,
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
        mutator: MutatorId,
    ) -> Result<(), SchedulerError> {
        crate::state::leave_native_managed_execution(scheduler, mutator).map_err(|_| {
            Self::collector_binding_error(
                worker,
                SchedulerCollectorBindingFailure::DetachedTransition,
            )
        })?;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.detached_mutator_transitions = telemetry
            .telemetry
            .detached_mutator_transitions
            .saturating_add(1);
        Ok(())
    }

    fn unregister_worker_mutator(
        &self,
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
        mutator: MutatorId,
    ) -> Result<(), SchedulerError> {
        crate::state::unregister_scheduler_mutator(scheduler, mutator).map_err(|_| {
            Self::collector_binding_error(worker, SchedulerCollectorBindingFailure::Unregistration)
        })?;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.mutator_unregistrations = telemetry
            .telemetry
            .mutator_unregistrations
            .saturating_add(1);
        Ok(())
    }

    fn retain_task_frame(
        &self,
        id: SchedulerTaskId,
        scheduler: SchedulerId,
        task: &mut dyn SchedulerTask,
    ) -> Result<TaskFrameRootId, SchedulerError> {
        let expected = task.frame_stack_map();
        let publication = task
            .publish_frame_roots()
            .map_err(|_| self.task_frame_error(id, SchedulerTaskFrameFailure::Publication))?;
        if publication.stack_map() != &expected {
            return Err(self.task_frame_error(id, SchedulerTaskFrameFailure::PublicationShape));
        }
        let identity = crate::state::retain_scheduler_task_roots(scheduler, &publication)
            .map_err(|_| self.task_frame_error(id, SchedulerTaskFrameFailure::Collector))?;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.retained_frame_root_containers = telemetry
            .telemetry
            .retained_frame_root_containers
            .saturating_add(1);
        telemetry.telemetry.maximum_retained_frame_root_containers = telemetry
            .telemetry
            .maximum_retained_frame_root_containers
            .max(telemetry.telemetry.retained_frame_root_containers);
        telemetry.telemetry.frame_root_retentions =
            telemetry.telemetry.frame_root_retentions.saturating_add(1);
        Ok(identity)
    }

    fn prepare_task_frame_restore(
        &self,
        id: SchedulerTaskId,
        scheduler: SchedulerId,
        identity: TaskFrameRootId,
        task: &mut dyn SchedulerTask,
    ) -> Result<(), SchedulerError> {
        let expected = task.frame_stack_map();
        let publication =
            crate::state::prepare_scheduler_task_root_restore(identity, scheduler, &expected)
                .map_err(|_| self.task_frame_error(id, SchedulerTaskFrameFailure::Collector))?;
        task.restore_frame_roots(publication)
            .map_err(|_| self.task_frame_error(id, SchedulerTaskFrameFailure::Restoration))
    }

    fn complete_task_frame_restore(
        &self,
        id: SchedulerTaskId,
        scheduler: SchedulerId,
        identity: TaskFrameRootId,
    ) -> Result<(), SchedulerError> {
        crate::state::complete_scheduler_task_root_restore(identity, scheduler)
            .map_err(|_| self.task_frame_error(id, SchedulerTaskFrameFailure::Collector))?;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.retained_frame_root_containers = telemetry
            .telemetry
            .retained_frame_root_containers
            .saturating_sub(1);
        telemetry.telemetry.frame_root_restorations = telemetry
            .telemetry
            .frame_root_restorations
            .saturating_add(1);
        Ok(())
    }

    fn release_task_frame(
        &self,
        id: SchedulerTaskId,
        scheduler: SchedulerId,
        identity: TaskFrameRootId,
    ) -> Result<(), SchedulerError> {
        crate::state::release_scheduler_task_roots(identity, scheduler)
            .map_err(|_| self.task_frame_error(id, SchedulerTaskFrameFailure::Collector))?;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.retained_frame_root_containers = telemetry
            .telemetry
            .retained_frame_root_containers
            .saturating_sub(1);
        telemetry.telemetry.frame_root_releases =
            telemetry.telemetry.frame_root_releases.saturating_add(1);
        Ok(())
    }

    fn release_all_task_frames(&self) -> Result<(), SchedulerError> {
        let tasks = lock(&self.registry)
            .tasks
            .iter()
            .map(|(id, cell)| (*id, Arc::clone(cell)))
            .collect::<Vec<_>>();
        let mut first_failure = None;
        for (id, cell) in tasks {
            let mut record = lock(&cell.record);
            let Some(identity) = record.frame_roots.take() else {
                continue;
            };
            if let Err(error) = self.release_task_frame(id, record.scheduler, identity) {
                record.frame_roots = Some(identity);
                first_failure.get_or_insert(error);
            }
        }
        first_failure.map_or(Ok(()), Err)
    }

    fn schedule_injected(
        &self,
        mut task: Box<dyn SchedulerTask>,
    ) -> Result<SchedulerTaskId, SchedulerError> {
        self.ensure_open()?;
        let _submission = SubmissionGuard::enter(&self.submissions_active, &self.searchers);
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
        let id = SchedulerTaskId::new(registry.next_task);
        let next_task = registry
            .next_task
            .checked_add(1)
            .ok_or(SchedulerError::IdentityOverflow)?;
        let frame_roots = self.retain_task_frame(id, scheduler, task.as_mut())?;
        let ready_since_work = self.advance_observation_work();
        registry.next_task = next_task;
        registry.tasks.insert(
            id,
            Arc::new(TaskCell {
                record: Mutex::new(TaskRecord {
                    task: Some(task),
                    state: InternalTaskState::Ready,
                    scheduler,
                    mobility: SchedulerTaskMobility::Movable,
                    cancellation_requested: false,
                    frame_roots: Some(frame_roots),
                    ready_since_work,
                }),
            }),
        );
        injection.push_back(id);
        self.record_injection_enqueued();
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
        mut task: Box<dyn SchedulerTask>,
    ) -> Result<SchedulerTaskId, SchedulerError> {
        self.ensure_open()?;
        let _submission = SubmissionGuard::enter(&self.submissions_active, &self.searchers);
        let index = self.scheduler_index(scheduler)?;
        let mut queue = lock(&self.queues.local[index]);
        if queue.len() >= self.configuration.local_queue_capacity {
            return Err(SchedulerError::LocalQueueCapacity);
        }
        let mut registry = lock(&self.registry);
        if registry.tasks.len() >= self.configuration.task_capacity {
            return Err(SchedulerError::TaskCapacity);
        }
        let id = SchedulerTaskId::new(registry.next_task);
        let next_task = registry
            .next_task
            .checked_add(1)
            .ok_or(SchedulerError::IdentityOverflow)?;
        let frame_roots = self.retain_task_frame(id, scheduler, task.as_mut())?;
        let ready_since_work = self.advance_observation_work();
        registry.next_task = next_task;
        registry.tasks.insert(
            id,
            Arc::new(TaskCell {
                record: Mutex::new(TaskRecord {
                    task: Some(task),
                    state: InternalTaskState::Ready,
                    scheduler,
                    mobility,
                    cancellation_requested: false,
                    frame_roots: Some(frame_roots),
                    ready_since_work,
                }),
            }),
        );
        queue.push_back(id);
        self.record_local_enqueued(1);
        self.record_scheduled(&registry);
        drop(registry);
        self.increment_ready();
        drop(queue);
        self.notify_work();
        Ok(id)
    }

    fn schedule_local_batch(
        &self,
        scheduler: SchedulerId,
        mobility: SchedulerTaskMobility,
        tasks: Vec<Box<dyn SchedulerTask>>,
    ) -> Result<Vec<SchedulerTaskId>, SchedulerError> {
        self.ensure_open()?;
        let _submission = SubmissionGuard::enter(&self.submissions_active, &self.searchers);
        if tasks.is_empty() {
            return Ok(Vec::new());
        }
        let index = self.scheduler_index(scheduler)?;
        let mut queue = lock(&self.queues.local[index]);
        if queue.len().saturating_add(tasks.len()) > self.configuration.local_queue_capacity {
            return Err(SchedulerError::LocalQueueCapacity);
        }
        let mut registry = lock(&self.registry);
        if registry.tasks.len().saturating_add(tasks.len()) > self.configuration.task_capacity {
            return Err(SchedulerError::TaskCapacity);
        }
        let additional =
            u64::try_from(tasks.len()).map_err(|_| SchedulerError::IdentityOverflow)?;
        let next_task = registry
            .next_task
            .checked_add(additional)
            .ok_or(SchedulerError::IdentityOverflow)?;

        let mut ids = Vec::with_capacity(tasks.len());
        let mut prepared = Vec::with_capacity(tasks.len());
        for (offset, mut task) in tasks.into_iter().enumerate() {
            let offset = u64::try_from(offset).map_err(|_| SchedulerError::IdentityOverflow)?;
            let raw = registry
                .next_task
                .checked_add(offset)
                .ok_or(SchedulerError::IdentityOverflow)?;
            let id = SchedulerTaskId::new(raw);
            let frame_roots = match self.retain_task_frame(id, scheduler, task.as_mut()) {
                Ok(frame_roots) => frame_roots,
                Err(error) => {
                    let mut cleanup_failure = None;
                    for (prepared_id, _, prepared_roots) in prepared {
                        if let Err(cleanup) =
                            self.release_task_frame(prepared_id, scheduler, prepared_roots)
                        {
                            cleanup_failure.get_or_insert(cleanup);
                        }
                    }
                    return Err(cleanup_failure.unwrap_or(error));
                }
            };
            prepared.push((id, task, frame_roots));
        }
        registry.next_task = next_task;
        for (id, task, frame_roots) in prepared {
            let ready_since_work = self.advance_observation_work();
            registry.tasks.insert(
                id,
                Arc::new(TaskCell {
                    record: Mutex::new(TaskRecord {
                        task: Some(task),
                        state: InternalTaskState::Ready,
                        scheduler,
                        mobility,
                        cancellation_requested: false,
                        frame_roots: Some(frame_roots),
                        ready_since_work,
                    }),
                }),
            );
            queue.push_back(id);
            ids.push(id);
        }
        self.record_local_enqueued(ids.len());
        {
            let mut telemetry = lock(&self.telemetry);
            telemetry.telemetry.tasks_scheduled = telemetry
                .telemetry
                .tasks_scheduled
                .saturating_add(u64::try_from(ids.len()).unwrap_or(u64::MAX));
            telemetry.telemetry.retained_tasks = registry.tasks.len();
            telemetry.telemetry.ready_tasks =
                telemetry.telemetry.ready_tasks.saturating_add(ids.len());
        }
        drop(registry);
        {
            let mut activity = lock(&self.activity);
            activity.ready = activity.ready.saturating_add(ids.len());
        }
        drop(queue);
        self.notify_work();
        Ok(ids)
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
                record.ready_since_work = self.advance_observation_work();
                record.state = InternalTaskState::Ready;
                queue.push_back(id);
                self.record_local_enqueued(1);
                drop(record);
                self.record_resumed();
                self.increment_ready();
                drop(queue);
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
                record.ready_since_work = self.advance_observation_work();
                record.state = InternalTaskState::Ready;
                queue.push_back(id);
                self.record_local_enqueued(1);
                drop(record);
                self.record_resumed();
                self.increment_ready();
                drop(queue);
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
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.retained_tasks = registry.tasks.len();
        telemetry.telemetry.terminal_tasks = telemetry.telemetry.terminal_tasks.saturating_sub(1);
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
            self.record_worker_parked();
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
            self.record_worker_unparked();
        }
    }

    fn try_take(&self, index: usize, local_polls: usize) -> Option<QueuedTask> {
        let check_injection =
            local_polls.is_multiple_of(self.configuration.injection_poll_interval);
        if check_injection && let Some(id) = self.take_injection(index) {
            return Some(QueuedTask {
                id,
                source: WorkSource::Injection,
            });
        }
        if let Some(id) = lock(&self.queues.local[index]).pop_front() {
            self.record_local_dequeued(1);
            return Some(QueuedTask {
                id,
                source: WorkSource::Local,
            });
        }
        if !check_injection && let Some(id) = self.take_injection(index) {
            return Some(QueuedTask {
                id,
                source: WorkSource::Injection,
            });
        }
        self.try_steal(index)
    }

    fn take_injection(&self, index: usize) -> Option<SchedulerTaskId> {
        let destination = scheduler_id(index);
        let mut injection = lock(&self.queues.injection);
        let owner_position = {
            let registry = lock(&self.registry);
            injection.iter().position(|id| {
                registry
                    .tasks
                    .get(id)
                    .is_some_and(|cell| lock(&cell.record).scheduler == destination)
            })
        };
        if let Some(position) = owner_position {
            let id = injection.remove(position);
            if id.is_some() {
                self.record_injection_dequeued();
            }
            return id;
        }
        if !self.migration_enabled {
            return None;
        }
        let id = injection.pop_front()?;
        drop(injection);
        if self.assign_scheduler(id, index) {
            self.record_injection_dequeued();
            Some(id)
        } else {
            lock(&self.queues.injection).push_back(id);
            None
        }
    }

    fn try_steal(&self, thief: usize) -> Option<QueuedTask> {
        if !self.migration_enabled {
            return None;
        }
        if !self.enter_steal_search() {
            return None;
        }
        let mut result = None;
        let mut victims_examined = 0;
        for offset in 1..self.configuration.scheduler_count {
            let victim = (thief + offset) % self.configuration.scheduler_count;
            victims_examined += 1;
            let Some(mut victim_queue) = try_lock(&self.queues.local[victim]) else {
                continue;
            };
            if victim_queue.is_empty() {
                continue;
            }
            let Some(registry) = try_lock(&self.registry) else {
                continue;
            };
            let eligible = victim_queue
                .iter()
                .filter(|id| {
                    registry.tasks.get(id).is_some_and(|cell| {
                        try_lock(&cell.record)
                            .is_some_and(|record| record.mobility == SchedulerTaskMobility::Movable)
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
                let Some(mut record) = try_lock(&cell.record) else {
                    continue;
                };
                if record.mobility != SchedulerTaskMobility::Movable {
                    continue;
                }
                let Some(position) = victim_queue.iter().position(|candidate| *candidate == id)
                else {
                    continue;
                };
                if !self.migrate_task_record(id, &mut record, scheduler_id(thief)) {
                    continue;
                }
                victim_queue.remove(position);
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
            self.record_local_dequeued(1);
            result = Some(QueuedTask {
                id: first,
                source: WorkSource::Stolen(batch),
            });
            break;
        }
        self.searchers.fetch_sub(1, Ordering::AcqRel);
        self.record_steal_search(
            victims_examined,
            result.as_ref().map(|queued| match queued.source {
                WorkSource::Stolen(batch) => batch,
                WorkSource::Local | WorkSource::Injection => 0,
            }),
        );
        result
    }

    fn enter_steal_search(&self) -> bool {
        if self.submissions_active.load(Ordering::Acquire) != 0 {
            return false;
        }
        let maximum_searchers = self.configuration.worker_count.div_ceil(2);
        let entered = self
            .searchers
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |searchers| {
                (searchers < maximum_searchers).then_some(searchers + 1)
            })
            .is_ok();
        if !entered {
            return false;
        }
        if self.submissions_active.load(Ordering::Acquire) == 0 {
            true
        } else {
            self.searchers.fetch_sub(1, Ordering::AcqRel);
            self.record_steal_search(0, None);
            false
        }
    }

    fn assign_scheduler(&self, id: SchedulerTaskId, index: usize) -> bool {
        if let Ok(cell) = self.task(id) {
            let mut record = lock(&cell.record);
            if record.mobility == SchedulerTaskMobility::Movable {
                return self.migrate_task_record(id, &mut record, scheduler_id(index));
            }
        }
        true
    }

    fn migrate_task_record(
        &self,
        id: SchedulerTaskId,
        record: &mut TaskRecord,
        destination: SchedulerId,
    ) -> bool {
        if record.scheduler == destination {
            return true;
        }
        if !self.migration_allowed(id, record.scheduler, destination) {
            return false;
        }
        let Some(frame_roots) = record.frame_roots else {
            return false;
        };
        if crate::state::transfer_scheduler_task_roots(frame_roots, record.scheduler, destination)
            .is_err()
        {
            let mut telemetry = lock(&self.telemetry);
            telemetry.telemetry.gc_delayed_migrations =
                telemetry.telemetry.gc_delayed_migrations.saturating_add(1);
            return false;
        }
        record.scheduler = destination;
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.scheduler_migrations =
            telemetry.telemetry.scheduler_migrations.saturating_add(1);
        true
    }

    fn begin_poll(
        &self,
        queued: &QueuedTask,
        worker: SchedulerWorkerId,
        mutator: MutatorId,
    ) -> Result<Option<StartedTask>, SchedulerError> {
        let cell = self.task(queued.id)?;
        let mut record = lock(&cell.record);
        if !matches!(record.state, InternalTaskState::Ready) {
            drop(record);
            self.discard_stale_ready_entry();
            return Ok(None);
        }
        let Some(frame_roots) = record.frame_roots else {
            return Err(self.task_frame_error(queued.id, SchedulerTaskFrameFailure::Collector));
        };
        let task_scheduler = record.scheduler;
        let Some(task) = record.task.as_mut() else {
            return Err(self.task_frame_error(queued.id, SchedulerTaskFrameFailure::Restoration));
        };
        self.prepare_task_frame_restore(queued.id, task_scheduler, frame_roots, task.as_mut())?;
        self.enter_worker_managed(worker, task_scheduler, mutator)?;
        if let Err(error) =
            self.require_runtime_transition(SchedulerRuntimeTransition::TaskDispatched {
                task: queued.id,
                worker,
                scheduler: record.scheduler,
            })
        {
            return Err(self
                .leave_worker_managed(worker, task_scheduler, mutator)
                .err()
                .unwrap_or(error));
        }
        if let Err(error) =
            self.complete_task_frame_restore(queued.id, record.scheduler, frame_roots)
        {
            return Err(self
                .leave_worker_managed(worker, task_scheduler, mutator)
                .err()
                .unwrap_or(error));
        }
        let ready_delay = self
            .advance_observation_work()
            .saturating_sub(record.ready_since_work);
        record.frame_roots = None;
        record.state = InternalTaskState::Running { notified: false };
        let Some(task) = record.task.take() else {
            let error = self.task_frame_error(queued.id, SchedulerTaskFrameFailure::Restoration);
            return Err(self
                .leave_worker_managed(worker, task_scheduler, mutator)
                .err()
                .unwrap_or(error));
        };
        let context = SchedulerTaskContext::new(
            queued.id,
            record.scheduler,
            worker,
            record.cancellation_requested,
            self.configuration.dispatch_work_budget,
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
            telemetry.telemetry.ready_to_run_delay.record(ready_delay);
            telemetry.telemetry.ready_tasks = telemetry.telemetry.ready_tasks.saturating_sub(1);
            telemetry.telemetry.running_tasks = telemetry.telemetry.running_tasks.saturating_add(1);
            telemetry.workers_used.insert(worker);
            if let WorkSource::Stolen(batch) = queued.source {
                let _ = batch;
            }
        }
        Ok(Some((cell, task, context)))
    }

    fn finish_poll(
        &self,
        id: SchedulerTaskId,
        cell: &TaskCell,
        worker: SchedulerWorkerId,
        mutator: MutatorId,
        mut task: Box<dyn SchedulerTask>,
        result: Result<SchedulerTaskPoll, Box<dyn std::any::Any + Send>>,
    ) -> Result<(), SchedulerError> {
        let mut record = lock(&cell.record);
        let notified = matches!(record.state, InternalTaskState::Running { notified: true });
        let scheduler = record.scheduler;
        let frame_roots = if matches!(
            &result,
            Ok(SchedulerTaskPoll::Ready | SchedulerTaskPoll::Pending)
        ) {
            match self.retain_task_frame(id, scheduler, task.as_mut()) {
                Ok(frame_roots) => Some(frame_roots),
                Err(error) => {
                    let _ = self.leave_worker_managed(worker, scheduler, mutator);
                    return Err(error);
                }
            }
        } else {
            None
        };
        if let Err(error) = self.leave_worker_managed(worker, scheduler, mutator) {
            if let Some(frame_roots) = frame_roots {
                record.task = Some(task);
                record.frame_roots = Some(frame_roots);
            }
            return Err(error);
        }
        let enqueue;
        let terminal_state = match result {
            Ok(SchedulerTaskPoll::Ready) => {
                let frame_roots = frame_roots.ok_or_else(|| {
                    self.task_frame_error(id, SchedulerTaskFrameFailure::Collector)
                })?;
                enqueue = self.finish_nonterminal_poll(
                    id,
                    &mut record,
                    task,
                    frame_roots,
                    NonterminalPollState::Ready,
                )?;
                None
            }
            Ok(SchedulerTaskPoll::Pending) if notified => {
                let frame_roots = frame_roots.ok_or_else(|| {
                    self.task_frame_error(id, SchedulerTaskFrameFailure::Collector)
                })?;
                enqueue = self.finish_nonterminal_poll(
                    id,
                    &mut record,
                    task,
                    frame_roots,
                    NonterminalPollState::Resumed,
                )?;
                None
            }
            Ok(SchedulerTaskPoll::Pending) => {
                let frame_roots = frame_roots.ok_or_else(|| {
                    self.task_frame_error(id, SchedulerTaskFrameFailure::Collector)
                })?;
                enqueue = self.finish_nonterminal_poll(
                    id,
                    &mut record,
                    task,
                    frame_roots,
                    NonterminalPollState::Suspended,
                )?;
                None
            }
            Ok(SchedulerTaskPoll::Complete) => {
                enqueue = false;
                self.finish_terminal_poll(id, &mut record, SchedulerTaskState::Completed)?;
                Some(SchedulerTaskState::Completed)
            }
            Ok(SchedulerTaskPoll::Cancelled) => {
                enqueue = false;
                self.finish_terminal_poll(id, &mut record, SchedulerTaskState::Cancelled)?;
                Some(SchedulerTaskState::Cancelled)
            }
            Err(panic) => {
                enqueue = false;
                drop(panic);
                self.finish_terminal_poll(id, &mut record, SchedulerTaskState::Panicked)?;
                Some(SchedulerTaskState::Panicked)
            }
        };
        drop(record);
        self.publish_finished_poll_state(id, scheduler, enqueue, terminal_state)
    }

    fn publish_finished_poll_state(
        &self,
        id: SchedulerTaskId,
        scheduler: SchedulerId,
        enqueue: bool,
        terminal_state: Option<SchedulerTaskState>,
    ) -> Result<(), SchedulerError> {
        {
            let mut telemetry = lock(&self.telemetry);
            telemetry.telemetry.running_tasks = telemetry.telemetry.running_tasks.saturating_sub(1);
            if enqueue {
                telemetry.telemetry.ready_tasks = telemetry.telemetry.ready_tasks.saturating_add(1);
            } else if terminal_state.is_some() {
                telemetry.telemetry.terminal_tasks =
                    telemetry.telemetry.terminal_tasks.saturating_add(1);
            } else {
                telemetry.telemetry.suspended_tasks =
                    telemetry.telemetry.suspended_tasks.saturating_add(1);
            }
        }
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
            let index = self.scheduler_index(scheduler)?;
            lock(&self.queues.local[index]).push_back(id);
            self.record_local_enqueued(1);
        }
        if enqueue {
            self.notify_work();
        }
        Ok(())
    }

    fn finish_terminal_poll(
        &self,
        id: SchedulerTaskId,
        record: &mut TaskRecord,
        state: SchedulerTaskState,
    ) -> Result<(), SchedulerError> {
        self.require_runtime_transition(SchedulerRuntimeTransition::TaskTerminal {
            task: id,
            scheduler: record.scheduler,
            state,
        })?;
        let mut telemetry = lock(&self.telemetry);
        match state {
            SchedulerTaskState::Completed => {
                record.state = InternalTaskState::Completed;
                telemetry.telemetry.completions = telemetry.telemetry.completions.saturating_add(1);
            }
            SchedulerTaskState::Cancelled => {
                record.state = InternalTaskState::Cancelled;
                telemetry.telemetry.cancellations_observed =
                    telemetry.telemetry.cancellations_observed.saturating_add(1);
            }
            SchedulerTaskState::Panicked => {
                record.state = InternalTaskState::Panicked;
                telemetry.telemetry.panics = telemetry.telemetry.panics.saturating_add(1);
            }
            SchedulerTaskState::Ready
            | SchedulerTaskState::Running
            | SchedulerTaskState::Suspended => unreachable!("terminal poll state"),
        }
        Ok(())
    }

    fn finish_nonterminal_poll(
        &self,
        id: SchedulerTaskId,
        record: &mut TaskRecord,
        task: Box<dyn SchedulerTask>,
        frame_roots: TaskFrameRootId,
        state: NonterminalPollState,
    ) -> Result<bool, SchedulerError> {
        let scheduler = record.scheduler;
        let transition = match state {
            NonterminalPollState::Ready => Ok(()),
            NonterminalPollState::Resumed => {
                self.require_runtime_transition(SchedulerRuntimeTransition::TaskResumed {
                    task: id,
                    scheduler,
                })
            }
            NonterminalPollState::Suspended => {
                self.require_runtime_transition(SchedulerRuntimeTransition::TaskSuspended {
                    task: id,
                    scheduler,
                })
            }
        };
        if let Err(error) = transition {
            record.task = Some(task);
            record.frame_roots = Some(frame_roots);
            return Err(error);
        }
        record.task = Some(task);
        record.frame_roots = Some(frame_roots);
        if matches!(state, NonterminalPollState::Suspended) {
            record.state = InternalTaskState::Suspended;
            let mut telemetry = lock(&self.telemetry);
            telemetry.telemetry.suspensions = telemetry.telemetry.suspensions.saturating_add(1);
            Ok(false)
        } else {
            record.ready_since_work = self.advance_observation_work();
            record.state = InternalTaskState::Ready;
            Ok(true)
        }
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
    }

    fn discard_stale_ready_entry(&self) {
        let mut activity = lock(&self.activity);
        activity.ready = activity.ready.saturating_sub(1);
        if activity.ready == 0 && activity.running == 0 {
            self.idle.notify_all();
        }
        drop(activity);
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.stale_ready_entries =
            telemetry.telemetry.stale_ready_entries.saturating_add(1);
    }

    fn notify_work(&self) {
        let _idle = lock(&self.queues.idle_gate);
        // The correctness scheduler has one shared park set. Broadcasting is
        // required because a single arbitrary wake can select a worker that
        // cannot run scheduler-affine work while its owner remains parked.
        self.queues.work_available.notify_all();
    }

    fn record_local_enqueued(&self, count: usize) {
        let mut telemetry = lock(&self.telemetry);
        let depth = telemetry.telemetry.local_queue_depth.saturating_add(count);
        telemetry.telemetry.local_queue_depth = depth;
        telemetry.telemetry.maximum_local_queue_depth =
            telemetry.telemetry.maximum_local_queue_depth.max(depth);
    }

    fn record_local_dequeued(&self, count: usize) {
        let mut telemetry = lock(&self.telemetry);
        debug_assert!(telemetry.telemetry.local_queue_depth >= count);
        telemetry.telemetry.local_queue_depth =
            telemetry.telemetry.local_queue_depth.saturating_sub(count);
    }

    fn record_injection_enqueued(&self) {
        let mut telemetry = lock(&self.telemetry);
        let depth = telemetry.telemetry.injection_queue_depth.saturating_add(1);
        telemetry.telemetry.injection_queue_depth = depth;
        telemetry.telemetry.maximum_injection_queue_depth =
            telemetry.telemetry.maximum_injection_queue_depth.max(depth);
    }

    fn record_injection_dequeued(&self) {
        let mut telemetry = lock(&self.telemetry);
        debug_assert!(telemetry.telemetry.injection_queue_depth > 0);
        telemetry.telemetry.injection_queue_depth =
            telemetry.telemetry.injection_queue_depth.saturating_sub(1);
    }

    fn record_steal_search(&self, victims_examined: usize, stolen_batch: Option<usize>) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.steal_searches = telemetry.telemetry.steal_searches.saturating_add(1);
        telemetry.telemetry.steal_victims_examined = telemetry
            .telemetry
            .steal_victims_examined
            .saturating_add(u64::try_from(victims_examined).unwrap_or(u64::MAX));
        if let Some(batch) = stolen_batch {
            telemetry.telemetry.steal_successes =
                telemetry.telemetry.steal_successes.saturating_add(1);
            telemetry.telemetry.maximum_stolen_batch =
                telemetry.telemetry.maximum_stolen_batch.max(batch);
        } else {
            telemetry.telemetry.steal_failures =
                telemetry.telemetry.steal_failures.saturating_add(1);
        }
    }

    fn record_worker_started(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.worker_starts = telemetry.telemetry.worker_starts.saturating_add(1);
    }

    fn record_worker_parked(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.worker_parks = telemetry.telemetry.worker_parks.saturating_add(1);
    }

    fn record_worker_unparked(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.worker_unparks = telemetry.telemetry.worker_unparks.saturating_add(1);
    }

    fn record_worker_stopped(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.worker_stops = telemetry.telemetry.worker_stops.saturating_add(1);
    }

    fn record_work_budget_exhaustion(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.work_budget_exhaustions = telemetry
            .telemetry
            .work_budget_exhaustions
            .saturating_add(1);
    }

    fn record_scheduled(&self, registry: &Registry) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.tasks_scheduled = telemetry.telemetry.tasks_scheduled.saturating_add(1);
        telemetry.telemetry.retained_tasks = registry.tasks.len();
        telemetry.telemetry.ready_tasks = telemetry.telemetry.ready_tasks.saturating_add(1);
    }

    fn record_resumed(&self) {
        let mut telemetry = lock(&self.telemetry);
        telemetry.telemetry.suspended_tasks = telemetry.telemetry.suspended_tasks.saturating_sub(1);
        telemetry.telemetry.ready_tasks = telemetry.telemetry.ready_tasks.saturating_add(1);
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
    let mutator = shared.register_worker_mutator(worker, scheduler)?;
    if let Err(error) = shared
        .require_runtime_transition(SchedulerRuntimeTransition::WorkerStarted { worker, scheduler })
    {
        return Err(shared
            .unregister_worker_mutator(worker, scheduler, mutator)
            .err()
            .unwrap_or(error));
    }
    shared.record_worker_started();
    let work_result = (|| {
        let mut local_polls = 0;
        while let Some(queued) = shared.take_work(worker, scheduler, local_polls)? {
            let local = matches!(queued.source, WorkSource::Local);
            if matches!(queued.source, WorkSource::Stolen(batch) if batch > 1) {
                shared.notify_work();
            }
            let Some((cell, mut task, context)) = shared.begin_poll(&queued, worker, mutator)?
            else {
                continue;
            };
            let mut result = catch_unwind(AssertUnwindSafe(|| task.poll(&context)));
            if context.work_budget_exhausted() {
                shared.record_work_budget_exhaustion();
                if matches!(result, Ok(SchedulerTaskPoll::Pending)) {
                    result = Ok(SchedulerTaskPoll::Ready);
                }
            }
            shared.finish_poll(queued.id, &cell, worker, mutator, task, result)?;
            if local {
                local_polls = local_polls.saturating_add(1);
            } else {
                local_polls = 0;
            }
        }
        Ok(())
    })();
    if work_result.is_err() {
        shared.shutdown();
    }
    let stop_result = shared
        .require_runtime_transition(SchedulerRuntimeTransition::WorkerStopped { worker, scheduler })
        .map(|()| shared.record_worker_stopped());
    crate::state::clear_native_execution_binding();
    let unregister_result = shared.unregister_worker_mutator(worker, scheduler, mutator);
    work_result.and(stop_result).and(unregister_result)
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
        {
            let mut telemetry = lock(&scheduler.telemetry);
            debug_assert!(telemetry.telemetry.blocking_queue_depth > 0);
            telemetry.telemetry.blocking_queue_depth =
                telemetry.telemetry.blocking_queue_depth.saturating_sub(1);
            let active = telemetry
                .telemetry
                .active_blocking_operations
                .saturating_add(1);
            telemetry.telemetry.active_blocking_operations = active;
            telemetry.telemetry.maximum_active_blocking_operations = telemetry
                .telemetry
                .maximum_active_blocking_operations
                .max(active);
        }
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
        debug_assert!(telemetry.telemetry.active_blocking_operations > 0);
        telemetry.telemetry.active_blocking_operations = telemetry
            .telemetry
            .active_blocking_operations
            .saturating_sub(1);
        drop(telemetry);
        let mut state = lock(&pool.state);
        state.active = state.active.saturating_sub(1);
        if state.active == 0 && state.queue.is_empty() {
            pool.idle.notify_all();
        }
    }
}

fn event_driver_loop(driver: &EventDriver, scheduler: &SharedScheduler) {
    loop {
        let mut state = lock(&driver.state);
        let delivery = loop {
            if state.shutdown {
                return;
            }
            if let Some(event) = state.deliveries.pop_front() {
                let Some(registration) = state.events.get_mut(&event) else {
                    continue;
                };
                registration.delivered = true;
                break HostDelivery::ExternalEvent(registration.task);
            }
            let now = Instant::now();
            if let Some((&key, &task)) = state.timers.first_key_value()
                && key.0 <= now
            {
                state.timers.remove(&key);
                break HostDelivery::Timer(task);
            }
            if let Some((&(deadline, _), _)) = state.timers.first_key_value() {
                let timeout = deadline.saturating_duration_since(now);
                let (waiting, _) = driver
                    .changed
                    .wait_timeout(state, timeout)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                state = waiting;
            } else {
                state = driver
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
        };
        drop(state);

        match delivery {
            HostDelivery::ExternalEvent(task) => {
                let _ = scheduler.wake(task);
                let mut telemetry = lock(&scheduler.telemetry);
                telemetry.telemetry.external_events_delivered = telemetry
                    .telemetry
                    .external_events_delivered
                    .saturating_add(1);
            }
            HostDelivery::Timer(task) => {
                let _ = scheduler.wake(task);
                let mut telemetry = lock(&scheduler.telemetry);
                telemetry.telemetry.timers_delivered =
                    telemetry.telemetry.timers_delivered.saturating_add(1);
            }
        }
    }
}

fn remaining_until(deadline: Instant) -> Result<Duration, SchedulerError> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|remaining| !remaining.is_zero())
        .ok_or(SchedulerError::WaitTimedOut)
}

fn scheduler_id(index: usize) -> SchedulerId {
    SchedulerId::new(u32::try_from(index + 1).expect("validated scheduler identity range"))
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn try_lock<T>(mutex: &Mutex<T>) -> Option<MutexGuard<'_, T>> {
    match mutex.try_lock() {
        Ok(guard) => Some(guard),
        Err(TryLockError::WouldBlock) => None,
        Err(TryLockError::Poisoned(poisoned)) => Some(poisoned.into_inner()),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use pop_runtime_interface::{ManagedReference, RootPublication, SafePointId, StackMap};

    use crate::state::allocation_scheduler;
    use crate::{
        SchedulerConfiguration, SchedulerTask, SchedulerTaskContext, SchedulerTaskFrame,
        SchedulerTaskFrameError, SchedulerTaskMobility, SchedulerTaskPoll, SchedulerTaskState,
        pop_rt_allocate_object,
    };

    use super::{NativeScheduler, SchedulerId};

    struct AllocationOwnershipTask;

    impl SchedulerTaskFrame for AllocationOwnershipTask {
        fn frame_stack_map(&self) -> StackMap {
            StackMap::new(SafePointId::new(1_001), Vec::new()).expect("allocation task frame map")
        }

        fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
            RootPublication::new(self.frame_stack_map(), Vec::new())
                .map_err(|_| SchedulerTaskFrameError::PublicationRejected)
        }

        fn restore_frame_roots(
            &mut self,
            publication: RootPublication,
        ) -> Result<(), SchedulerTaskFrameError> {
            if publication.stack_map() == &self.frame_stack_map() {
                Ok(())
            } else {
                Err(SchedulerTaskFrameError::RestorationRejected)
            }
        }
    }

    impl SchedulerTask for AllocationOwnershipTask {
        fn poll(&mut self, context: &SchedulerTaskContext) -> SchedulerTaskPoll {
            let reference = pop_rt_allocate_object(0);
            assert_ne!(reference, 0, "managed scheduler allocation");
            assert_eq!(
                allocation_scheduler(ManagedReference::new(reference)),
                Some(context.scheduler())
            );
            SchedulerTaskPoll::Complete
        }
    }

    #[test]
    fn managed_native_allocations_use_the_dispatching_logical_scheduler() {
        let configuration =
            SchedulerConfiguration::new(2, 2, 4, 4, 1, 1).expect("scheduler configuration");
        let scheduler = NativeScheduler::new(configuration).expect("native scheduler");
        let mut tasks = Vec::new();
        for scheduler_raw in 1..=2 {
            tasks.push(
                scheduler
                    .schedule_on(
                        SchedulerId::new(scheduler_raw),
                        SchedulerTaskMobility::Affine,
                        AllocationOwnershipTask,
                    )
                    .expect("schedule allocation owner"),
            );
        }
        scheduler
            .wait_until_idle(Duration::from_secs(1))
            .expect("allocation tasks complete");
        for task in tasks {
            assert_eq!(
                scheduler.task_state(task),
                Ok(SchedulerTaskState::Completed)
            );
        }
        scheduler.shutdown().expect("scheduler shutdown");
    }
}
