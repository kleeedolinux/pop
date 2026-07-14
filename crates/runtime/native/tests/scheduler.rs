use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use pop_runtime_collector::SchedulerId;
use pop_runtime_native::{
    DeterministicScheduler, NativeScheduler, SchedulerConfiguration, SchedulerConfigurationError,
    SchedulerError, SchedulerRuntimeTransition, SchedulerRuntimeTransitionControl,
    SchedulerRuntimeTransitionFailure, SchedulerRuntimeTransitions, SchedulerTask,
    SchedulerTaskContext, SchedulerTaskMobility, SchedulerTaskPoll, SchedulerTaskState,
};

#[derive(Default)]
struct RecordingRuntimeTransitions {
    events: Mutex<Vec<SchedulerRuntimeTransition>>,
    refuse_migration: bool,
}

impl SchedulerRuntimeTransitions for RecordingRuntimeTransitions {
    fn apply(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure> {
        self.events
            .lock()
            .expect("runtime transition log")
            .push(transition);
        if self.refuse_migration
            && matches!(transition, SchedulerRuntimeTransition::TaskMigration { .. })
        {
            Ok(SchedulerRuntimeTransitionControl::RefuseMigration)
        } else {
            Ok(SchedulerRuntimeTransitionControl::Continue)
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

type PollRecord = (u64, u32, u32, bool);

struct StepTask {
    polls: Arc<Mutex<Vec<PollRecord>>>,
    step: u8,
}

impl SchedulerTask for StepTask {
    fn poll(&mut self, context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        self.polls.lock().expect("poll log").push((
            context.task().raw(),
            context.scheduler().raw(),
            context.worker().raw(),
            context.cancellation_requested(),
        ));
        self.step += 1;
        match self.step {
            1 => SchedulerTaskPoll::Ready,
            2 => SchedulerTaskPoll::Pending,
            _ => SchedulerTaskPoll::Complete,
        }
    }
}

struct CancellationTask;

impl SchedulerTask for CancellationTask {
    fn poll(&mut self, context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        if context.cancellation_requested() {
            SchedulerTaskPoll::Cancelled
        } else {
            SchedulerTaskPoll::Pending
        }
    }
}

struct PanicTask;

impl SchedulerTask for PanicTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        panic!("task panic must not terminate its scheduler worker")
    }
}

struct CompleteTask {
    completions: Arc<Mutex<usize>>,
}

impl SchedulerTask for CompleteTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        let mut completions = self.completions.lock().expect("completion count");
        *completions += 1;
        SchedulerTaskPoll::Complete
    }
}

struct YieldOnceTask {
    first: bool,
}

struct ReadyManyTask {
    remaining_ready_polls: usize,
}

struct SuspendOnceTask {
    first: bool,
    completed: Option<mpsc::Sender<()>>,
}

impl SchedulerTask for SuspendOnceTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        if self.first {
            self.first = false;
            SchedulerTaskPoll::Pending
        } else {
            if let Some(completed) = self.completed.take() {
                completed.send(()).expect("report resumed completion");
            }
            SchedulerTaskPoll::Complete
        }
    }
}

impl SchedulerTask for ReadyManyTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        if self.remaining_ready_polls == 0 {
            SchedulerTaskPoll::Complete
        } else {
            self.remaining_ready_polls -= 1;
            SchedulerTaskPoll::Ready
        }
    }
}

impl SchedulerTask for YieldOnceTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        if self.first {
            self.first = false;
            SchedulerTaskPoll::Ready
        } else {
            SchedulerTaskPoll::Complete
        }
    }
}

struct BlockingProbeTask {
    started: mpsc::Sender<u32>,
    gate: Arc<(Mutex<bool>, Condvar)>,
}

struct OpenGateOnDrop(Arc<(Mutex<bool>, Condvar)>);

impl Drop for OpenGateOnDrop {
    fn drop(&mut self) {
        let (open, changed) = &*self.0;
        *open.lock().expect("cleanup probe gate") = true;
        changed.notify_all();
    }
}

struct WakeDuringPollTask {
    entered: Option<mpsc::Sender<()>>,
    resume: mpsc::Receiver<()>,
    polls: Arc<Mutex<usize>>,
}

impl SchedulerTask for WakeDuringPollTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        let mut polls = self.polls.lock().expect("wake-race poll count");
        *polls += 1;
        let first = *polls == 1;
        drop(polls);
        if first {
            self.entered
                .take()
                .expect("first poll entry sender")
                .send(())
                .expect("report first poll entry");
            self.resume.recv().expect("release first poll");
            SchedulerTaskPoll::Pending
        } else {
            SchedulerTaskPoll::Complete
        }
    }
}

impl SchedulerTask for BlockingProbeTask {
    fn poll(&mut self, context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        self.started
            .send(context.worker().raw())
            .expect("report blocking probe start");
        let (open, changed) = &*self.gate;
        let mut open = open.lock().expect("probe gate");
        while !*open {
            open = changed.wait(open).expect("probe gate wait");
        }
        SchedulerTaskPoll::Complete
    }
}

