//! Scheduler identities, configuration, task transitions, and telemetry.

use std::cell::Cell;

use pop_runtime_collector::SchedulerId;
use pop_runtime_interface::{RootPublication, StackMap};

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
    pub(super) dispatch_work_budget: usize,
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
    ZeroDispatchWorkBudget,
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
                dispatch_work_budget: 128,
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

    /// Replaces the deterministic work units available to each dispatch.
    ///
    /// # Errors
    ///
    /// Rejects a zero budget because it could make every task permanently
    /// ineligible to perform useful work.
    pub const fn with_dispatch_work_budget(
        mut self,
        work_units: usize,
    ) -> Result<Self, SchedulerConfigurationError> {
        if work_units == 0 {
            Err(SchedulerConfigurationError::ZeroDispatchWorkBudget)
        } else {
            self.dispatch_work_budget = work_units;
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

    #[must_use]
    pub const fn dispatch_work_budget(self) -> usize {
        self.dispatch_work_budget
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
    remaining_work: Cell<usize>,
    work_budget_exhausted: Cell<bool>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerWorkBudgetStatus {
    Available,
    Exhausted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerWorkBudgetError {
    ZeroWorkUnits,
}

impl SchedulerTaskContext {
    pub(super) const fn new(
        task: SchedulerTaskId,
        scheduler: SchedulerId,
        worker: SchedulerWorkerId,
        cancellation_requested: bool,
        work_budget: usize,
    ) -> Self {
        Self {
            task,
            scheduler,
            worker,
            cancellation_requested,
            remaining_work: Cell::new(work_budget),
            work_budget_exhausted: Cell::new(false),
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

    /// Charges deterministic work units to the current dispatch.
    ///
    /// # Errors
    ///
    /// Rejects a zero-unit charge, which cannot represent compiler or runtime
    /// progress.
    pub fn consume_work(
        &self,
        work_units: usize,
    ) -> Result<SchedulerWorkBudgetStatus, SchedulerWorkBudgetError> {
        if work_units == 0 {
            return Err(SchedulerWorkBudgetError::ZeroWorkUnits);
        }
        let remaining = self.remaining_work.get();
        if work_units >= remaining {
            self.remaining_work.set(0);
            self.work_budget_exhausted.set(true);
            Ok(SchedulerWorkBudgetStatus::Exhausted)
        } else {
            self.remaining_work.set(remaining - work_units);
            Ok(SchedulerWorkBudgetStatus::Available)
        }
    }

    #[must_use]
    pub fn remaining_work(&self) -> usize {
        self.remaining_work.get()
    }

    pub(super) fn work_budget_exhausted(&self) -> bool {
        self.work_budget_exhausted.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerTaskFrameError {
    PublicationRejected,
    RestorationRejected,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerTaskFrameFailure {
    Publication,
    PublicationShape,
    Restoration,
    Collector,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerCollectorBindingFailure {
    Registration,
    ManagedTransition,
    DetachedTransition,
    Unregistration,
}

/// Exact compiler/runtime adapter for one scheduler-owned task frame.
///
/// Implementations are mandatory even for trusted root-free host tasks; there
/// is deliberately no implicit empty-frame fallback.
pub trait SchedulerTaskFrame: Send + 'static {
    fn frame_stack_map(&self) -> StackMap;

    /// # Errors
    ///
    /// Returns a closed rejection when the task cannot publish its exact live
    /// frame values.
    fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError>;

    /// # Errors
    ///
    /// Returns a closed rejection when the task cannot install the exact
    /// possibly relocated values before polling.
    fn restore_frame_roots(
        &mut self,
        publication: RootPublication,
    ) -> Result<(), SchedulerTaskFrameError>;
}

pub trait SchedulerTask: SchedulerTaskFrame {
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
    TaskFrame {
        task: SchedulerTaskId,
        failure: SchedulerTaskFrameFailure,
    },
    CollectorBinding {
        worker: SchedulerWorkerId,
        failure: SchedulerCollectorBindingFailure,
    },
}

const SCHEDULER_DELAY_BUCKETS: usize = 65;

/// Bounded logarithmic distribution of scheduler delay in semantic work units.
///
/// Percentiles are conservative bucket upper bounds capped by the exact
/// observed maximum. The fixed histogram prevents telemetry from retaining an
/// unbounded sample stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerDelayTelemetry {
    buckets: [u64; SCHEDULER_DELAY_BUCKETS],
    samples: u64,
    maximum_work_units: u64,
}

impl Default for SchedulerDelayTelemetry {
    fn default() -> Self {
        Self {
            buckets: [0; SCHEDULER_DELAY_BUCKETS],
            samples: 0,
            maximum_work_units: 0,
        }
    }
}

impl SchedulerDelayTelemetry {
    pub(super) fn record(&mut self, work_units: u64) {
        let bucket = if work_units == 0 {
            0
        } else {
            usize::try_from(u64::BITS - work_units.leading_zeros())
                .expect("u64 bit count fits usize")
        };
        self.buckets[bucket] = self.buckets[bucket].saturating_add(1);
        self.samples = self.samples.saturating_add(1);
        self.maximum_work_units = self.maximum_work_units.max(work_units);
    }

    #[must_use]
    pub const fn samples(self) -> u64 {
        self.samples
    }

    #[must_use]
    pub fn p50_work_units(self) -> u64 {
        self.percentile(50, 100)
    }

    #[must_use]
    pub fn p95_work_units(self) -> u64 {
        self.percentile(95, 100)
    }

    #[must_use]
    pub fn p99_work_units(self) -> u64 {
        self.percentile(99, 100)
    }

    #[must_use]
    pub fn p999_work_units(self) -> u64 {
        self.percentile(999, 1_000)
    }

    #[must_use]
    pub const fn maximum_work_units(self) -> u64 {
        self.maximum_work_units
    }

    fn percentile(self, numerator: u64, denominator: u64) -> u64 {
        if self.samples == 0 {
            return 0;
        }
        let whole = (self.samples / denominator).saturating_mul(numerator);
        let remainder = self.samples % denominator;
        let partial = remainder
            .saturating_mul(numerator)
            .saturating_add(denominator - 1)
            / denominator;
        let rank = whole.saturating_add(partial);
        let mut cumulative = 0_u64;
        for (bucket, samples) in self.buckets.into_iter().enumerate() {
            cumulative = cumulative.saturating_add(samples);
            if cumulative >= rank {
                return delay_bucket_upper_bound(bucket).min(self.maximum_work_units);
            }
        }
        self.maximum_work_units
    }
}

const fn delay_bucket_upper_bound(bucket: usize) -> u64 {
    match bucket {
        0 => 0,
        64 => u64::MAX,
        _ => (1_u64 << bucket) - 1,
    }
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
    pub(super) scheduler_migrations: u64,
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
    pub(super) retained_frame_root_containers: usize,
    pub(super) maximum_retained_frame_root_containers: usize,
    pub(super) frame_root_retentions: u64,
    pub(super) frame_root_restorations: u64,
    pub(super) frame_root_releases: u64,
    pub(super) frame_root_failures: u64,
    pub(super) mutator_registrations: u64,
    pub(super) managed_mutator_transitions: u64,
    pub(super) detached_mutator_transitions: u64,
    pub(super) mutator_unregistrations: u64,
    pub(super) work_budget_exhaustions: u64,
    pub(super) ready_to_run_delay: SchedulerDelayTelemetry,
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
        scheduler_migrations: u64,
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
        retained_frame_root_containers: usize,
        maximum_retained_frame_root_containers: usize,
        frame_root_retentions: u64,
        frame_root_restorations: u64,
        frame_root_releases: u64,
        frame_root_failures: u64,
        mutator_registrations: u64,
        managed_mutator_transitions: u64,
        detached_mutator_transitions: u64,
        mutator_unregistrations: u64,
        work_budget_exhaustions: u64,
        ready_to_run_delay: SchedulerDelayTelemetry,
    }

    #[must_use]
    pub const fn worker_threads_used(self) -> usize {
        self.worker_threads_used
    }
}

#[cfg(test)]
mod tests {
    use super::SchedulerDelayTelemetry;

    #[test]
    fn delay_histogram_is_bounded_and_reports_conservative_percentiles() {
        let mut delay = SchedulerDelayTelemetry::default();
        for work_units in [0, 1, 2, 3, 8, u64::MAX] {
            delay.record(work_units);
        }

        assert_eq!(delay.samples(), 6);
        assert_eq!(delay.p50_work_units(), 3);
        assert_eq!(delay.p95_work_units(), u64::MAX);
        assert_eq!(delay.p99_work_units(), u64::MAX);
        assert_eq!(delay.p999_work_units(), u64::MAX);
        assert_eq!(delay.maximum_work_units(), u64::MAX);
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
