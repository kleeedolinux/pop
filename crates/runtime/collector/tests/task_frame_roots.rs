use pop_runtime_collector::{
    GenerationalRuntime, SchedulerId, StableGenerationalRuntime, TaskFrameRootConfig,
    TaskFrameRootConfigError, TaskFrameRootError,
};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RootPublication, RootSlot, RuntimeAdapter, RuntimeFailure, RuntimeTypeId, SafePointId,
    StackMap,
};

fn object(class: AllocationClass, slots: u32, references: &[u32]) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(73),
        class,
        ObjectMap::new(
            slots,
            references.iter().copied().map(ObjectSlot::new).collect(),
        )
        .expect("object map"),
    )
}

fn stack_map(id: u32, slots: &[u32]) -> StackMap {
    StackMap::new(
        SafePointId::new(id),
        slots.iter().copied().map(RootSlot::new).collect(),
    )
    .expect("stack map")
}

fn publication(id: u32, slots: &[u32], values: Vec<Option<ManagedReference>>) -> RootPublication {
    RootPublication::new(stack_map(id, slots), values).expect("root publication")
}

fn no_stack_roots(id: u32) -> RootPublication {
    publication(id, &[], Vec::new())
}

fn force_minor(runtime: &mut GenerationalRuntime, id: u32) {
    let mut roots = no_stack_roots(id);
    runtime.request_minor_collection();
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("minor collection")
            .collection()
            .is_some()
    );
}

fn finish_major(runtime: &mut GenerationalRuntime, id: u32) {
    let mut roots = no_stack_roots(id);
    runtime.request_major_collection();
    for _ in 0..128 {
        if runtime
            .safe_point(&mut roots)
            .expect("major collection slice")
            .collection()
            .is_some()
        {
            return;
        }
    }
    panic!("major collection did not finish within its deterministic work bound");
}

#[test]
fn retained_ready_or_suspended_frame_survives_minor_and_restores_relocated_slots() {
    let scheduler = SchedulerId::new(1);
    let mut runtime = GenerationalRuntime::new();
    let parent = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 1, &[0]))
        .expect("parent");
    let child = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("child");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("parent edge");
    let map = stack_map(10, &[2, 8]);
    let frame = runtime
        .retain_task_frame_roots(
            scheduler,
            &RootPublication::new(map.clone(), vec![Some(parent), None])
                .expect("frame publication"),
        )
        .expect("retain frame roots");

    force_minor(&mut runtime, 11);

    assert!(!runtime.contains(parent));
    assert!(!runtime.contains(child));
    assert_eq!(runtime.object_count(), 2);
    let restored = runtime
        .restore_task_frame_roots(frame, scheduler, &map)
        .expect("restore relocated roots");
    let values: Vec<_> = restored.root_values().collect();
    let relocated_parent = values[0].1.expect("relocated parent");
    assert_eq!(values[0].0, RootSlot::new(2));
    assert_eq!(values[1], (RootSlot::new(8), None));
    assert_ne!(relocated_parent, parent);
    assert!(runtime.contains(relocated_parent));
    assert!(
        runtime
            .load_reference(relocated_parent, ObjectSlot::new(0))
            .expect("relocated edge")
            .is_some()
    );

    let telemetry = runtime.task_frame_root_telemetry();
    assert_eq!(telemetry.current_containers(), 0);
    assert_eq!(telemetry.current_slots(), 0);
    assert_eq!(telemetry.containers_retained(), 1);
    assert_eq!(telemetry.containers_restored(), 1);
    assert_eq!(telemetry.maximum_containers(), 1);
    assert_eq!(telemetry.maximum_slots(), 2);
}

#[test]
fn retained_mature_graph_survives_major_until_explicit_release() {
    let scheduler = SchedulerId::new(1);
    let mut runtime = GenerationalRuntime::new();
    let parent = runtime
        .allocate_object(&object(AllocationClass::Mature, 1, &[0]))
        .expect("parent");
    let child = runtime
        .allocate_object(&object(AllocationClass::Mature, 0, &[]))
        .expect("child");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("parent edge");
    let map = stack_map(20, &[0]);
    let frame = runtime
        .retain_task_frame_roots(
            scheduler,
            &RootPublication::new(map, vec![Some(parent)]).expect("frame publication"),
        )
        .expect("retain frame roots");

    finish_major(&mut runtime, 21);
    assert!(runtime.contains(parent));
    assert!(runtime.contains(child));

    runtime
        .release_task_frame_roots(frame, scheduler)
        .expect("release frame roots");
    finish_major(&mut runtime, 22);
    assert!(!runtime.contains(parent));
    assert!(!runtime.contains(child));
    assert_eq!(runtime.task_frame_root_telemetry().containers_released(), 1);
}