fn configuration(scheduler_count: usize, task_capacity: usize) -> SchedulerConfiguration {
    SchedulerConfiguration::new(
        scheduler_count,
        scheduler_count,
        task_capacity,
        task_capacity,
        task_capacity,
        8,
    )
    .expect("scheduler configuration")
}

#[test]
fn scheduler_configuration_rejects_unbounded_zero_limits() {
    assert_eq!(
        SchedulerConfiguration::new(0, 1, 1, 1, 1, 1),
        Err(SchedulerConfigurationError::ZeroSchedulers)
    );
    assert_eq!(
        SchedulerConfiguration::new(1, 0, 1, 1, 1, 1),
        Err(SchedulerConfigurationError::ZeroWorkers)
    );
    assert_eq!(
        SchedulerConfiguration::new(1, 1, 0, 1, 1, 1),
        Err(SchedulerConfigurationError::ZeroTaskCapacity)
    );
    assert_eq!(
        SchedulerConfiguration::new(1, 1, 1, 0, 1, 1),
        Err(SchedulerConfigurationError::ZeroLocalQueueCapacity)
    );
    assert_eq!(
        SchedulerConfiguration::new(1, 1, 1, 1, 0, 1),
        Err(SchedulerConfigurationError::ZeroInjectionQueueCapacity)
    );
    assert_eq!(
        SchedulerConfiguration::new(1, 1, 1, 1, 1, 0),
        Err(SchedulerConfigurationError::ZeroInjectionPollInterval)
    );
    assert_eq!(
        SchedulerConfiguration::new(1, 1, 2, 1, 1, 1),
        Err(SchedulerConfigurationError::LocalQueueBelowTaskCapacity)
    );
    assert_eq!(
        SchedulerConfiguration::new(1, 2, 1, 1, 1, 1),
        Err(SchedulerConfigurationError::WorkerSchedulerCountMismatch)
    );
    assert_eq!(
        SchedulerConfiguration::new(2, 1, 1, 1, 1, 1),
        Err(SchedulerConfigurationError::WorkerSchedulerCountMismatch)
    );
    let bounded = SchedulerConfiguration::new(1, 1, 1, 1, 1, 1)
        .expect("base bounded scheduler configuration");
    assert_eq!(
        bounded.with_blocking_pool(0, 1),
        Err(SchedulerConfigurationError::ZeroBlockingWorkers)
    );
    assert_eq!(
        bounded.with_blocking_pool(1, 0),
        Err(SchedulerConfigurationError::ZeroBlockingQueueCapacity)
    );
    assert_eq!(
        bounded.with_event_driver(0, 1, 1),
        Err(SchedulerConfigurationError::ZeroExternalEventCapacity)
    );
    assert_eq!(
        bounded.with_event_driver(1, 0, 1),
        Err(SchedulerConfigurationError::ZeroTimerCapacity)
    );
    assert_eq!(
        bounded.with_event_driver(1, 1, 0),
        Err(SchedulerConfigurationError::ZeroEventDeliveryCapacity)
    );
}

#[test]
fn native_scheduler_preserves_ready_suspend_wake_and_completion_transitions() {
    let scheduler = NativeScheduler::new(configuration(1, 4)).expect("native scheduler");
    let polls = Arc::new(Mutex::new(Vec::new()));
    let task = scheduler
        .schedule(StepTask {
            polls: Arc::clone(&polls),
            step: 0,
        })
        .expect("schedule task");

    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("task suspends");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Suspended)
    );
    assert_eq!(polls.lock().expect("poll log").len(), 2);

    assert_eq!(scheduler.wake(task), Ok(true));
    assert_eq!(scheduler.wake(task), Ok(false));
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("task completes");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(polls.lock().expect("poll log").len(), 3);

    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.tasks_scheduled(), 1);
    assert_eq!(telemetry.polls(), 3);
    assert_eq!(telemetry.suspensions(), 1);
    assert_eq!(telemetry.completions(), 1);
}

#[test]
fn cancellation_is_cooperative_and_wakes_a_suspended_task() {
    let scheduler = NativeScheduler::new(configuration(1, 2)).expect("native scheduler");
    let task = scheduler
        .schedule(CancellationTask)
        .expect("schedule cancellation task");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("task suspends");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Suspended)
    );

    assert_eq!(scheduler.request_cancellation(task), Ok(true));
    assert_eq!(scheduler.request_cancellation(task), Ok(false));
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("task observes cancellation");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Cancelled)
    );
    assert_eq!(scheduler.telemetry().cancellations_observed(), 1);
}

