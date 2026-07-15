use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::time::Duration;

use pop_runtime_native::{
    NativeCompilerTask, NativeScheduler, NativeTaskFrame, SchedulerConfiguration,
    SchedulerTaskMobility, SchedulerTaskPoll, pop_rt_allocate_object, pop_rt_enter_foreign,
    pop_rt_leave_foreign, pop_rt_release_root, pop_rt_retain_root, request_abi_collection,
};

fn foreign_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("foreign ABI test lock")
}

fn configuration() -> SchedulerConfiguration {
    SchedulerConfiguration::new(1, 1, 8, 8, 8, 1).expect("valid foreign-call scheduler")
}

#[test]
#[allow(unsafe_code)]
fn foreign_transition_requires_a_managed_thread_binding() {
    let _guard = foreign_test_lock();
    let mut roots = [];
    assert_eq!(
        unsafe { pop_rt_enter_foreign(1, roots.as_mut_ptr(), 0, 0) },
        0
    );
}

#[test]
#[allow(unsafe_code)]
fn foreign_transition_balances_exact_roots_and_rejects_reuse() {
    let _guard = foreign_test_lock();
    let observations = Arc::new(Mutex::new(Vec::new()));
    let task_observations = Arc::clone(&observations);
    let frame = NativeTaskFrame::new(
        Vec::new(),
        pop_runtime_interface::SafePointId::new(41),
        Vec::new(),
    )
    .expect("empty compiler frame");
    let task = NativeCompilerTask::new(
        frame,
        move |_: &mut NativeTaskFrame, _: &pop_runtime_native::SchedulerTaskContext| {
            let root = pop_rt_allocate_object(0);
            assert_ne!(root, 0);
            let mut roots = [root];
            let transition = unsafe { pop_rt_enter_foreign(42, roots.as_mut_ptr(), 1, 0) };
            assert_ne!(transition, 0);
            assert_eq!(
                unsafe { pop_rt_leave_foreign(transition, roots.as_mut_ptr(), 0) },
                0
            );
            assert_eq!(
                unsafe { pop_rt_leave_foreign(transition, roots.as_mut_ptr(), 1) },
                1
            );
            assert_eq!(roots, [root]);
            assert_eq!(
                unsafe { pop_rt_leave_foreign(transition, roots.as_mut_ptr(), 1) },
                0
            );

            let bounded = unsafe { pop_rt_enter_foreign(43, roots.as_mut_ptr(), 1, 1) };
            assert_ne!(bounded, 0);
            assert_eq!(
                unsafe { pop_rt_leave_foreign(bounded, roots.as_mut_ptr(), 1) },
                1
            );
            assert_eq!(
                unsafe { pop_rt_enter_foreign(44, roots.as_mut_ptr(), 1, 2) },
                0
            );
            task_observations
                .lock()
                .expect("foreign observations")
                .push(root);
            SchedulerTaskPoll::Complete
        },
    );
    let scheduler = NativeScheduler::new(configuration()).expect("native scheduler");
    scheduler
        .schedule_on(
            pop_runtime_interface::SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            task,
        )
        .expect("schedule foreign-call task");
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("foreign-call task completes");
    assert_eq!(observations.lock().expect("foreign observations").len(), 1);
    scheduler
        .shutdown()
        .expect("foreign-call scheduler shutdown");
}

#[test]
#[allow(unsafe_code)]
fn blocking_foreign_transition_retains_roots_during_collection() {
    let _guard = foreign_test_lock();
    let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
    let (leave_sender, leave_receiver) = mpsc::sync_channel(1);
    let (result_sender, result_receiver) = mpsc::sync_channel(1);
    let frame = NativeTaskFrame::new(
        Vec::new(),
        pop_runtime_interface::SafePointId::new(50),
        Vec::new(),
    )
    .expect("empty compiler frame");
    let task = NativeCompilerTask::new(
        frame,
        move |_: &mut NativeTaskFrame, _: &pop_runtime_native::SchedulerTaskContext| {
            let root = pop_rt_allocate_object(0);
            let mut roots = [root];
            let transition = unsafe { pop_rt_enter_foreign(51, roots.as_mut_ptr(), 1, 0) };
            entered_sender
                .send((root, transition))
                .expect("report entered transition");
            leave_receiver.recv().expect("permit foreign return");
            let left = unsafe { pop_rt_leave_foreign(transition, roots.as_mut_ptr(), 1) } == 1;
            let retained = pop_rt_retain_root(roots[0]);
            let live = retained != 0;
            if live {
                assert_eq!(pop_rt_release_root(retained), 1);
            }
            result_sender
                .send(left && live && roots == [root])
                .expect("report foreign result");
            SchedulerTaskPoll::Complete
        },
    );
    let scheduler = NativeScheduler::new(configuration()).expect("native scheduler");
    scheduler
        .schedule_on(
            pop_runtime_interface::SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            task,
        )
        .expect("schedule blocking foreign-call task");
    let (root, transition) = entered_receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("foreign transition entered");
    assert_ne!(root, 0);
    assert_ne!(transition, 0);
    assert!(request_abi_collection());
    assert_eq!(pop_runtime_native::abi_safe_point(52, &[]), 1);
    leave_sender.send(()).expect("permit foreign leave");
    assert!(
        result_receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("foreign result")
    );
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("foreign-call task completes");
    scheduler
        .shutdown()
        .expect("foreign-call scheduler shutdown");
}
