use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::Duration;

use pop_runtime_collector::SchedulerId;
use pop_runtime_interface::{ManagedReference, RootPublication, RootSlot, SafePointId, StackMap};
use pop_runtime_native::{
    DeterministicScheduler, NativeScheduler, SchedulerConfiguration, SchedulerConfigurationError,
    SchedulerError, SchedulerRuntimeTransition, SchedulerRuntimeTransitionControl,
    SchedulerRuntimeTransitionFailure, SchedulerRuntimeTransitions, SchedulerTask,
    SchedulerTaskContext, SchedulerTaskFrame, SchedulerTaskFrameError, SchedulerTaskFrameFailure,
    SchedulerTaskId, SchedulerTaskMobility, SchedulerTaskPoll, SchedulerTaskState,
    SchedulerWorkBudgetError, SchedulerWorkBudgetStatus, abi_safe_point, native_epoch_telemetry,
    pop_rt_allocate_object, pop_rt_release_root, pop_rt_retain_root, request_abi_collection,
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

struct RejectDispatchRuntimeTransitions {
    attempted: Mutex<Option<mpsc::Sender<()>>>,
}

impl SchedulerRuntimeTransitions for RejectDispatchRuntimeTransitions {
    fn apply(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure> {
        if matches!(
            transition,
            SchedulerRuntimeTransition::TaskDispatched { .. }
        ) {
            if let Some(attempted) = self
                .attempted
                .lock()
                .expect("dispatch rejection sender")
                .take()
            {
                attempted.send(()).expect("report dispatch rejection");
            }
            Err(SchedulerRuntimeTransitionFailure::CollectorState)
        } else {
            Ok(SchedulerRuntimeTransitionControl::Continue)
        }
    }
}

struct RejectWorkerStartRuntimeTransitions {
    attempted: Mutex<Option<mpsc::Sender<SchedulerRuntimeTransition>>>,
}

struct PanicWorkerStartRuntimeTransitions {
    attempted: Mutex<Option<mpsc::Sender<SchedulerRuntimeTransition>>>,
}

struct RejectMigrationRuntimeTransitions {
    attempted: Mutex<Option<mpsc::Sender<SchedulerRuntimeTransition>>>,
}

impl SchedulerRuntimeTransitions for RejectMigrationRuntimeTransitions {
    fn apply(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure> {
        if matches!(transition, SchedulerRuntimeTransition::TaskMigration { .. }) {
            if let Some(attempted) = self
                .attempted
                .lock()
                .expect("migration rejection sender")
                .take()
            {
                attempted
                    .send(transition)
                    .expect("report migration rejection");
            }
            Err(SchedulerRuntimeTransitionFailure::CollectorState)
        } else {
            Ok(SchedulerRuntimeTransitionControl::Continue)
        }
    }
}

impl SchedulerRuntimeTransitions for PanicWorkerStartRuntimeTransitions {
    fn apply(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure> {
        if matches!(transition, SchedulerRuntimeTransition::WorkerStarted { .. }) {
            if let Some(attempted) = self
                .attempted
                .lock()
                .expect("worker-start panic sender")
                .take()
            {
                attempted
                    .send(transition)
                    .expect("report worker-start panic");
            }
            panic!("trusted runtime transition panic");
        }
        Ok(SchedulerRuntimeTransitionControl::Continue)
    }
}

impl SchedulerRuntimeTransitions for RejectWorkerStartRuntimeTransitions {
    fn apply(
        &self,
        transition: SchedulerRuntimeTransition,
    ) -> Result<SchedulerRuntimeTransitionControl, SchedulerRuntimeTransitionFailure> {
        if matches!(transition, SchedulerRuntimeTransition::WorkerStarted { .. }) {
            if let Some(attempted) = self
                .attempted
                .lock()
                .expect("worker-start rejection sender")
                .take()
            {
                attempted
                    .send(transition)
                    .expect("report worker-start rejection");
            }
            Err(SchedulerRuntimeTransitionFailure::CollectorState)
        } else {
            Ok(SchedulerRuntimeTransitionControl::Continue)
        }
    }
}

fn explicit_empty_frame(id: u32) -> StackMap {
    StackMap::new(SafePointId::new(id), Vec::new()).expect("empty task frame map")
}

fn explicit_empty_publication(map: StackMap) -> RootPublication {
    RootPublication::new(map, Vec::new()).expect("empty task frame publication")
}

struct FrameLifecycleTask {
    events: Arc<Mutex<Vec<&'static str>>>,
    map: StackMap,
    first: bool,
}

impl SchedulerTaskFrame for FrameLifecycleTask {
    fn frame_stack_map(&self) -> StackMap {
        self.map.clone()
    }

    fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
        self.events.lock().expect("frame event log").push("publish");
        Ok(explicit_empty_publication(self.map.clone()))
    }

    fn restore_frame_roots(
        &mut self,
        publication: RootPublication,
    ) -> Result<(), SchedulerTaskFrameError> {
        if publication.stack_map() != &self.map {
            return Err(SchedulerTaskFrameError::RestorationRejected);
        }
        self.events.lock().expect("frame event log").push("restore");
        Ok(())
    }
}

impl SchedulerTask for FrameLifecycleTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        self.events.lock().expect("frame event log").push("poll");
        if self.first {
            self.first = false;
            SchedulerTaskPoll::Pending
        } else {
            SchedulerTaskPoll::Complete
        }
    }
}

struct InvalidInitialFrameTask;

impl SchedulerTaskFrame for InvalidInitialFrameTask {
    fn frame_stack_map(&self) -> StackMap {
        explicit_empty_frame(901)
    }

    fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
        Ok(explicit_empty_publication(explicit_empty_frame(902)))
    }

    fn restore_frame_roots(
        &mut self,
        _publication: RootPublication,
    ) -> Result<(), SchedulerTaskFrameError> {
        Ok(())
    }
}