#[test]
fn wake_racing_with_a_running_poll_is_retained_without_duplicate_queue_entries() {
    let scheduler = NativeScheduler::new(configuration(1, 2)).expect("native scheduler");
    let (entered_sender, entered_receiver) = mpsc::channel();
    let (resume_sender, resume_receiver) = mpsc::channel();
    let polls = Arc::new(Mutex::new(0));
    let task = scheduler
        .schedule(WakeDuringPollTask {
            entered: Some(entered_sender),
            resume: resume_receiver,
            polls: Arc::clone(&polls),
        })
        .expect("schedule wake-race task");
    entered_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("task enters first poll");

    assert_eq!(scheduler.wake(task), Ok(true));
    assert_eq!(scheduler.wake(task), Ok(false));
    resume_sender.send(()).expect("release first poll");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("retained wake completes task");

    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(*polls.lock().expect("wake-race poll count"), 2);
    assert_eq!(scheduler.telemetry().coalesced_wakes(), 1);
}

#[test]
fn task_panics_are_terminal_without_destroying_the_worker() {
    let scheduler = NativeScheduler::new(configuration(1, 2)).expect("native scheduler");
    let completions = Arc::new(Mutex::new(0));
    let panicked = scheduler.schedule(PanicTask).expect("schedule panic task");
    let completed = scheduler
        .schedule(CompleteTask {
            completions: Arc::clone(&completions),
        })
        .expect("schedule completion task");

    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("tasks terminate");
    assert_eq!(
        scheduler.task_state(panicked),
        Ok(SchedulerTaskState::Panicked)
    );
    assert_eq!(
        scheduler.task_state(completed),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(*completions.lock().expect("completion count"), 1);
    assert_eq!(scheduler.telemetry().panics(), 1);
}

#[test]
fn a_task_panic_storm_does_not_destroy_any_logical_scheduler_worker() {
    let scheduler = NativeScheduler::new(configuration(4, 128)).expect("native scheduler");
    let mut panicked_tasks = Vec::new();
    for _ in 0..64 {
        panicked_tasks.push(scheduler.schedule(PanicTask).expect("schedule panic task"));
    }
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("panic storm remains contained");
    for task in &panicked_tasks {
        assert_eq!(
            scheduler.task_state(*task),
            Ok(SchedulerTaskState::Panicked)
        );
    }
    assert_eq!(scheduler.telemetry().panics(), 64);
    for task in panicked_tasks {
        scheduler
            .release_terminal_task(task)
            .expect("release panicked task state");
    }

    let completions = Arc::new(Mutex::new(0));
    let mut continuation_tasks = Vec::new();
    for scheduler_raw in 1..=4 {
        continuation_tasks.push(
            scheduler
                .schedule_on(
                    SchedulerId::new(scheduler_raw),
                    SchedulerTaskMobility::Affine,
                    CompleteTask {
                        completions: Arc::clone(&completions),
                    },
                )
                .expect("schedule exact-worker continuation"),
        );
    }
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("every logical scheduler worker remains usable");
    for task in continuation_tasks {
        assert_eq!(
            scheduler.task_state(task),
            Ok(SchedulerTaskState::Completed)
        );
    }
    assert_eq!(*completions.lock().expect("completion count"), 4);
}

#[test]
fn task_capacity_is_bounded_until_retained_terminal_state_is_released() {
    let scheduler = NativeScheduler::new(configuration(1, 1)).expect("native scheduler");
    let first = scheduler
        .schedule(CancellationTask)
        .expect("schedule first task");
    assert_eq!(
        scheduler.schedule(CancellationTask),
        Err(SchedulerError::TaskCapacity)
    );
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("first task suspends");
    scheduler
        .request_cancellation(first)
        .expect("request cancellation");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("first task cancels");
    assert_eq!(
        scheduler.schedule(CancellationTask),
        Err(SchedulerError::TaskCapacity)
    );

    scheduler
        .release_terminal_task(first)
        .expect("release retained terminal task");
    let second = scheduler
        .schedule(CancellationTask)
        .expect("capacity becomes available");
    assert_eq!(
        scheduler.task_state(first),
        Err(SchedulerError::UnknownTask(first))
    );
    assert!(matches!(
        scheduler.task_state(second),
        Ok(SchedulerTaskState::Ready | SchedulerTaskState::Running | SchedulerTaskState::Suspended)
    ));
}

#[test]
fn bounded_global_injection_rejects_before_retaining_an_unqueueable_task() {
    let configuration =
        SchedulerConfiguration::new(1, 1, 4, 4, 1, 1).expect("bounded injection configuration");
    let scheduler = NativeScheduler::new(configuration).expect("native scheduler");
    let (started_sender, started_receiver) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _gate_cleanup = OpenGateOnDrop(Arc::clone(&gate));
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            BlockingProbeTask {
                started: started_sender,
                gate: Arc::clone(&gate),
            },
        )
        .expect("occupy normal worker");
    started_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("normal worker is occupied");

    scheduler
        .schedule(CancellationTask)
        .expect("fill injection queue");
    assert_eq!(
        scheduler.schedule(CancellationTask),
        Err(SchedulerError::InjectionQueueCapacity)
    );
    assert_eq!(scheduler.telemetry().retained_tasks(), 2);

    let (open, changed) = &*gate;
    *open.lock().expect("probe gate") = true;
    changed.notify_one();
}