#[test]
fn bounded_admission_and_invalid_roots_fail_without_partial_retention() {
    let scheduler = SchedulerId::new(1);
    let config = TaskFrameRootConfig::new(1, 2).expect("bounded config");
    let mut runtime = GenerationalRuntime::with_task_frame_root_config(config);
    let stale = ManagedReference::new(u64::MAX);

    assert_eq!(
        runtime.retain_task_frame_roots(scheduler, &publication(30, &[0], vec![Some(stale)]),),
        Err(TaskFrameRootError::Runtime(
            RuntimeFailure::runtime_invariant()
        ))
    );
    assert_eq!(runtime.task_frame_root_telemetry().current_containers(), 0);
    assert_eq!(runtime.task_frame_root_telemetry().current_slots(), 0);

    let retained = runtime
        .retain_task_frame_roots(scheduler, &publication(31, &[1, 4], vec![None, None]))
        .expect("explicit empty slots still retain exact shape");
    assert_eq!(
        runtime.retain_task_frame_roots(scheduler, &no_stack_roots(32)),
        Err(TaskFrameRootError::ContainerCapacityExceeded { maximum: 1 })
    );
    assert_eq!(
        runtime.retain_task_frame_roots(
            scheduler,
            &publication(33, &[0, 1, 2], vec![None, None, None]),
        ),
        Err(TaskFrameRootError::SlotCapacityExceeded { maximum: 2 })
    );
    assert_eq!(runtime.task_frame_root_telemetry().current_containers(), 1);
    assert_eq!(runtime.task_frame_root_telemetry().current_slots(), 2);

    runtime
        .release_task_frame_roots(retained, scheduler)
        .expect("release retained frame");
}

#[test]
fn admission_rejects_foreign_scheduler_local_roots_without_a_container() {
    let first = SchedulerId::new(1);
    let second = SchedulerId::new(2);
    let mut runtime = GenerationalRuntime::new();
    runtime.select_scheduler(second);
    let foreign = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("second scheduler value");

    assert_eq!(
        runtime.retain_task_frame_roots(first, &publication(35, &[0], vec![Some(foreign)]),),
        Err(TaskFrameRootError::Runtime(
            RuntimeFailure::runtime_invariant()
        ))
    );
    assert_eq!(runtime.task_frame_root_telemetry().current_containers(), 0);
    assert_eq!(runtime.task_frame_root_telemetry().current_slots(), 0);
}

#[test]
fn restore_mismatch_and_duplicate_restore_fail_closed_without_losing_container() {
    let scheduler = SchedulerId::new(4);
    let other = SchedulerId::new(5);
    let mut runtime = GenerationalRuntime::new();
    let exact = stack_map(40, &[3]);
    let frame = runtime
        .retain_task_frame_roots(
            scheduler,
            &RootPublication::new(exact.clone(), vec![None]).expect("publication"),
        )
        .expect("retain roots");

    assert_eq!(
        runtime.restore_task_frame_roots(frame, other, &exact),
        Err(TaskFrameRootError::SchedulerMismatch {
            expected: scheduler,
            found: other,
        })
    );
    assert_eq!(
        runtime.restore_task_frame_roots(frame, scheduler, &stack_map(41, &[3])),
        Err(TaskFrameRootError::StackMapMismatch)
    );
    assert_eq!(runtime.task_frame_root_telemetry().current_containers(), 1);
    runtime
        .restore_task_frame_roots(frame, scheduler, &exact)
        .expect("exact restore remains available");
    assert_eq!(
        runtime.restore_task_frame_roots(frame, scheduler, &exact),
        Err(TaskFrameRootError::UnknownContainer(frame))
    );
}