impl SchedulerTask for InvalidInitialFrameTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        panic!("invalid initial frame must never enter a ready queue")
    }
}

struct BatchFrameTask {
    valid: bool,
}

impl SchedulerTaskFrame for BatchFrameTask {
    fn frame_stack_map(&self) -> StackMap {
        explicit_empty_frame(906)
    }

    fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
        let map = if self.valid {
            self.frame_stack_map()
        } else {
            explicit_empty_frame(907)
        };
        Ok(explicit_empty_publication(map))
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

impl SchedulerTask for BatchFrameTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        SchedulerTaskPoll::Complete
    }
}

struct RejectRestoreTask {
    attempted: Option<mpsc::Sender<()>>,
}

struct ManagedFrameTask {
    value: ManagedReference,
    first: bool,
}

struct EpochPollingTask;

impl SchedulerTask for EpochPollingTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        assert!(request_abi_collection());
        assert_eq!(abi_safe_point(931, &[]), 1);
        assert_eq!(abi_safe_point(932, &[]), 1);
        SchedulerTaskPoll::Complete
    }
}

struct BudgetExhaustionTask {
    polls: Arc<Mutex<Vec<&'static str>>>,
    first: bool,
}

impl SchedulerTask for BudgetExhaustionTask {
    fn poll(&mut self, context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        self.polls.lock().expect("budget poll log").push("budget");
        if self.first {
            self.first = false;
            assert_eq!(
                context.consume_work(0),
                Err(SchedulerWorkBudgetError::ZeroWorkUnits)
            );
            assert_eq!(context.remaining_work(), 2);
            assert_eq!(
                context.consume_work(2),
                Ok(SchedulerWorkBudgetStatus::Exhausted)
            );
        }
        SchedulerTaskPoll::Pending
    }
}

struct OrderedCompleteTask {
    polls: Arc<Mutex<Vec<&'static str>>>,
}

impl SchedulerTask for OrderedCompleteTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        self.polls.lock().expect("budget poll log").push("peer");
        SchedulerTaskPoll::Complete
    }
}

impl SchedulerTaskFrame for ManagedFrameTask {
    fn frame_stack_map(&self) -> StackMap {
        StackMap::new(SafePointId::new(909), vec![RootSlot::new(0)])
            .expect("managed task frame map")
    }

    fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
        RootPublication::new(self.frame_stack_map(), vec![Some(self.value)])
            .map_err(|_| SchedulerTaskFrameError::PublicationRejected)
    }

    fn restore_frame_roots(
        &mut self,
        publication: RootPublication,
    ) -> Result<(), SchedulerTaskFrameError> {
        if publication.stack_map() != &self.frame_stack_map() {
            return Err(SchedulerTaskFrameError::RestorationRejected);
        }
        self.value = publication
            .managed_references()
            .next()
            .ok_or(SchedulerTaskFrameError::RestorationRejected)?;
        Ok(())
    }
}

impl SchedulerTask for ManagedFrameTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        if self.first {
            self.first = false;
            SchedulerTaskPoll::Pending
        } else {
            let root = pop_rt_retain_root(self.value.raw());
            assert_ne!(root, 0, "retained frame value must survive collection");
            assert_eq!(pop_rt_release_root(root), 1);
            SchedulerTaskPoll::Complete
        }
    }
}

impl SchedulerTaskFrame for RejectRestoreTask {
    fn frame_stack_map(&self) -> StackMap {
        explicit_empty_frame(908)
    }

    fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
        Ok(explicit_empty_publication(self.frame_stack_map()))
    }

    fn restore_frame_roots(
        &mut self,
        _publication: RootPublication,
    ) -> Result<(), SchedulerTaskFrameError> {
        if let Some(attempted) = self.attempted.take() {
            attempted.send(()).expect("report rejected restoration");
        }
        Err(SchedulerTaskFrameError::RestorationRejected)
    }
}

impl SchedulerTask for RejectRestoreTask {
    fn poll(&mut self, _context: &SchedulerTaskContext) -> SchedulerTaskPoll {
        panic!("rejected root installation must prevent task polling")
    }
}

macro_rules! impl_explicit_rootless_frame {
    ($task:ty, $safe_point:expr) => {
        impl SchedulerTaskFrame for $task {
            fn frame_stack_map(&self) -> StackMap {
                explicit_empty_frame($safe_point)
            }

            fn publish_frame_roots(&mut self) -> Result<RootPublication, SchedulerTaskFrameError> {
                Ok(explicit_empty_publication(self.frame_stack_map()))
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
    };
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

impl_explicit_rootless_frame!(StepTask, 910);
impl_explicit_rootless_frame!(CancellationTask, 911);
impl_explicit_rootless_frame!(PanicTask, 912);
impl_explicit_rootless_frame!(CompleteTask, 913);
impl_explicit_rootless_frame!(YieldOnceTask, 914);
impl_explicit_rootless_frame!(ReadyManyTask, 915);
impl_explicit_rootless_frame!(SuspendOnceTask, 916);
impl_explicit_rootless_frame!(WakeDuringPollTask, 917);
impl_explicit_rootless_frame!(BlockingProbeTask, 918);
impl_explicit_rootless_frame!(EpochPollingTask, 920);
impl_explicit_rootless_frame!(BudgetExhaustionTask, 921);
impl_explicit_rootless_frame!(OrderedCompleteTask, 922);

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

fn wait_until_scheduler_closed(scheduler: &NativeScheduler, failure: &str) {
    let closure_deadline = std::time::Instant::now() + Duration::from_secs(1);
    loop {
        match scheduler.wake(SchedulerTaskId::new(u64::MAX)) {
            Err(SchedulerError::Closed) => break,
            Err(SchedulerError::UnknownTask(_)) => {
                assert!(
                    std::time::Instant::now() < closure_deadline,
                    "{failure} did not close the scheduler"
                );
                std::thread::yield_now();
            }
            result => panic!("unexpected scheduler closure probe: {result:?}"),
        }
    }
}

#[test]
fn native_task_frames_restore_before_poll_and_retain_after_nonterminal_poll() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let scheduler = NativeScheduler::new(configuration(1, 2)).expect("native scheduler");
    let task = scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            FrameLifecycleTask {
                events: Arc::clone(&events),
                map: explicit_empty_frame(900),
                first: true,
            },
        )
        .expect("schedule framed task");

    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("first suspension");
    assert_eq!(
        *events.lock().expect("frame event log"),
        ["publish", "restore", "poll", "publish"]
    );
    let suspended = scheduler.telemetry();
    assert_eq!(suspended.retained_frame_root_containers(), 1);
    assert_eq!(suspended.frame_root_retentions(), 2);
    assert_eq!(suspended.frame_root_restorations(), 1);

    assert!(scheduler.wake(task).expect("wake framed task"));
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("terminal completion");
    assert_eq!(
        *events.lock().expect("frame event log"),
        ["publish", "restore", "poll", "publish", "restore", "poll"]
    );
    let final_telemetry = scheduler
        .shutdown_with_telemetry()
        .expect("shutdown framed scheduler");
    assert_eq!(final_telemetry.retained_frame_root_containers(), 0);
    assert_eq!(final_telemetry.maximum_retained_frame_root_containers(), 1);
    assert_eq!(final_telemetry.frame_root_retentions(), 2);
    assert_eq!(final_telemetry.frame_root_restorations(), 2);
    assert_eq!(final_telemetry.frame_root_failures(), 0);
    assert_eq!(final_telemetry.blocking_shutdowns(), 1);
    assert_eq!(final_telemetry.blocking_shutdown_delay().samples(), 1);
    assert!(
        final_telemetry
            .blocking_shutdown_delay()
            .maximum_work_units()
            > 0
    );
}

#[test]
fn native_workers_register_manage_detach_and_unregister_exact_mutators() {
    let scheduler = NativeScheduler::new(configuration(2, 4)).expect("native scheduler");
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
            .expect("schedule exact-worker task");
    }
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("worker tasks complete");
    let telemetry = scheduler
        .shutdown_with_telemetry()
        .expect("worker mutators unregister");

    assert_eq!(telemetry.mutator_registrations(), 2);
    assert_eq!(telemetry.managed_mutator_transitions(), 2);
    assert_eq!(telemetry.detached_mutator_transitions(), 2);
    assert_eq!(telemetry.mutator_unregistrations(), 2);
}