#[test]
fn telemetry_tracks_ready_and_blocking_queue_depths_and_high_water_marks() {
    let configuration = configuration(1, 8)
        .with_blocking_pool(1, 4)
        .expect("bounded blocking configuration");
    let scheduler = NativeScheduler::new(configuration).expect("native scheduler");
    let (normal_started_sender, normal_started_receiver) = mpsc::channel();
    let normal_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _normal_gate_cleanup = OpenGateOnDrop(Arc::clone(&normal_gate));
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            BlockingProbeTask {
                started: normal_started_sender,
                gate: Arc::clone(&normal_gate),
            },
        )
        .expect("occupy normal worker");
    normal_started_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("normal worker is occupied");

    let completions = Arc::new(Mutex::new(0));
    scheduler
        .schedule_batch_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            (0..3)
                .map(|_| CompleteTask {
                    completions: Arc::clone(&completions),
                })
                .collect(),
        )
        .expect("fill local queue");
    let injection_task = scheduler
        .schedule(CancellationTask)
        .expect("fill injection queue");

    let (blocking_started_sender, blocking_started_receiver) = mpsc::channel();
    let blocking_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _blocking_gate_cleanup = OpenGateOnDrop(Arc::clone(&blocking_gate));
    let first_blocking_gate = Arc::clone(&blocking_gate);
    scheduler
        .submit_blocking(injection_task, move || {
            blocking_started_sender
                .send(())
                .expect("report blocking operation start");
            let (open, changed) = &*first_blocking_gate;
            let mut open = open.lock().expect("blocking gate");
            while !*open {
                open = changed.wait(open).expect("blocking gate wait");
            }
        })
        .expect("occupy blocking worker");
    blocking_started_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("blocking worker is occupied");
    scheduler
        .submit_blocking(injection_task, || {})
        .expect("queue first blocking operation");
    scheduler
        .submit_blocking(injection_task, || {})
        .expect("queue second blocking operation");

    let queued = scheduler.telemetry();
    assert_eq!(queued.local_queue_depth(), 3);
    assert_eq!(queued.maximum_local_queue_depth(), 3);
    assert_eq!(queued.injection_queue_depth(), 1);
    assert_eq!(queued.maximum_injection_queue_depth(), 1);
    assert_eq!(queued.blocking_queue_depth(), 2);
    assert_eq!(queued.maximum_blocking_queue_depth(), 2);
    assert_eq!(queued.active_blocking_operations(), 1);
    assert_eq!(queued.maximum_active_blocking_operations(), 1);

    let (open, changed) = &*blocking_gate;
    *open.lock().expect("blocking gate") = true;
    changed.notify_one();
    let (open, changed) = &*normal_gate;
    *open.lock().expect("normal gate") = true;
    changed.notify_one();
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("queued work drains");
    let drained = scheduler.telemetry();
    assert_eq!(drained.local_queue_depth(), 0);
    assert_eq!(drained.injection_queue_depth(), 0);
    assert_eq!(drained.blocking_queue_depth(), 0);
    assert_eq!(drained.active_blocking_operations(), 0);
    assert_eq!(drained.maximum_local_queue_depth(), 3);
    assert_eq!(drained.maximum_blocking_queue_depth(), 2);
}

#[test]
fn idle_workers_steal_ready_work_from_a_blocked_peer_queue() {
    let transitions = Arc::new(PermitRuntimeTransitions);
    let scheduler = NativeScheduler::new_with_runtime_transitions(configuration(3, 8), transitions)
        .expect("native scheduler with migration approval");
    let (started_sender, started_receiver) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _gate_cleanup = OpenGateOnDrop(Arc::clone(&gate));
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            BlockingProbeTask {
                started: started_sender,
                gate: Arc::clone(&gate),
            },
        )
        .expect("schedule blocking probe");
    started_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("first worker starts probe");
    let parked_deadline = std::time::Instant::now() + Duration::from_secs(5);
    while scheduler.telemetry().worker_threads_used() != 3 {
        assert!(
            std::time::Instant::now() < parked_deadline,
            "idle workers did not reach their park protocol"
        );
        std::thread::yield_now();
    }

    let completions = Arc::new(Mutex::new(0));
    scheduler
        .schedule_batch_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Movable,
            (0..4)
                .map(|_| CompleteTask {
                    completions: Arc::clone(&completions),
                })
                .collect(),
        )
        .expect("queue peer work batch");

    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while *completions.lock().expect("completion count") != 4 {
        if std::time::Instant::now() >= deadline {
            let telemetry = scheduler.telemetry();
            let (open, changed) = &*gate;
            *open.lock().expect("probe gate") = true;
            changed.notify_one();
            panic!("peer work was not stolen: {telemetry:?}");
        }
        std::thread::yield_now();
    }
    let (open, changed) = &*gate;
    *open.lock().expect("probe gate") = true;
    changed.notify_one();
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("all work completes");

    let telemetry = scheduler.telemetry();
    assert!(telemetry.tasks_stolen() >= 1);
    assert!(telemetry.worker_threads_used() >= 2);
    assert_eq!(telemetry.affine_tasks_stolen(), 0);
    assert!(telemetry.steal_searches() >= 1);
    assert!(telemetry.steal_victims_examined() >= telemetry.steal_successes());
    assert!(telemetry.steal_successes() >= 1);
    assert_eq!(
        telemetry.steal_searches(),
        telemetry
            .steal_successes()
            .saturating_add(telemetry.steal_failures())
    );
    assert!(telemetry.maximum_stolen_batch() >= 1);
}

