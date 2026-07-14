//! Scheduler identities, configuration, task transitions, and telemetry.

use pop_runtime_collector::SchedulerId;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchedulerTaskId(u64);

impl SchedulerTaskId {
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchedulerWorkerId(u32);

impl SchedulerWorkerId {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchedulerBlockingOperationId(u64);

impl SchedulerBlockingOperationId {
    pub(super) const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchedulerTimerId(u64);

impl SchedulerTimerId {
    pub(super) const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SchedulerExternalEventId(u64);

impl SchedulerExternalEventId {
    pub(super) const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerConfiguration {
    pub(super) scheduler_count: usize,
    pub(super) worker_count: usize,
    pub(super) task_capacity: usize,
    pub(super) local_queue_capacity: usize,
    pub(super) injection_queue_capacity: usize,
    pub(super) injection_poll_interval: usize,
    pub(super) blocking_worker_count: usize,
    pub(super) blocking_queue_capacity: usize,
    pub(super) external_event_capacity: usize,
    pub(super) timer_capacity: usize,
    pub(super) event_delivery_capacity: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerConfigurationError {
    ZeroSchedulers,
    ZeroWorkers,
    WorkerSchedulerCountMismatch,
    ZeroTaskCapacity,
    ZeroLocalQueueCapacity,
    LocalQueueBelowTaskCapacity,
    ZeroInjectionQueueCapacity,
    ZeroInjectionPollInterval,
    IdentityRange,
    ZeroBlockingWorkers,
    ZeroBlockingQueueCapacity,
    ZeroExternalEventCapacity,
    ZeroTimerCapacity,
    ZeroEventDeliveryCapacity,
}

impl SchedulerConfiguration {
    /// Defines explicit bounds for the synchronized correctness scheduler.
    ///
    /// # Errors
    ///
    /// Rejects zero capacities, unequal logical-scheduler/worker counts in the
    /// initial binding implementation, and counts outside typed identity range.
    pub const fn new(
        scheduler_count: usize,
        worker_count: usize,
        task_capacity: usize,
        local_queue_capacity: usize,
        injection_queue_capacity: usize,
        injection_poll_interval: usize,
    ) -> Result<Self, SchedulerConfigurationError> {
        if scheduler_count == 0 {
            Err(SchedulerConfigurationError::ZeroSchedulers)
        } else if worker_count == 0 {
            Err(SchedulerConfigurationError::ZeroWorkers)
        } else if scheduler_count != worker_count {
            Err(SchedulerConfigurationError::WorkerSchedulerCountMismatch)
        } else if task_capacity == 0 {
            Err(SchedulerConfigurationError::ZeroTaskCapacity)
        } else if local_queue_capacity == 0 {
            Err(SchedulerConfigurationError::ZeroLocalQueueCapacity)
        } else if local_queue_capacity < task_capacity {
            Err(SchedulerConfigurationError::LocalQueueBelowTaskCapacity)
        } else if injection_queue_capacity == 0 {
            Err(SchedulerConfigurationError::ZeroInjectionQueueCapacity)
        } else if injection_poll_interval == 0 {
            Err(SchedulerConfigurationError::ZeroInjectionPollInterval)
        } else if scheduler_count > u32::MAX as usize || worker_count > u32::MAX as usize {
            Err(SchedulerConfigurationError::IdentityRange)
        } else {
            Ok(Self {
                scheduler_count,
                worker_count,
                task_capacity,
                local_queue_capacity,
                injection_queue_capacity,
                injection_poll_interval,
                blocking_worker_count: 1,
                blocking_queue_capacity: task_capacity,
                external_event_capacity: task_capacity,
                timer_capacity: task_capacity,
                event_delivery_capacity: task_capacity,
            })
        }
    }

    /// Replaces bounded event registrations, timers, and pending deliveries.
    ///
    /// # Errors
    ///
    /// Rejects every zero capacity.
    pub const fn with_event_driver(
        mut self,
        external_event_capacity: usize,
        timer_capacity: usize,
        delivery_capacity: usize,
    ) -> Result<Self, SchedulerConfigurationError> {
        if external_event_capacity == 0 {
            Err(SchedulerConfigurationError::ZeroExternalEventCapacity)
        } else if timer_capacity == 0 {
            Err(SchedulerConfigurationError::ZeroTimerCapacity)
        } else if delivery_capacity == 0 {
            Err(SchedulerConfigurationError::ZeroEventDeliveryCapacity)
        } else {
            self.external_event_capacity = external_event_capacity;
            self.timer_capacity = timer_capacity;
            self.event_delivery_capacity = delivery_capacity;
            Ok(self)
        }
    }

    /// Replaces the explicit bounded blocking-worker and queue limits.
    ///
    /// # Errors
    ///
    /// Rejects zero workers or queue capacity and typed identity overflow.
    pub const fn with_blocking_pool(
        mut self,
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<Self, SchedulerConfigurationError> {
        if worker_count == 0 {
            Err(SchedulerConfigurationError::ZeroBlockingWorkers)
        } else if queue_capacity == 0 {
            Err(SchedulerConfigurationError::ZeroBlockingQueueCapacity)
        } else if worker_count > u32::MAX as usize {
            Err(SchedulerConfigurationError::IdentityRange)
        } else {
            self.blocking_worker_count = worker_count;
            self.blocking_queue_capacity = queue_capacity;
            Ok(self)
        }
    }

    #[must_use]
    pub const fn scheduler_count(self) -> usize {
        self.scheduler_count
    }

    #[must_use]
    pub const fn worker_count(self) -> usize {
        self.worker_count
    }

    #[must_use]
    pub const fn task_capacity(self) -> usize {
        self.task_capacity
    }

    #[must_use]
    pub const fn local_queue_capacity(self) -> usize {
        self.local_queue_capacity
    }

    #[must_use]
    pub const fn injection_queue_capacity(self) -> usize {
        self.injection_queue_capacity
    }

    #[must_use]
    pub const fn injection_poll_interval(self) -> usize {
        self.injection_poll_interval
    }

    #[must_use]
    pub const fn blocking_worker_count(self) -> usize {
        self.blocking_worker_count
    }

    #[must_use]
    pub const fn blocking_queue_capacity(self) -> usize {
        self.blocking_queue_capacity
    }

    #[must_use]
    pub const fn external_event_capacity(self) -> usize {
        self.external_event_capacity
    }

    #[must_use]
    pub const fn timer_capacity(self) -> usize {
        self.timer_capacity
    }

    #[must_use]
    pub const fn event_delivery_capacity(self) -> usize {
        self.event_delivery_capacity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerTaskMobility {
    Movable,
    Affine,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerTaskPoll {
    Ready,
    Pending,
    Complete,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerTaskState {
    Ready,
    Running,
    Suspended,
    Completed,
    Cancelled,
    Panicked,
}

impl SchedulerTaskState {
    #[must_use]
    pub const fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Panicked)
    }
}

pub struct SchedulerTaskContext {
    task: SchedulerTaskId,
    scheduler: SchedulerId,
    worker: SchedulerWorkerId,
    cancellation_requested: bool,
}

impl SchedulerTaskContext {
    pub(super) const fn new(
        task: SchedulerTaskId,
        scheduler: SchedulerId,
        worker: SchedulerWorkerId,
        cancellation_requested: bool,
    ) -> Self {
        Self {
            task,
            scheduler,
            worker,
            cancellation_requested,
        }
    }

    #[must_use]
    pub const fn task(&self) -> SchedulerTaskId {
        self.task
    }

    #[must_use]
    pub const fn scheduler(&self) -> SchedulerId {
        self.scheduler
    }

    #[must_use]
    pub const fn worker(&self) -> SchedulerWorkerId {
        self.worker
    }

    #[must_use]
    pub const fn cancellation_requested(&self) -> bool {
        self.cancellation_requested
    }
}

pub trait SchedulerTask: Send + 'static {
    fn poll(&mut self, context: &SchedulerTaskContext) -> SchedulerTaskPoll;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerError {
    TaskCapacity,
    LocalQueueCapacity,
    InjectionQueueCapacity,
    BlockingQueueCapacity,
    ExternalEventCapacity,
    TimerCapacity,
    EventDeliveryCapacity,
    UnknownExternalEvent(SchedulerExternalEventId),
    UnknownTimer(SchedulerTimerId),
    UnknownScheduler(SchedulerId),
    UnknownTask(SchedulerTaskId),
    TaskNotTerminal(SchedulerTaskId),
    Closed,
    ThreadStart,
    ThreadJoin,
    IdentityOverflow,
    WaitTimedOut,
    ReplayExhausted,
    ReplayEnabledSetMismatch,
    PollBudgetExhausted,
    ExplorationBudget,
    RuntimeTransition {
        transition: SchedulerRuntimeTransition,
        failure: SchedulerRuntimeTransitionFailure,
    },
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SchedulerTelemetry {
    pub(super) retained_tasks: usize,
    pub(super) ready_tasks: usize,
    pub(super) running_tasks: usize,
    pub(super) suspended_tasks: usize,
    pub(super) terminal_tasks: usize,
    pub(super) local_queue_depth: usize,
    pub(super) maximum_local_queue_depth: usize,
    pub(super) injection_queue_depth: usize,
    pub(super) maximum_injection_queue_depth: usize,
    pub(super) blocking_queue_depth: usize,
    pub(super) maximum_blocking_queue_depth: usize,
    pub(super) active_blocking_operations: usize,
    pub(super) maximum_active_blocking_operations: usize,
    pub(super) tasks_scheduled: u64,
    pub(super) polls: u64,
    pub(super) suspensions: u64,
    pub(super) completions: u64,
    pub(super) cancellations_requested: u64,
    pub(super) cancellations_observed: u64,
    pub(super) panics: u64,
    pub(super) wake_requests: u64,
    pub(super) coalesced_wakes: u64,
    pub(super) tasks_stolen: u64,
    pub(super) affine_tasks_stolen: u64,
    pub(super) steal_searches: u64,
    pub(super) steal_victims_examined: u64,
    pub(super) steal_successes: u64,
    pub(super) steal_failures: u64,
    pub(super) maximum_stolen_batch: usize,
    pub(super) gc_delayed_migrations: u64,
    pub(super) blocking_submissions: u64,
    pub(super) blocking_queue_rejections: u64,
    pub(super) blocking_completions: u64,
    pub(super) blocking_panics: u64,
    pub(super) timers_scheduled: u64,
    pub(super) timers_delivered: u64,
    pub(super) external_events_registered: u64,
    pub(super) external_events_delivered: u64,
    pub(super) external_event_signals_coalesced: u64,
    pub(super) stale_ready_entries: u64,
    pub(super) worker_starts: u64,
    pub(super) worker_parks: u64,
    pub(super) worker_unparks: u64,
    pub(super) worker_stops: u64,
    pub(super) worker_threads_used: usize,
}

macro_rules! telemetry_accessors {
    ($($name:ident: $type:ty),* $(,)?) => {
        $(
            #[must_use]
            pub const fn $name(self) -> $type {
                self.$name
            }
        )*
    };
}

impl SchedulerTelemetry {
    telemetry_accessors! {
        retained_tasks: usize,
        ready_tasks: usize,
        running_tasks: usize,
        suspended_tasks: usize,
        terminal_tasks: usize,
        local_queue_depth: usize,
        maximum_local_queue_depth: usize,
        injection_queue_depth: usize,
        maximum_injection_queue_depth: usize,
        blocking_queue_depth: usize,
        maximum_blocking_queue_depth: usize,
        active_blocking_operations: usize,
        maximum_active_blocking_operations: usize,
        tasks_scheduled: u64,
        polls: u64,
        suspensions: u64,
        completions: u64,
        cancellations_requested: u64,
        cancellations_observed: u64,
        panics: u64,
        wake_requests: u64,
        coalesced_wakes: u64,
        tasks_stolen: u64,
        affine_tasks_stolen: u64,
        steal_searches: u64,
        steal_victims_examined: u64,
        steal_successes: u64,
        steal_failures: u64,
        maximum_stolen_batch: usize,
        gc_delayed_migrations: u64,
        blocking_submissions: u64,
        blocking_queue_rejections: u64,
        blocking_completions: u64,
        blocking_panics: u64,
        timers_scheduled: u64,
        timers_delivered: u64,
        external_events_registered: u64,
        external_events_delivered: u64,
        external_event_signals_coalesced: u64,
        stale_ready_entries: u64,
        worker_starts: u64,
        worker_parks: u64,
        worker_unparks: u64,
        worker_stops: u64,
    }

    #[must_use]
    pub const fn worker_threads_used(self) -> usize {
        self.worker_threads_used
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerRuntimeTransition {
    WorkerStarted {
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
    },
    WorkerParked {
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
    },
    WorkerUnparked {
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
    },
    WorkerStopped {
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
    },
    TaskDispatched {
        task: SchedulerTaskId,
        worker: SchedulerWorkerId,
        scheduler: SchedulerId,
    },
    TaskSuspended {
        task: SchedulerTaskId,
        scheduler: SchedulerId,
    },
    TaskResumed {
        task: SchedulerTaskId,
        scheduler: SchedulerId,
    },
    TaskTerminal {
        task: SchedulerTaskId,
        scheduler: SchedulerId,
        state: SchedulerTaskState,
    },
    TaskMigration {
        task: SchedulerTaskId,
        from: SchedulerId,
        to: SchedulerId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerRuntimeTransitionControl {
    Continue,
    RefuseMigration,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerRuntimeTransitionFailure {
    CollectorState,
}

pub trait SchedulerRuntimeTransitions: Send + Sync + 'static {
    /// Applies one typed scheduler/collector boundary transition.
    ///
    /// # Errors
    ///
    /// Returns a closed typed collector-state failure. Migration may instead
    /// be delayed without failing the worker.
    fn apply(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure>;
}

pub(super) struct DetachedSchedulerRuntimeTransitions;

impl SchedulerRuntimeTransitions for DetachedSchedulerRuntimeTransitions {
    fn apply(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure> {
        if matches!(transition, SchedulerRuntimeTransition::TaskMigration { .. }) {
            Ok(SchedulerRuntimeTransitionControl::RefuseMigration)
        } else {
            Ok(SchedulerRuntimeTransitionControl::Continue)
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SchedulerDecision {
    decision: u64,
    enabled: Vec<SchedulerTaskId>,
    selected: SchedulerTaskId,
    scheduler: SchedulerId,
}

impl SchedulerDecision {
    pub(super) fn new(
        decision: u64,
        enabled: Vec<SchedulerTaskId>,
        selected: SchedulerTaskId,
        scheduler: SchedulerId,
    ) -> Self {
        Self {
            decision,
            enabled,
            selected,
            scheduler,
        }
    }

    #[must_use]
    pub const fn decision(&self) -> u64 {
        self.decision
    }

    #[must_use]
    pub fn enabled(&self) -> &[SchedulerTaskId] {
        &self.enabled
    }

    #[must_use]
    pub const fn selected(&self) -> SchedulerTaskId {
        self.selected
    }

    #[must_use]
    pub const fn scheduler(&self) -> SchedulerId {
        self.scheduler
    }
}