#[test]
fn rejected_worker_start_closes_scheduler_before_further_admission() {
    let (attempted, observed) = mpsc::channel();
    let transitions = Arc::new(RejectWorkerStartRuntimeTransitions {
        attempted: Mutex::new(Some(attempted)),
    });
    let scheduler = NativeScheduler::new_with_runtime_transitions(configuration(1, 1), transitions)
        .expect("native scheduler starts host worker");
    let transition = observed
        .recv_timeout(Duration::from_secs(1))
        .expect("worker-start rejection observed");
    wait_until_scheduler_closed(&scheduler, "worker-start rejection");

    assert_eq!(
        scheduler.schedule(CompleteTask {
            completions: Arc::new(Mutex::new(0)),
        }),
        Err(SchedulerError::Closed)
    );
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.retained_tasks(), 0);
    assert_eq!(telemetry.mutator_registrations(), 1);
    assert_eq!(telemetry.mutator_unregistrations(), 1);
    assert_eq!(
        scheduler.shutdown(),
        Err(SchedulerError::RuntimeTransition {
            transition,
            failure: SchedulerRuntimeTransitionFailure::CollectorState,
        })
    );
}

#[test]
fn panicked_worker_runtime_transition_closes_scheduler_globally() {
    let (attempted, observed) = mpsc::channel();
    let transitions = Arc::new(PanicWorkerStartRuntimeTransitions {
        attempted: Mutex::new(Some(attempted)),
    });
    let scheduler = NativeScheduler::new_with_runtime_transitions(configuration(1, 1), transitions)
        .expect("native scheduler starts host worker");
    let transition = observed
        .recv_timeout(Duration::from_secs(1))
        .expect("worker-start panic observed");
    wait_until_scheduler_closed(&scheduler, "worker runtime panic");

    assert_eq!(
        scheduler.schedule(CompleteTask {
            completions: Arc::new(Mutex::new(0)),
        }),
        Err(SchedulerError::Closed)
    );
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.mutator_registrations(), 1);
    assert_eq!(telemetry.mutator_unregistrations(), 1);
    assert_eq!(
        scheduler.shutdown(),
        Err(SchedulerError::RuntimeTransition {
            transition,
            failure: SchedulerRuntimeTransitionFailure::CollectorState,
        })
    );
}