#[test]
fn telemetry_reports_every_worker_lifecycle_through_shutdown() {
    let scheduler = NativeScheduler::new(configuration(2, 4)).expect("native scheduler");
    let parked_deadline = std::time::Instant::now() + Duration::from_secs(5);
    while scheduler.telemetry().worker_parks() < 2 {
        assert!(
            std::time::Instant::now() < parked_deadline,
            "workers did not enter the park protocol"
        );
        std::thread::yield_now();
    }

    let completions = Arc::new(Mutex::new(0));
    for scheduler_raw in 1..=2 {
        scheduler
            .schedule_on(
                SchedulerId::new(scheduler_raw),
                SchedulerTaskMobility::Affine,
                CompleteTask {
                    completions: Arc::clone(&completions),
                },
            )
            .expect("schedule exact-worker lifecycle probe");
    }
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("lifecycle probes complete");
    let final_telemetry = scheduler
        .shutdown_with_telemetry()
        .expect("scheduler shutdown telemetry");

    assert_eq!(final_telemetry.worker_starts(), 2);
    assert_eq!(final_telemetry.worker_stops(), 2);
    assert!(final_telemetry.worker_parks() >= 2);
    assert!(final_telemetry.worker_unparks() >= 2);
    assert_eq!(final_telemetry.worker_threads_used(), 2);
    assert_eq!(*completions.lock().expect("completion count"), 2);
}

#[test]
fn native_workers_report_gc_visible_bind_dispatch_suspend_resume_park_and_stop_transitions() {
    let transitions = Arc::new(RecordingRuntimeTransitions::default());
    let scheduler =
        NativeScheduler::new_with_runtime_transitions(configuration(1, 2), transitions.clone())
            .expect("native scheduler with runtime transitions");
    let task = scheduler
        .schedule(CancellationTask)
        .expect("schedule transition task");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("transition task suspends");
    scheduler.wake(task).expect("wake transition task");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("transition task suspends again");
    scheduler
        .request_cancellation(task)
        .expect("cancel transition task");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("transition task terminates");
    scheduler.shutdown().expect("scheduler shutdown");

    let events = transitions.events.lock().expect("runtime transition log");
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SchedulerRuntimeTransition::WorkerStarted { .. }))
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SchedulerRuntimeTransition::TaskDispatched { task: found, .. } if *found == task
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SchedulerRuntimeTransition::TaskSuspended { task: found, .. } if *found == task
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SchedulerRuntimeTransition::TaskResumed { task: found, .. } if *found == task
    )));
    assert!(events.iter().any(|event| matches!(
        event,
        SchedulerRuntimeTransition::TaskTerminal { task: found, .. } if *found == task
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SchedulerRuntimeTransition::WorkerParked { .. }))
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SchedulerRuntimeTransition::WorkerStopped { .. }))
    );
}

#[test]
fn gc_transition_hook_can_delay_migration_without_losing_ready_work() {
    let transitions = Arc::new(RecordingRuntimeTransitions {
        events: Mutex::new(Vec::new()),
        refuse_migration: true,
    });
    let scheduler =
        NativeScheduler::new_with_runtime_transitions(configuration(2, 4), transitions.clone())
            .expect("native scheduler with migration gate");
    let (started_sender, started_receiver) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            BlockingProbeTask {
                started: started_sender,
                gate: Arc::clone(&gate),
            },
        )
        .expect("occupy owning worker");
    started_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("owning worker is occupied");

    let completions = Arc::new(Mutex::new(0));
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Movable,
            CompleteTask {
                completions: Arc::clone(&completions),
            },
        )
        .expect("queue migration-gated task");
    assert!(
        scheduler
            .wait_until_idle(Duration::from_millis(20))
            .is_err()
    );
    assert_eq!(*completions.lock().expect("completion count"), 0);
    assert!(scheduler.telemetry().gc_delayed_migrations() > 0);

    let (open, changed) = &*gate;
    *open.lock().expect("probe gate") = true;
    changed.notify_one();
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("owning worker completes queued work");
    assert_eq!(*completions.lock().expect("completion count"), 1);
    assert!(
        transitions
            .events
            .lock()
            .expect("runtime transition log")
            .iter()
            .any(|event| matches!(event, SchedulerRuntimeTransition::TaskMigration { .. }))
    );
}