#[test]
fn migration_changes_exact_container_owner_once_and_refusal_preserves_source() {
    let source = SchedulerId::new(8);
    let destination = SchedulerId::new(9);
    let wrong = SchedulerId::new(10);
    let mut runtime = GenerationalRuntime::new();
    let map = stack_map(50, &[]);
    let frame = runtime
        .retain_task_frame_roots(source, &no_stack_roots(50))
        .expect("retain explicit empty frame");

    assert_eq!(
        runtime.transfer_task_frame_roots(frame, wrong, destination),
        Err(TaskFrameRootError::SchedulerMismatch {
            expected: source,
            found: wrong,
        })
    );
    assert_eq!(runtime.task_frame_root_telemetry().transfers_refused(), 1);
    runtime
        .transfer_task_frame_roots(frame, source, destination)
        .expect("transfer exact container");
    assert_eq!(runtime.task_frame_root_telemetry().transfers_completed(), 1);
    assert_eq!(
        runtime.restore_task_frame_roots(frame, source, &map),
        Err(TaskFrameRootError::SchedulerMismatch {
            expected: destination,
            found: source,
        })
    );
    runtime
        .restore_task_frame_roots(frame, destination, &map)
        .expect("destination restores transferred frame");
}

#[test]
fn migration_refuses_scheduler_local_roots_but_accepts_explicit_shared_graphs() {
    let source = SchedulerId::new(1);
    let destination = SchedulerId::new(2);
    let mut runtime = GenerationalRuntime::new();
    let local = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("scheduler-local value");
    let map = stack_map(55, &[0]);
    let local_frame = runtime
        .retain_task_frame_roots(
            source,
            &RootPublication::new(map.clone(), vec![Some(local)]).expect("local frame"),
        )
        .expect("retain local frame");

    assert_eq!(
        runtime.transfer_task_frame_roots(local_frame, source, destination),
        Err(TaskFrameRootError::Runtime(
            RuntimeFailure::runtime_invariant()
        ))
    );
    runtime
        .restore_task_frame_roots(local_frame, source, &map)
        .expect("refusal preserves source container");

    runtime
        .publish_shared(local)
        .expect("publish complete graph");
    let shared_frame = runtime
        .retain_task_frame_roots(
            source,
            &RootPublication::new(map.clone(), vec![Some(local)]).expect("shared frame"),
        )
        .expect("retain shared frame");
    runtime
        .transfer_task_frame_roots(shared_frame, source, destination)
        .expect("transfer shared frame");
    runtime
        .restore_task_frame_roots(shared_frame, destination, &map)
        .expect("restore shared frame at destination");
}

#[test]
fn configuration_rejects_zero_bounds() {
    assert_eq!(
        TaskFrameRootConfig::new(0, 1),
        Err(TaskFrameRootConfigError::ZeroMaximumContainers)
    );
    assert_eq!(
        TaskFrameRootConfig::new(1, 0),
        Err(TaskFrameRootConfigError::ZeroMaximumSlots)
    );
}

#[test]
fn stable_native_facade_retains_and_restores_the_same_exact_frame_contract() {
    let scheduler = SchedulerId::new(1);
    let config = TaskFrameRootConfig::new(2, 4).expect("bounded config");
    let mut runtime = StableGenerationalRuntime::with_task_frame_root_config(config);
    let value = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("stable native value");
    let map = stack_map(60, &[6]);
    let frame = runtime
        .retain_task_frame_roots(
            scheduler,
            &RootPublication::new(map.clone(), vec![Some(value)]).expect("publication"),
        )
        .expect("retain stable frame");

    runtime.request_collection();
    let mut no_roots = no_stack_roots(61);
    let mut collection_finished = false;
    for _ in 0..128 {
        if runtime
            .safe_point(&mut no_roots)
            .expect("stable major slice")
            .collection()
            .is_some()
        {
            collection_finished = true;
            break;
        }
    }
    assert!(collection_finished, "stable major collection must finish");
    let restored = runtime
        .restore_task_frame_roots(frame, scheduler, &map)
        .expect("restore stable frame");
    assert_eq!(restored.managed_references().next(), Some(value));
    assert_eq!(runtime.task_frame_root_telemetry().containers_restored(), 1);
}