#[test]
fn rejected_dispatch_detaches_and_unregisters_without_losing_frame_roots() {
    let (attempted, observed) = mpsc::channel();
    let transitions = Arc::new(RejectDispatchRuntimeTransitions {
        attempted: Mutex::new(Some(attempted)),
    });
    let scheduler = NativeScheduler::new_with_runtime_transitions(configuration(1, 1), transitions)
        .expect("native scheduler");
    scheduler
        .schedule(CompleteTask {
            completions: Arc::new(Mutex::new(0)),
        })
        .expect("schedule rejected dispatch");
    observed
        .recv_timeout(Duration::from_secs(1))
        .expect("dispatch rejection observed");

    let cleanup_deadline = std::time::Instant::now() + Duration::from_secs(1);
    while scheduler.telemetry().mutator_unregistrations() == 0 {
        assert!(
            std::time::Instant::now() < cleanup_deadline,
            "failed worker did not unregister its mutator"
        );
        std::thread::yield_now();
    }
    let failed = scheduler.telemetry();
    assert_eq!(failed.managed_mutator_transitions(), 1);
    assert_eq!(failed.detached_mutator_transitions(), 1);
    assert_eq!(failed.mutator_unregistrations(), 1);
    assert_eq!(failed.retained_frame_root_containers(), 1);
    assert_eq!(
        scheduler.shutdown(),
        Err(SchedulerError::RuntimeTransition {
            transition: SchedulerRuntimeTransition::TaskDispatched {
                task: SchedulerTaskId::new(1),
                worker: pop_runtime_native::SchedulerWorkerId::new(1),
                scheduler: SchedulerId::new(1),
            },
            failure: SchedulerRuntimeTransitionFailure::CollectorState,
        })
    );
}

#[test]
fn managed_safe_point_acknowledges_the_current_collector_epoch_once() {
    let before = native_epoch_telemetry();
    let scheduler = NativeScheduler::new(configuration(1, 1)).expect("native scheduler");
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            EpochPollingTask,
        )
        .expect("schedule epoch poll");
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("epoch task completes");
    scheduler.shutdown().expect("scheduler shutdown");
    let after = native_epoch_telemetry();

    assert_eq!(after.acknowledgements(), before.acknowledgements() + 1);
}

#[test]
fn suspended_native_frame_keeps_managed_value_alive_through_forced_collection() {
    let value = pop_rt_allocate_object(0);
    assert_ne!(value, 0, "allocate managed frame value");
    let scheduler = NativeScheduler::new(configuration(1, 1)).expect("native scheduler");
    let task = scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            ManagedFrameTask {
                value: ManagedReference::new(value),
                first: true,
            },
        )
        .expect("schedule managed frame");
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("managed task suspension");

    assert!(request_abi_collection());
    for safe_point in 920..928 {
        assert_eq!(abi_safe_point(safe_point, &[]), 1);
    }

    assert!(scheduler.wake(task).expect("wake managed frame"));
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("managed task completion");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Completed)
    );
    scheduler.shutdown().expect("scheduler shutdown");
}

#[test]
fn invalid_initial_frame_fails_before_identity_or_queue_publication() {
    let scheduler = NativeScheduler::new(configuration(1, 2)).expect("native scheduler");

    assert_eq!(
        scheduler.schedule(InvalidInitialFrameTask),
        Err(SchedulerError::TaskFrame {
            task: SchedulerTaskId::new(1),
            failure: SchedulerTaskFrameFailure::PublicationShape,
        })
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let admitted = scheduler
        .schedule(FrameLifecycleTask {
            events,
            map: explicit_empty_frame(903),
            first: true,
        })
        .expect("failed admission must not consume task identity");
    assert_eq!(admitted, SchedulerTaskId::new(1));
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.tasks_scheduled(), 1);
    assert_eq!(telemetry.frame_root_failures(), 1);
    scheduler.shutdown().expect("scheduler shutdown");
}