#[test]
fn bounded_blocking_pool_runs_off_normal_workers_and_rejects_queue_overflow() {
    let configuration = configuration(1, 6)
        .with_blocking_pool(1, 1)
        .expect("bounded blocking configuration");
    let scheduler = NativeScheduler::new(configuration).expect("native scheduler");
    let wake_target = scheduler
        .schedule(SuspendOnceTask {
            first: true,
            completed: None,
        })
        .expect("schedule wake target");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("wake target suspends");

    let (normal_started_sender, normal_started_receiver) = mpsc::channel();
    let normal_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _normal_gate_cleanup = OpenGateOnDrop(Arc::clone(&normal_gate));
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            BlockingProbeTask {
                started: normal_started_sender,
                gate: Arc::clone(&normal_gate),
            },
        )
        .expect("occupy normal worker");
    normal_started_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("normal worker is occupied");

    let (blocking_started_sender, blocking_started_receiver) = mpsc::channel();
    let blocking_gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _blocking_gate_cleanup = OpenGateOnDrop(Arc::clone(&blocking_gate));
    let first_gate = Arc::clone(&blocking_gate);
    scheduler
        .submit_blocking(wake_target, move || {
            blocking_started_sender
                .send(())
                .expect("report blocking operation start");
            let (open, changed) = &*first_gate;
            let mut open = open.lock().expect("blocking gate");
            while !*open {
                open = changed.wait(open).expect("blocking gate wait");
            }
        })
        .expect("submit running blocking operation");
    blocking_started_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("blocking operation runs while normal worker is occupied");
    scheduler
        .submit_blocking(wake_target, || {})
        .expect("fill blocking queue");
    assert_eq!(
        scheduler.submit_blocking(wake_target, || {}),
        Err(SchedulerError::BlockingQueueCapacity)
    );

    let (open, changed) = &*blocking_gate;
    *open.lock().expect("blocking gate") = true;
    changed.notify_one();
    let (open, changed) = &*normal_gate;
    *open.lock().expect("normal gate") = true;
    changed.notify_one();
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("normal scheduler drains resumed task");
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.blocking_submissions(), 2);
    assert_eq!(telemetry.blocking_queue_rejections(), 1);
    assert_eq!(telemetry.blocking_completions(), 2);
}

#[test]
fn blocking_operation_panic_is_contained_and_worker_continues() {
    let scheduler = NativeScheduler::new(configuration(1, 4)).expect("native scheduler");
    let wake_target = scheduler
        .schedule(SuspendOnceTask {
            first: true,
            completed: None,
        })
        .expect("schedule wake target");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("wake target suspends");
    let completed = Arc::new(Mutex::new(0));

    scheduler
        .submit_blocking(wake_target, || {
            panic!("blocking operation failure must remain contained")
        })
        .expect("submit panicking operation");
    let completed_from_worker = Arc::clone(&completed);
    scheduler
        .submit_blocking(wake_target, move || {
            *completed_from_worker
                .lock()
                .expect("blocking completion count") += 1;
        })
        .expect("submit operation after panic");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("blocking pool and scheduler become idle");

    assert_eq!(*completed.lock().expect("blocking completion count"), 1);
    assert_eq!(scheduler.telemetry().blocking_panics(), 1);
}

#[test]
fn host_timer_and_external_event_wake_only_the_exact_registered_tasks() {
    let configuration = configuration(1, 6)
        .with_event_driver(2, 2, 2)
        .expect("bounded event-driver configuration");
    let scheduler = NativeScheduler::new(configuration).expect("native scheduler");
    let (timer_sender, timer_receiver) = mpsc::channel();
    let timer_task = scheduler
        .schedule(SuspendOnceTask {
            first: true,
            completed: Some(timer_sender),
        })
        .expect("schedule timer task");
    let (event_sender, event_receiver) = mpsc::channel();
    let event_task = scheduler
        .schedule(SuspendOnceTask {
            first: true,
            completed: Some(event_sender),
        })
        .expect("schedule event task");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("event tasks suspend");

    scheduler
        .schedule_wake_after(timer_task, Duration::from_millis(5))
        .expect("register timer");
    let event = scheduler
        .register_external_event(event_task)
        .expect("register external event");
    assert_eq!(scheduler.signal_external_event(event), Ok(true));
    assert_eq!(scheduler.signal_external_event(event), Ok(false));

    timer_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("timer task resumes");
    event_receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("event task resumes");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("delivered tasks complete");
    assert_eq!(
        scheduler.task_state(timer_task),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(
        scheduler.task_state(event_task),
        Ok(SchedulerTaskState::Completed)
    );
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.timers_delivered(), 1);
    assert_eq!(telemetry.external_events_delivered(), 1);
    assert_eq!(telemetry.external_event_signals_coalesced(), 1);
}

