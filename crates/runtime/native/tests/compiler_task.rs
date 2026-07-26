use std::sync::{Arc, Mutex};
use std::time::Duration;

use pop_runtime_collector::GenerationalRuntime;
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, RootPublication,
    RootSlot, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};
use pop_runtime_native::{
    NativeCompilerTask, NativeScheduler, NativeTaskFrame, NativeTaskFrameError,
    SchedulerConfiguration, SchedulerTaskContext, SchedulerTaskFrame, SchedulerTaskMobility,
    SchedulerTaskPoll, SchedulerTaskState,
};

fn configuration() -> SchedulerConfiguration {
    SchedulerConfiguration::new(1, 1, 8, 8, 8, 1).expect("valid compiler-task scheduler")
}

#[test]
fn compiler_frame_publishes_and_installs_exact_relocated_root_slots() {
    let frame = NativeTaskFrame::new(
        vec![41, 700, 99],
        SafePointId::new(11),
        vec![RootSlot::new(1)],
    )
    .expect("verified compiler frame");
    let mut task = NativeCompilerTask::new(
        frame,
        |_: &mut NativeTaskFrame, _: &SchedulerTaskContext| SchedulerTaskPoll::Complete,
    );

    let mut publication = task.publish_frame_roots().expect("publish exact root");
    assert_eq!(
        publication.managed_references().collect::<Vec<_>>(),
        [ManagedReference::new(700)]
    );
    let (_, relocated) = publication
        .root_values_mut()
        .next()
        .expect("one writable root");
    *relocated = Some(ManagedReference::new(701));
    task.restore_frame_roots(publication)
        .expect("install relocated root");

    assert_eq!(task.frame().slot(1), Ok(701));
    assert_eq!(task.frame().slot(0), Ok(41));
    assert_eq!(
        task.frame().slot(3),
        Err(NativeTaskFrameError::UnknownSlot(3))
    );
}

#[test]
fn abi_two_compiler_coroutine_frame_reloads_a_forced_relocated_root() {
    let scheduler = pop_runtime_interface::SchedulerId::new(1);
    let mut runtime = GenerationalRuntime::new();
    let original = runtime
        .allocate_object(&ObjectAllocationRequest::new(
            RuntimeTypeId::new(91),
            AllocationClass::NurseryEligible,
            ObjectMap::scalar(0),
        ))
        .expect("nursery object");
    let frame = NativeTaskFrame::new(
        vec![0, original.raw()],
        SafePointId::new(12),
        vec![RootSlot::new(1)],
    )
    .expect("verified compiler frame");
    let mut task = NativeCompilerTask::new(
        frame,
        |_: &mut NativeTaskFrame, _: &SchedulerTaskContext| SchedulerTaskPoll::Complete,
    );
    let publication = task.publish_frame_roots().expect("publish coroutine root");
    let retained = runtime
        .retain_task_frame_roots(scheduler, &publication)
        .expect("retain suspended coroutine frame");

    runtime.request_minor_collection();
    let mut no_stack_roots = RootPublication::new(
        StackMap::new(SafePointId::new(13), Vec::new()).expect("empty stack map"),
        Vec::new(),
    )
    .expect("empty root publication");
    assert!(
        runtime
            .safe_point(&mut no_stack_roots)
            .expect("forced nursery relocation")
            .collection()
            .is_some()
    );
    assert!(
        !runtime.contains(original),
        "the physical pre-suspension token must become stale"
    );

    let restored = runtime
        .restore_task_frame_roots(retained, scheduler, task.frame().stack_map())
        .expect("restore relocated coroutine roots");
    task.restore_frame_roots(restored)
        .expect("install relocated compiler-frame token");
    let relocated = ManagedReference::new(task.frame().slot(1).expect("restored root slot"));
    assert_ne!(relocated, original);
    assert!(runtime.contains(relocated));
}

#[test]
fn compiler_task_changes_to_the_next_exact_live_map_before_native_suspension() {
    let events = Arc::new(Mutex::new(Vec::new()));
    let poll_events = Arc::clone(&events);
    let frame = NativeTaskFrame::new(vec![0, 0], SafePointId::new(20), Vec::new())
        .expect("initial ready frame");
    let mut first = true;
    let task = NativeCompilerTask::new(
        frame,
        move |frame: &mut NativeTaskFrame, _: &SchedulerTaskContext| {
            if first {
                first = false;
                frame.set_slot(0, 42).expect("store live scalar");
                frame
                    .set_live_frame(SafePointId::new(21), vec![RootSlot::new(1)])
                    .expect("install suspend-state map");
                poll_events.lock().expect("poll event log").push("suspend");
                SchedulerTaskPoll::Pending
            } else {
                assert_eq!(frame.safe_point(), SafePointId::new(21));
                assert_eq!(frame.slot(0), Ok(42));
                poll_events.lock().expect("poll event log").push("resume");
                SchedulerTaskPoll::Complete
            }
        },
    );
    let scheduler = NativeScheduler::new(configuration()).expect("native scheduler");
    let task = scheduler
        .schedule_on(
            pop_runtime_interface::SchedulerId::new(1),
            SchedulerTaskMobility::Affine,
            task,
        )
        .expect("schedule compiler-created frame");

    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("compiler task suspends");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Suspended)
    );
    assert_eq!(scheduler.telemetry().retained_frame_root_containers(), 1);
    assert!(scheduler.wake(task).expect("wake compiler task"));
    scheduler
        .wait_until_idle(Duration::from_secs(1))
        .expect("compiler task completes");
    assert_eq!(
        scheduler.task_state(task),
        Ok(SchedulerTaskState::Completed)
    );
    assert_eq!(
        *events.lock().expect("poll event log"),
        ["suspend", "resume"]
    );
    let telemetry = scheduler
        .shutdown_with_telemetry()
        .expect("compiler task scheduler shutdown");
    assert_eq!(telemetry.retained_frame_root_containers(), 0);
    assert_eq!(telemetry.frame_root_failures(), 0);
}

#[test]
fn compiler_frame_rejects_duplicate_and_out_of_bounds_live_roots() {
    assert_eq!(
        NativeTaskFrame::new(
            vec![0],
            SafePointId::new(30),
            vec![RootSlot::new(0), RootSlot::new(0)],
        ),
        Err(NativeTaskFrameError::InvalidRootMap)
    );
    assert_eq!(
        NativeTaskFrame::new(vec![0], SafePointId::new(31), vec![RootSlot::new(1)],),
        Err(NativeTaskFrameError::RootOutOfBounds(RootSlot::new(1)))
    );
}