#[test]
fn failed_batch_admission_releases_prior_roots_and_preserves_task_identity() {
    let scheduler = NativeScheduler::new(configuration(1, 2)).expect("native scheduler");

    assert_eq!(
        scheduler.schedule_batch_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            vec![
                BatchFrameTask { valid: true },
                BatchFrameTask { valid: false }
            ],
        ),
        Err(SchedulerError::TaskFrame {
            task: SchedulerTaskId::new(2),
            failure: SchedulerTaskFrameFailure::PublicationShape,
        })
    );
    let failed = scheduler.telemetry();
    assert_eq!(failed.retained_tasks(), 0);
    assert_eq!(failed.retained_frame_root_containers(), 0);
    assert_eq!(failed.frame_root_retentions(), 1);
    assert_eq!(failed.frame_root_releases(), 1);
    assert_eq!(failed.frame_root_failures(), 1);

    let admitted = scheduler
        .schedule(BatchFrameTask { valid: true })
        .expect("failed batch must not consume identities");
    assert_eq!(admitted, SchedulerTaskId::new(1));
    scheduler.shutdown().expect("scheduler shutdown");
}

#[test]
fn rejected_restoration_stops_runtime_without_polling_or_losing_container() {
    let (attempted, observed) = mpsc::channel();
    let scheduler = NativeScheduler::new(configuration(1, 1)).expect("native scheduler");
    scheduler
        .schedule(RejectRestoreTask {
            attempted: Some(attempted),
        })
        .expect("schedule rejection probe");

    observed
        .recv_timeout(Duration::from_secs(1))
        .expect("worker attempted frame restoration");
    let failure_deadline = std::time::Instant::now() + Duration::from_secs(1);
    while scheduler.telemetry().frame_root_failures() == 0 {
        assert!(
            std::time::Instant::now() < failure_deadline,
            "worker did not publish the restoration failure"
        );
        std::thread::yield_now();
    }
    let failed = scheduler.telemetry();
    assert_eq!(failed.polls(), 0);
    assert_eq!(failed.retained_frame_root_containers(), 1);
    assert_eq!(failed.frame_root_failures(), 1);
    assert_eq!(
        scheduler.shutdown(),
        Err(SchedulerError::TaskFrame {
            task: SchedulerTaskId::new(1),
            failure: SchedulerTaskFrameFailure::Restoration,
        })
    );
}

#[test]
fn shutdown_releases_suspended_task_frame_container_exactly_once() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let scheduler = NativeScheduler::new(configuration(1, 1)).expect("native scheduler");
    scheduler
        .schedule(FrameLifecycleTask {
            events,
            map: explicit_empty_frame(904),
            first: true,
        })
        .expect("schedule suspending task");
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("task suspension");
    assert_eq!(scheduler.telemetry().retained_frame_root_containers(), 1);

    let telemetry = scheduler
        .shutdown_with_telemetry()
        .expect("shutdown releases retained roots");
    assert_eq!(telemetry.retained_frame_root_containers(), 0);
    assert_eq!(telemetry.frame_root_releases(), 1);
    assert_eq!(telemetry.frame_root_failures(), 0);
}

#[test]
fn deterministic_scheduler_uses_the_same_explicit_frame_root_lifecycle() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = DeterministicScheduler::recording(configuration(1, 1));
    let task = scheduler
        .schedule(FrameLifecycleTask {
            events: Arc::clone(&events),
            map: explicit_empty_frame(905),
            first: true,
        })
        .expect("schedule deterministic framed task");

    scheduler
        .run_until_idle(1)
        .expect("first deterministic poll");
    assert_eq!(scheduler.telemetry().retained_frame_root_containers(), 1);
    assert!(scheduler.wake(task).expect("wake deterministic task"));
    scheduler
        .run_until_idle(1)
        .expect("second deterministic poll");
    assert_eq!(
        *events.lock().expect("frame event log"),
        ["publish", "restore", "poll", "publish", "restore", "poll"]
    );
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.retained_frame_root_containers(), 0);
    assert_eq!(telemetry.frame_root_retentions(), 2);
    assert_eq!(telemetry.frame_root_restorations(), 2);
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
    assert_eq!(
        bounded.with_dispatch_work_budget(0),
        Err(SchedulerConfigurationError::ZeroDispatchWorkBudget)
    );
}