#[test]
fn deterministic_virtual_timer_never_consults_wall_clock() {
    let mut scheduler = DeterministicScheduler::recording(configuration(1, 2));
    let task = scheduler
        .schedule(SuspendOnceTask {
            first: true,
            completed: None,
        })
        .expect("schedule virtual timer task");
    scheduler
        .run_until_idle(2)
        .expect("virtual timer task suspends");
    scheduler
        .schedule_wake_at(task, 10)
        .expect("register virtual timer");

    assert_eq!(scheduler.advance_virtual_work(9), Ok(0));
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Suspended)
    );
    assert_eq!(scheduler.advance_virtual_work(1), Ok(1));
    scheduler
        .run_until_idle(2)
        .expect("virtual timer task completes");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(scheduler.virtual_work(), 10);
}

#[test]
fn deterministic_external_event_is_bounded_and_coalesced() {
    let configuration = configuration(1, 2)
        .with_event_driver(1, 1, 1)
        .expect("bounded deterministic event configuration");
    let mut scheduler = DeterministicScheduler::recording(configuration);
    let task = scheduler
        .schedule(SuspendOnceTask {
            first: true,
            completed: None,
        })
        .expect("schedule deterministic event task");
    scheduler
        .run_until_idle(2)
        .expect("deterministic event task suspends");
    let event = scheduler
        .register_external_event(task)
        .expect("register deterministic external event");
    assert_eq!(
        scheduler.register_external_event(task),
        Err(SchedulerError::ExternalEventCapacity)
    );
    assert_eq!(scheduler.signal_external_event(event), Ok(true));
    assert_eq!(scheduler.signal_external_event(event), Ok(false));
    scheduler
        .run_until_idle(2)
        .expect("deterministic event task completes");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(scheduler.telemetry().external_events_delivered(), 1);
    assert_eq!(scheduler.telemetry().external_event_signals_coalesced(), 1);
    scheduler
        .release_external_event(event)
        .expect("release deterministic external event");
    assert_eq!(
        scheduler.release_external_event(event),
        Err(SchedulerError::UnknownExternalEvent(event))
    );
}

#[test]
fn concurrent_wake_and_cancellation_races_do_not_lose_or_duplicate_tasks() {
    let scheduler = NativeScheduler::new_with_runtime_transitions(
        configuration(4, 128),
        Arc::new(PermitRuntimeTransitions),
    )
    .expect("native scheduler with migration approval");
    let mut tasks = Vec::new();
    for _ in 0..100 {
        tasks.push(
            scheduler
                .schedule(CancellationTask)
                .expect("schedule cancellation stress task"),
        );
    }
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("stress tasks suspend");

    std::thread::scope(|scope| {
        for worker in 0..4 {
            let tasks = &tasks;
            let scheduler = &scheduler;
            scope.spawn(move || {
                for (index, task) in tasks.iter().copied().enumerate() {
                    if index % 4 == worker {
                        let _ = scheduler.wake(task);
                        let _ = scheduler.request_cancellation(task);
                        let _ = scheduler.wake(task);
                    }
                }
            });
        }
    });
    if let Err(error) = scheduler.wait_until_idle(Duration::from_secs(5)) {
        panic!(
            "stress tasks failed to observe cancellation: {error:?}; {:?}",
            scheduler.telemetry()
        );
    }
    for task in tasks {
        assert_eq!(
            scheduler.task_state(task),
            Ok(SchedulerTaskState::Cancelled)
        );
    }
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.cancellations_observed(), 100);
    assert_eq!(telemetry.local_queue_depth(), 0);
    assert_eq!(telemetry.injection_queue_depth(), 0);
    assert_eq!(telemetry.stale_ready_entries(), 0);
}

#[test]
fn shutdown_cancels_dormant_events_and_timers_without_waiting_for_deadlines() {
    let scheduler = NativeScheduler::new(configuration(1, 4)).expect("native scheduler");
    let task = scheduler
        .schedule(CancellationTask)
        .expect("schedule dormant task");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("dormant task suspends");
    scheduler
        .register_external_event(task)
        .expect("register dormant event");
    scheduler
        .schedule_wake_after(task, Duration::from_mins(1))
        .expect("register dormant timer");

    let started = std::time::Instant::now();
    scheduler.shutdown().expect("bounded scheduler shutdown");
    assert!(started.elapsed() < Duration::from_secs(1));
}

