use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use pop_runtime_collector::SchedulerId;
use pop_runtime_interface::{RootPublication, SafePointId, StackMap};
use pop_runtime_native::{
    NativeScheduler, SchedulerConfiguration, SchedulerConfigurationError, SchedulerError,
    SchedulerRuntimeTransition, SchedulerRuntimeTransitionControl,
    SchedulerRuntimeTransitionFailure, SchedulerRuntimeTransitions, SchedulerTask,
    SchedulerTaskContext, SchedulerTaskFrame, SchedulerTaskFrameError, SchedulerTaskId,
    SchedulerTaskMobility, SchedulerTaskPoll, SchedulerTaskState, SchedulerTelemetry,
};

const WAIT_TIMEOUT: Duration = Duration::from_mins(5);
pub const SCHEDULER_BENCHMARK_SCHEMA: &str = "pop-scheduler-benchmark-v2";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerBenchmarkConfiguration {
    pub workers: usize,
    pub tasks: usize,
    pub polls_per_task: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerBenchmarkError {
    InvalidConfiguration,
    CounterOverflow,
    InvariantViolation,
    Configuration(SchedulerConfigurationError),
    Scheduler(SchedulerError),
}

impl From<SchedulerConfigurationError> for SchedulerBenchmarkError {
    fn from(error: SchedulerConfigurationError) -> Self {
        Self::Configuration(error)
    }
}

impl From<SchedulerError> for SchedulerBenchmarkError {
    fn from(error: SchedulerError) -> Self {
        Self::Scheduler(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerWorkload {
    TaskControl,
    ReadyPolls,
    BurstInjection,
    HotQueueSteal,
    SuspendedFrames,
    TimerFanOut,
    ExternalEventFanOut,
    BlockingSaturation,
}

impl SchedulerWorkload {
    pub const ALL: [Self; 8] = [
        Self::TaskControl,
        Self::ReadyPolls,
        Self::BurstInjection,
        Self::HotQueueSteal,
        Self::SuspendedFrames,
        Self::TimerFanOut,
        Self::ExternalEventFanOut,
        Self::BlockingSaturation,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::TaskControl => "task_control",
            Self::ReadyPolls => "ready_polls",
            Self::BurstInjection => "burst_injection",
            Self::HotQueueSteal => "hot_queue_steal",
            Self::SuspendedFrames => "suspended_frames",
            Self::TimerFanOut => "timer_fan_out",
            Self::ExternalEventFanOut => "external_event_fan_out",
            Self::BlockingSaturation => "blocking_saturation",
        }
    }

    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|workload| workload.name() == name)
    }

    const fn suspends(self) -> bool {
        matches!(
            self,
            Self::SuspendedFrames
                | Self::TimerFanOut
                | Self::ExternalEventFanOut
                | Self::BlockingSaturation
        )
    }

    const fn task_polls(self, configuration: SchedulerBenchmarkConfiguration) -> usize {
        if self.suspends() {
            2
        } else if matches!(self, Self::TaskControl) {
            1
        } else {
            configuration.polls_per_task
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerWorkloadCounters {
    pub workload: &'static str,
    pub workers: usize,
    pub tasks: u64,
    pub operations: u64,
    pub checksum: u64,
    pub polls: u64,
    pub completions: u64,
    pub suspensions: u64,
    pub wake_requests: u64,
    pub tasks_stolen: u64,
    pub blocking_submissions: u64,
    pub timers_delivered: u64,
    pub external_events_delivered: u64,
    pub local_queue_depth: usize,
    pub maximum_local_queue_depth: usize,
    pub injection_queue_depth: usize,
    pub maximum_injection_queue_depth: usize,
    pub blocking_queue_depth: usize,
    pub maximum_blocking_queue_depth: usize,
    pub active_blocking_operations: usize,
    pub maximum_active_blocking_operations: usize,
    pub steal_searches: u64,
    pub steal_victims_examined: u64,
    pub steal_successes: u64,
    pub steal_failures: u64,
    pub maximum_stolen_batch: usize,
    pub worker_starts: u64,
    pub worker_parks: u64,
    pub worker_unparks: u64,
    pub worker_stops: u64,
    pub stale_ready_entries: u64,
    pub first_poll_latency_p50_nanoseconds: u64,
    pub first_poll_latency_p95_nanoseconds: u64,
    pub first_poll_latency_p99_nanoseconds: u64,
    pub first_poll_latency_p999_nanoseconds: u64,
    pub first_poll_latency_max_nanoseconds: u64,
}

struct Latencies(Mutex<Vec<u64>>);

impl Latencies {
    fn record(&self, ready_at: Instant) {
        let elapsed = u64::try_from(ready_at.elapsed().as_nanos()).unwrap_or(u64::MAX);
        self.0
            .lock()
            .expect("benchmark latency samples")
            .push(elapsed);
    }

    fn percentiles(&self) -> (u64, u64, u64, u64, u64) {
        let mut samples = self.0.lock().expect("benchmark latency samples").clone();
        samples.sort_unstable();
        (
            percentile(&samples, 500),
            percentile(&samples, 950),
            percentile(&samples, 990),
            percentile(&samples, 999),
            samples.last().copied().unwrap_or(0),
        )
    }
}

fn percentile(samples: &[u64], per_thousand: usize) -> u64 {
    if samples.is_empty() {
        return 0;
    }
    let index = (samples.len() - 1).saturating_mul(per_thousand) / 1_000;
    samples[index]
}

struct BenchmarkTask {
    token: u64,
    remaining_polls: usize,
    suspend_once: bool,
    checksum: Arc<AtomicU64>,
    latencies: Arc<Latencies>,
    ready_at: Instant,
    first_poll: bool,
}

impl SchedulerTaskFrame for BenchmarkTask {
    fn frame_stack_map(&self) -> StackMap {
        StackMap::new(SafePointId::new(1), Vec::new()).expect("benchmark empty frame map")
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

impl SchedulerTask for BenchmarkTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        if self.first_poll {
            self.first_poll = false;
            self.latencies.record(self.ready_at);
        }
        self.checksum.fetch_add(self.token, Ordering::Relaxed);
        if self.suspend_once {
            self.suspend_once = false;
            SchedulerTaskPoll::Pending
        } else if self.remaining_polls == 1 {
            self.remaining_polls = 0;
            SchedulerTaskPoll::Complete
        } else {
            self.remaining_polls -= 1;
            SchedulerTaskPoll::Ready
        }
    }
}

struct PermitRuntimeTransitions;

impl SchedulerRuntimeTransitions for PermitRuntimeTransitions {
    fn apply(
        &self,
        _transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure> {
        Ok(SchedulerRuntimeTransitionControl::Continue)
    }
}

struct WorkloadState {
    scheduler: NativeScheduler,
    workers: usize,
    checksum: Arc<AtomicU64>,
    latencies: Arc<Latencies>,
}

impl WorkloadState {
    fn new(
        configuration: SchedulerBenchmarkConfiguration,
        migration: bool,
    ) -> Result<Self, SchedulerBenchmarkError> {
        validate(configuration)?;
        let scheduler_configuration = SchedulerConfiguration::new(
            configuration.workers,
            configuration.workers,
            configuration.tasks,
            configuration.tasks,
            configuration.tasks,
            1,
        )?
        .with_blocking_pool(configuration.workers, configuration.tasks)?
        .with_event_driver(
            configuration.tasks,
            configuration.tasks,
            configuration.tasks,
        )?;
        let scheduler = if migration {
            NativeScheduler::new_with_runtime_transitions(
                scheduler_configuration,
                Arc::new(PermitRuntimeTransitions),
            )?
        } else {
            NativeScheduler::new(scheduler_configuration)?
        };
        Ok(Self {
            scheduler,
            workers: configuration.workers,
            checksum: Arc::new(AtomicU64::new(0)),
            latencies: Arc::new(Latencies(Mutex::new(Vec::with_capacity(
                configuration.tasks,
            )))),
        })
    }

    fn task(&self, token: u64, polls: usize, suspend_once: bool) -> BenchmarkTask {
        BenchmarkTask {
            token,
            remaining_polls: polls,
            suspend_once,
            checksum: Arc::clone(&self.checksum),
            latencies: Arc::clone(&self.latencies),
            ready_at: Instant::now(),
            first_poll: true,
        }
    }
}

fn validate(configuration: SchedulerBenchmarkConfiguration) -> Result<(), SchedulerBenchmarkError> {
    if configuration.workers == 0
        || configuration.tasks == 0
        || configuration.polls_per_task == 0
        || configuration.workers > configuration.tasks
    {
        Err(SchedulerBenchmarkError::InvalidConfiguration)
    } else {
        Ok(())
    }
}

/// Runs one bounded scheduler benchmark workload and verifies its logical work.
///
/// # Errors
///
/// Rejects invalid bounds, counter overflow, scheduler failures, and any lost
/// or duplicated logical work detected by the checksum and telemetry contract.
pub fn run_scheduler_workload(
    workload: SchedulerWorkload,
    configuration: SchedulerBenchmarkConfiguration,
) -> Result<SchedulerWorkloadCounters, SchedulerBenchmarkError> {
    let task_polls = workload.task_polls(configuration);
    let state = WorkloadState::new(
        configuration,
        matches!(workload, SchedulerWorkload::HotQueueSteal),
    )?;
    let task_ids = match workload {
        SchedulerWorkload::TaskControl => run_task_control(&state, configuration.tasks)?,
        SchedulerWorkload::ReadyPolls => {
            run_balanced_ready(&state, configuration.tasks, task_polls)?
        }
        SchedulerWorkload::BurstInjection => {
            run_burst_injection(&state, configuration.tasks, task_polls)?
        }
        SchedulerWorkload::HotQueueSteal => run_hot_queue(&state, configuration.tasks, task_polls)?,
        SchedulerWorkload::SuspendedFrames => {
            run_suspended(&state, configuration.tasks, ResumeSource::Wake)?
        }
        SchedulerWorkload::TimerFanOut => {
            run_suspended(&state, configuration.tasks, ResumeSource::Timer)?
        }
        SchedulerWorkload::ExternalEventFanOut => {
            run_suspended(&state, configuration.tasks, ResumeSource::ExternalEvent)?
        }
        SchedulerWorkload::BlockingSaturation => {
            run_suspended(&state, configuration.tasks, ResumeSource::Blocking)?
        }
    };
    state.scheduler.wait_until_idle(WAIT_TIMEOUT)?;
    let telemetry = state.scheduler.telemetry();
    let checksum = state.checksum.load(Ordering::Relaxed);
    let operations = expected_operations(configuration.tasks, task_polls)?;
    let expected_checksum = expected_checksum(configuration.tasks, task_polls)?;
    if telemetry.polls() != operations
        || telemetry.completions() != u64::try_from(configuration.tasks).unwrap_or(u64::MAX)
        || checksum != expected_checksum
        || telemetry.stale_ready_entries() != 0
    {
        return Err(SchedulerBenchmarkError::InvariantViolation);
    }
    for task in task_ids {
        state.scheduler.release_terminal_task(task)?;
    }
    let latencies = state.latencies.percentiles();
    let final_telemetry = state.scheduler.shutdown_with_telemetry()?;
    Ok(counters(
        workload,
        configuration,
        &final_telemetry,
        checksum,
        latencies,
    ))
}

fn run_task_control(
    state: &WorkloadState,
    tasks: usize,
) -> Result<Vec<SchedulerTaskId>, SchedulerBenchmarkError> {
    let mut task_ids = Vec::with_capacity(tasks);
    for index in 0..tasks {
        let task = state
            .scheduler
            .schedule(state.task(token(index)?, 1, false))?;
        state.scheduler.wait_until_idle(WAIT_TIMEOUT)?;
        task_ids.push(task);
    }
    Ok(task_ids)
}

fn run_balanced_ready(
    state: &WorkloadState,
    tasks: usize,
    polls: usize,
) -> Result<Vec<SchedulerTaskId>, SchedulerBenchmarkError> {
    let mut task_ids = Vec::with_capacity(tasks);
    for index in 0..tasks {
        let scheduler = SchedulerId::new(
            u32::try_from(index % state.workers + 1)
                .map_err(|_| SchedulerBenchmarkError::CounterOverflow)?,
        );
        task_ids.push(state.scheduler.schedule_on(
            scheduler,
            SchedulerTaskMobility::Affine,
            state.task(token(index)?, polls, false),
        )?);
    }
    Ok(task_ids)
}

fn run_burst_injection(
    state: &WorkloadState,
    tasks: usize,
    polls: usize,
) -> Result<Vec<SchedulerTaskId>, SchedulerBenchmarkError> {
    (0..tasks)
        .map(|index| {
            state
                .scheduler
                .schedule(state.task(token(index)?, polls, false))
                .map_err(Into::into)
        })
        .collect()
}

fn run_hot_queue(
    state: &WorkloadState,
    tasks: usize,
    polls: usize,
) -> Result<Vec<SchedulerTaskId>, SchedulerBenchmarkError> {
    let batch: Result<Vec<_>, SchedulerBenchmarkError> = (0..tasks)
        .map(|index| Ok(state.task(token(index)?, polls, false)))
        .collect();
    Ok(state.scheduler.schedule_batch_on(
        SchedulerId::new(1),
        SchedulerTaskMobility::Movable,
        batch?,
    )?)
}

#[derive(Clone, Copy)]
enum ResumeSource {
    Wake,
    Timer,
    ExternalEvent,
    Blocking,
}

fn run_suspended(
    state: &WorkloadState,
    tasks: usize,
    source: ResumeSource,
) -> Result<Vec<SchedulerTaskId>, SchedulerBenchmarkError> {
    let task_ids: Vec<_> = (0..tasks)
        .map(|index| {
            state
                .scheduler
                .schedule(state.task(token(index)?, 1, true))
                .map_err(Into::into)
        })
        .collect::<Result<_, SchedulerBenchmarkError>>()?;
    state.scheduler.wait_until_idle(WAIT_TIMEOUT)?;
    match source {
        ResumeSource::Wake => {
            for task in &task_ids {
                state.scheduler.wake(*task)?;
            }
        }
        ResumeSource::Timer => {
            for task in &task_ids {
                state.scheduler.schedule_wake_after(*task, Duration::ZERO)?;
            }
            wait_until_terminal(&state.scheduler, &task_ids)?;
        }
        ResumeSource::ExternalEvent => {
            let mut events = Vec::with_capacity(task_ids.len());
            for task in &task_ids {
                let event = state.scheduler.register_external_event(*task)?;
                state.scheduler.signal_external_event(event)?;
                events.push(event);
            }
            wait_until_terminal(&state.scheduler, &task_ids)?;
            for event in events {
                state.scheduler.release_external_event(event)?;
            }
        }
        ResumeSource::Blocking => {
            for task in &task_ids {
                state.scheduler.submit_blocking(*task, || {})?;
            }
        }
    }
    Ok(task_ids)
}

fn wait_until_terminal(
    scheduler: &NativeScheduler,
    tasks: &[SchedulerTaskId],
) -> Result<(), SchedulerBenchmarkError> {
    let deadline = Instant::now()
        .checked_add(WAIT_TIMEOUT)
        .ok_or(SchedulerBenchmarkError::InvariantViolation)?;
    loop {
        let mut all_terminal = true;
        for task in tasks {
            all_terminal &= matches!(
                scheduler.task_state(*task)?,
                SchedulerTaskState::Completed
                    | SchedulerTaskState::Cancelled
                    | SchedulerTaskState::Panicked
            );
        }
        if all_terminal {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(SchedulerBenchmarkError::Scheduler(
                SchedulerError::WaitTimedOut,
            ));
        }
        std::thread::yield_now();
    }
}

fn token(index: usize) -> Result<u64, SchedulerBenchmarkError> {
    u64::try_from(index + 1).map_err(|_| SchedulerBenchmarkError::CounterOverflow)
}

fn expected_operations(tasks: usize, polls: usize) -> Result<u64, SchedulerBenchmarkError> {
    u64::try_from(tasks)
        .ok()
        .and_then(|tasks| {
            u64::try_from(polls)
                .ok()
                .and_then(|polls| tasks.checked_mul(polls))
        })
        .ok_or(SchedulerBenchmarkError::CounterOverflow)
}

fn expected_checksum(tasks: usize, polls: usize) -> Result<u64, SchedulerBenchmarkError> {
    let tasks = u64::try_from(tasks).map_err(|_| SchedulerBenchmarkError::CounterOverflow)?;
    let polls = u64::try_from(polls).map_err(|_| SchedulerBenchmarkError::CounterOverflow)?;
    tasks
        .checked_mul(
            tasks
                .checked_add(1)
                .ok_or(SchedulerBenchmarkError::CounterOverflow)?,
        )
        .and_then(|sum| sum.checked_div(2))
        .and_then(|sum| sum.checked_mul(polls))
        .ok_or(SchedulerBenchmarkError::CounterOverflow)
}

fn counters(
    workload: SchedulerWorkload,
    configuration: SchedulerBenchmarkConfiguration,
    telemetry: &SchedulerTelemetry,
    checksum: u64,
    latencies: (u64, u64, u64, u64, u64),
) -> SchedulerWorkloadCounters {
    SchedulerWorkloadCounters {
        workload: workload.name(),
        workers: configuration.workers,
        tasks: u64::try_from(configuration.tasks).unwrap_or(u64::MAX),
        operations: telemetry.polls(),
        checksum,
        polls: telemetry.polls(),
        completions: telemetry.completions(),
        suspensions: telemetry.suspensions(),
        wake_requests: telemetry.wake_requests(),
        tasks_stolen: telemetry.tasks_stolen(),
        blocking_submissions: telemetry.blocking_submissions(),
        timers_delivered: telemetry.timers_delivered(),
        external_events_delivered: telemetry.external_events_delivered(),
        local_queue_depth: telemetry.local_queue_depth(),
        maximum_local_queue_depth: telemetry.maximum_local_queue_depth(),
        injection_queue_depth: telemetry.injection_queue_depth(),
        maximum_injection_queue_depth: telemetry.maximum_injection_queue_depth(),
        blocking_queue_depth: telemetry.blocking_queue_depth(),
        maximum_blocking_queue_depth: telemetry.maximum_blocking_queue_depth(),
        active_blocking_operations: telemetry.active_blocking_operations(),
        maximum_active_blocking_operations: telemetry.maximum_active_blocking_operations(),
        steal_searches: telemetry.steal_searches(),
        steal_victims_examined: telemetry.steal_victims_examined(),
        steal_successes: telemetry.steal_successes(),
        steal_failures: telemetry.steal_failures(),
        maximum_stolen_batch: telemetry.maximum_stolen_batch(),
        worker_starts: telemetry.worker_starts(),
        worker_parks: telemetry.worker_parks(),
        worker_unparks: telemetry.worker_unparks(),
        worker_stops: telemetry.worker_stops(),
        stale_ready_entries: telemetry.stale_ready_entries(),
        first_poll_latency_p50_nanoseconds: latencies.0,
        first_poll_latency_p95_nanoseconds: latencies.1,
        first_poll_latency_p99_nanoseconds: latencies.2,
        first_poll_latency_p999_nanoseconds: latencies.3,
        first_poll_latency_max_nanoseconds: latencies.4,
    }
}