#[test]
fn work_budget_exhaustion_requeues_at_the_ready_tail_before_suspension() {
    let configuration = configuration(1, 4)
        .with_dispatch_work_budget(2)
        .expect("bounded dispatch work");
    let polls = Arc::new(Mutex::new(Vec::new()));
    let mut scheduler = DeterministicScheduler::recording(configuration);
    let budget = scheduler
        .schedule(BudgetExhaustionTask {
            polls: Arc::clone(&polls),
            first: true,
        })
        .expect("schedule budget task");
    scheduler
        .schedule(OrderedCompleteTask {
            polls: Arc::clone(&polls),
        })
        .expect("schedule peer");

    scheduler
        .run_until_idle(3)
        .expect("budget yield and later suspension");
    assert_eq!(
        *polls.lock().expect("budget poll log"),
        ["budget", "peer", "budget"]
    );
    assert_eq!(
        scheduler.task_state(budget),
        Ok(SchedulerTaskState::Suspended)
    );
    assert_eq!(scheduler.telemetry().work_budget_exhaustions(), 1);
}

#[test]
fn native_work_budget_exhaustion_preserves_peer_progress() {
    let configuration = configuration(1, 4)
        .with_dispatch_work_budget(2)
        .expect("bounded dispatch work");
    let scheduler = NativeScheduler::new(configuration).expect("native scheduler");
    let (started, observed) = mpsc::channel();
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let _gate_cleanup = OpenGateOnDrop(Arc::clone(&gate));
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            BlockingProbeTask {
                started,
                gate: Arc::clone(&gate),
            },
        )
        .expect("occupy worker while publishing peers");
    observed
        .recv_timeout(Duration::from_secs(1))
        .expect("worker occupied");

    let polls = Arc::new(Mutex::new(Vec::new()));
    let budget = scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            BudgetExhaustionTask {
                polls: Arc::clone(&polls),
                first: true,
            },
        )
        .expect("schedule budget task");
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            OrderedCompleteTask {
                polls: Arc::clone(&polls),
            },
        )
        .expect("schedule peer");
    let (open, changed) = &*gate;
    *open.lock().expect("worker gate") = true;
    changed.notify_one();
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("budget task suspends after peer progress");

    assert_eq!(
        *polls.lock().expect("budget poll log"),
        ["budget", "peer", "budget"]
    );
    assert_eq!(
        scheduler.task_state(budget),
        Ok(SchedulerTaskState::Suspended)
    );
    assert_eq!(scheduler.telemetry().work_budget_exhaustions(), 1);
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
    assert_eq!(telemetry.scheduler_migrations(), telemetry.tasks_stolen());
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
    assert_eq!(scheduler.telemetry().scheduler_migrations(), 0);

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
fn failed_migration_transition_closes_scheduler_instead_of_becoming_refusal() {
    let (attempted, observed) = mpsc::channel();
    let transitions = Arc::new(RejectMigrationRuntimeTransitions {
        attempted: Mutex::new(Some(attempted)),
    });
    let scheduler = NativeScheduler::new_with_runtime_transitions(configuration(2, 4), transitions)
        .expect("native scheduler with migration failure");
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
        .recv_timeout(Duration::from_secs(1))
        .expect("owning worker is occupied");
    scheduler
        .schedule_on(
            SchedulerId::new(1),
            SchedulerTaskMobility::Movable,
            CompleteTask {
                completions: Arc::new(Mutex::new(0)),
            },
        )
        .expect("queue migration-failing task");
    let transition = observed
        .recv_timeout(Duration::from_secs(1))
        .expect("migration failure observed");

    let closure_deadline = std::time::Instant::now() + Duration::from_secs(1);
    let closed = loop {
        match scheduler.wake(SchedulerTaskId::new(u64::MAX)) {
            Err(SchedulerError::Closed) => break true,
            Err(SchedulerError::UnknownTask(_)) if std::time::Instant::now() < closure_deadline => {
                std::thread::yield_now();
            }
            Err(SchedulerError::UnknownTask(_)) => break false,
            result => panic!("unexpected scheduler closure probe: {result:?}"),
        }
    };
    let (open, changed) = &*gate;
    *open.lock().expect("probe gate") = true;
    changed.notify_one();
    assert!(
        closed,
        "migration transition failure did not close scheduler"
    );
    assert_eq!(
        scheduler.schedule(CompleteTask {
            completions: Arc::new(Mutex::new(0)),
        }),
        Err(SchedulerError::Closed)
    );
    assert_eq!(
        scheduler.shutdown(),
        Err(SchedulerError::RuntimeTransition {
            transition,
            failure: SchedulerRuntimeTransitionFailure::CollectorState,
        })
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
    let telemetry = scheduler
        .shutdown_with_telemetry()
        .expect("blocking pool shuts down cleanly");
    assert_eq!(telemetry.blocking_submissions(), 2);
    assert_eq!(telemetry.blocking_queue_rejections(), 1);
    assert_eq!(telemetry.blocking_completions(), 2);
    assert_eq!(telemetry.blocking_shutdowns(), 1);
    assert_eq!(telemetry.blocking_shutdown_delay().samples(), 1);
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
    assert!(telemetry.timer_polls() >= 1);
    assert!(telemetry.external_event_polls() >= 1);
    assert_eq!(telemetry.timer_delivery_delay().samples(), 1);
    assert_eq!(telemetry.external_event_delivery_delay().samples(), 1);
    assert!(telemetry.timer_delivery_delay().maximum_work_units() > 0);
    assert!(
        telemetry
            .external_event_delivery_delay()
            .maximum_work_units()
            > 0
    );
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
    let telemetry = scheduler.telemetry();
    assert_eq!(telemetry.timer_polls(), 2);
    assert_eq!(telemetry.timer_delivery_delay().samples(), 1);
    assert!(telemetry.timer_delivery_delay().maximum_work_units() > 0);
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
    assert_eq!(scheduler.telemetry().external_event_polls(), 2);
    assert_eq!(
        scheduler
            .telemetry()
            .external_event_delivery_delay()
            .samples(),
        1
    );
    assert!(
        scheduler
            .telemetry()
            .external_event_delivery_delay()
            .maximum_work_units()
            > 0
    );
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
fn deterministic_telemetry_records_bounded_ready_to_run_percentiles() {
    let mut scheduler = DeterministicScheduler::recording(configuration(1, 4));
    scheduler
        .schedule(ReadyManyTask {
            remaining_ready_polls: 2,
        })
        .expect("multi-poll deterministic task");
    scheduler
        .schedule(CancellationTask)
        .expect("deterministic peer task");

    scheduler
        .run_until_idle(8)
        .expect("collect deterministic ready delays");
    let telemetry = scheduler.telemetry();
    let delay = telemetry.ready_to_run_delay();

    assert_eq!(delay.samples(), telemetry.polls());
    assert!(delay.p50_work_units() <= delay.p95_work_units());
    assert!(delay.p95_work_units() <= delay.p99_work_units());
    assert!(delay.p99_work_units() <= delay.p999_work_units());
    assert!(delay.p999_work_units() <= delay.maximum_work_units());
    assert!(delay.maximum_work_units() > 0);
}

#[test]
fn native_telemetry_records_one_ready_delay_for_every_dispatch() {
    let scheduler = NativeScheduler::new(configuration(2, 8)).expect("native scheduler");
    scheduler
        .schedule(YieldOnceTask { first: true })
        .expect("yielding native task");
    scheduler
        .schedule(CancellationTask)
        .expect("native peer task");
    scheduler
        .wait_until_idle(Duration::from_secs(5))
        .expect("native tasks become idle");

    let telemetry = scheduler
        .shutdown_with_telemetry()
        .expect("clean native shutdown");
    let delay = telemetry.ready_to_run_delay();

    assert_eq!(delay.samples(), telemetry.polls());
    assert!(delay.p50_work_units() <= delay.p999_work_units());
    assert!(delay.p999_work_units() <= delay.maximum_work_units());
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