#[test]
fn deterministic_scheduler_records_and_replays_every_ready_task_choice() {
    let mut recording = DeterministicScheduler::recording(configuration(2, 4));
    let first = recording
        .schedule(YieldOnceTask { first: true })
        .expect("first deterministic task");
    let second = recording
        .schedule(YieldOnceTask { first: true })
        .expect("second deterministic task");
    recording
        .run_until_idle(8)
        .expect("record deterministic execution");
    assert_eq!(
        recording.task_state(first),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(
        recording.task_state(second),
        Ok(SchedulerTaskState::Completed)
    );
    let transcript = recording.transcript().to_vec();
    assert_eq!(transcript.len(), 4);
    assert_eq!(transcript[0].decision(), 1);
    assert_eq!(transcript[0].enabled(), &[first, second]);
    assert_eq!(transcript[0].selected(), first);
    assert_eq!(transcript[1].enabled(), &[second, first]);

    let mut replay = DeterministicScheduler::replaying(configuration(2, 4), transcript.clone());
    let replay_first = replay
        .schedule(YieldOnceTask { first: true })
        .expect("first replay task");
    let replay_second = replay
        .schedule(YieldOnceTask { first: true })
        .expect("second replay task");
    replay
        .run_until_idle(8)
        .expect("replay deterministic execution");
    assert_eq!(replay_first, first);
    assert_eq!(replay_second, second);
    assert_eq!(replay.transcript(), transcript);
    assert!(replay.replay_complete());
}

#[test]
fn deterministic_ready_tail_prevents_an_always_ready_task_from_starving_a_peer() {
    let mut scheduler = DeterministicScheduler::recording(configuration(1, 2));
    let busy = scheduler
        .schedule(ReadyManyTask {
            remaining_ready_polls: 8,
        })
        .expect("busy deterministic task");
    let peer = scheduler
        .schedule(CompleteTask {
            completions: Arc::new(Mutex::new(0)),
        })
        .expect("peer deterministic task");
    scheduler
        .run_until_idle(16)
        .expect("fair deterministic execution");

    let peer_position = scheduler
        .transcript()
        .iter()
        .position(|decision| decision.selected() == peer)
        .expect("peer is selected");
    assert!(peer_position <= 1, "busy task monopolized the ready queue");
    assert_eq!(
        scheduler.task_state(busy),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(
        scheduler.task_state(peer),
        Ok(SchedulerTaskState::Completed)
    );
}

#[test]
fn deterministic_telemetry_tracks_ready_queue_depth_and_high_water_mark() {
    let mut scheduler = DeterministicScheduler::recording(configuration(2, 4));
    scheduler
        .schedule(YieldOnceTask { first: true })
        .expect("first deterministic queue task");
    scheduler
        .schedule(YieldOnceTask { first: true })
        .expect("second deterministic queue task");

    let queued = scheduler.telemetry();
    assert_eq!(queued.local_queue_depth(), 2);
    assert_eq!(queued.maximum_local_queue_depth(), 2);
    scheduler
        .run_until_idle(8)
        .expect("drain deterministic ready queue");
    let drained = scheduler.telemetry();
    assert_eq!(drained.local_queue_depth(), 0);
    assert_eq!(drained.maximum_local_queue_depth(), 2);
}

#[test]
fn deterministic_replay_fails_closed_when_the_recorded_task_is_not_ready() {
    let mut recording = DeterministicScheduler::recording(configuration(1, 2));
    recording
        .schedule(YieldOnceTask { first: true })
        .expect("recorded task");
    recording.run_until_idle(2).expect("record transcript");
    let transcript = recording.transcript().to_vec();

    let mut replay = DeterministicScheduler::replaying(configuration(1, 2), transcript);
    replay
        .schedule(CancellationTask)
        .expect("different replay task shape");
    assert_eq!(
        replay.run_until_idle(2),
        Err(SchedulerError::ReplayEnabledSetMismatch)
    );
}

#[test]
fn deterministic_exploration_generates_bounded_alternative_choice_prefixes() {
    let mut recording = DeterministicScheduler::recording(configuration(1, 2));
    let first = recording
        .schedule(YieldOnceTask { first: true })
        .expect("first exploration task");
    let second = recording
        .schedule(YieldOnceTask { first: true })
        .expect("second exploration task");
    recording
        .run_until_idle(8)
        .expect("record exploration baseline");

    let prefixes = recording
        .exploration_prefixes(2)
        .expect("derive bounded alternative prefixes");
    assert_eq!(prefixes.len(), 2);
    assert_eq!(prefixes[0], vec![second]);
    assert_eq!(prefixes[1], vec![first, first]);

    let mut explored = DeterministicScheduler::exploring(configuration(1, 2), prefixes[0].clone());
    let explored_first = explored
        .schedule(YieldOnceTask { first: true })
        .expect("first explored task");
    let explored_second = explored
        .schedule(YieldOnceTask { first: true })
        .expect("second explored task");
    explored
        .run_until_idle(8)
        .expect("execute alternative prefix");
    assert_eq!(explored_first, first);
    assert_eq!(explored_second, second);
    assert_eq!(explored.transcript()[0].selected(), second);
    assert_eq!(
        recording.exploration_prefixes(0),
        Err(SchedulerError::ExplorationBudget)
    );
}
