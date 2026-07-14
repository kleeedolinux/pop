use pop_runtime_collector::{
    AllocationInfrastructureConfig, GenerationalMemoryConfig, GenerationalRuntime,
    MajorCollectorConfig, MajorCyclePhase,
};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RootPublication, RootSlot, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn object(slots: u32, references: &[u32]) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(41),
        AllocationClass::Mature,
        ObjectMap::new(
            slots,
            references.iter().copied().map(ObjectSlot::new).collect(),
        )
        .expect("object map"),
    )
}

fn no_stack_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

fn one_stack_root(id: u32, reference: ManagedReference) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), vec![RootSlot::new(0)]).expect("stack map"),
        vec![Some(reference)],
    )
    .expect("root publication")
}

fn finish_major(runtime: &mut GenerationalRuntime, roots: &mut RootPublication) {
    for _ in 0..128 {
        if runtime
            .safe_point(roots)
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
fn mature_marking_preserves_rooted_cycles_and_sweeps_after_release() {
    let mut runtime = GenerationalRuntime::new();
    let request = object(1, &[0]);
    let left = runtime.allocate_object(&request).expect("left");
    let right = runtime.allocate_object(&request).expect("right");
    runtime
        .store_reference(left, ObjectSlot::new(0), Some(right))
        .expect("left edge");
    runtime
        .store_reference(right, ObjectSlot::new(0), Some(left))
        .expect("right edge");
    let root = runtime.retain_root(left).expect("root");
    let mut roots = no_stack_roots(1);

    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(runtime.contains(left));
    assert!(runtime.contains(right));

    runtime.release_root(root).expect("release root");
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(!runtime.contains(left));
    assert!(!runtime.contains(right));
}

#[test]
fn satb_overwrite_keeps_the_snapshot_target_alive_for_the_active_cycle() {
    let mut runtime = GenerationalRuntime::new();
    let parent = runtime.allocate_object(&object(1, &[0])).expect("parent");
    let child = runtime.allocate_object(&object(0, &[])).expect("child");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("parent edge");
    let root = runtime.retain_root(parent).expect("root");
    let mut roots = no_stack_roots(2);

    runtime
        .start_major_collection(&roots)
        .expect("start snapshot");
    runtime
        .store_reference(parent, ObjectSlot::new(0), None)
        .expect("overwrite snapshot edge");
    finish_major(&mut runtime, &mut roots);
    assert!(runtime.contains(child));

    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(!runtime.contains(child));
    assert!(runtime.contains(parent));
    runtime.release_root(root).expect("release root");
}

#[test]
fn new_mature_allocations_are_live_during_an_active_snapshot() {
    let mut runtime = GenerationalRuntime::new();
    let mut roots = no_stack_roots(3);
    runtime
        .start_major_collection(&roots)
        .expect("start snapshot");

    let allocated = runtime
        .allocate_object(&object(0, &[]))
        .expect("allocate during marking");
    finish_major(&mut runtime, &mut roots);

    assert!(runtime.contains(allocated));
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(!runtime.contains(allocated));
}

#[test]
fn post_scan_initialization_of_a_new_mature_object_shades_its_target() {
    let mut runtime = GenerationalRuntime::with_config(MajorCollectorConfig::new(1));
    let target = runtime.allocate_object(&object(0, &[])).expect("target");
    let mut roots = no_stack_roots(30);
    runtime
        .start_major_collection(&roots)
        .expect("start snapshot");
    let holder = runtime
        .allocate_object(&object(1, &[0]))
        .expect("new holder");

    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("scan new holder")
            .collection()
            .is_none()
    );
    runtime
        .store_reference(holder, ObjectSlot::new(0), Some(target))
        .expect("initialize scanned holder");
    finish_major(&mut runtime, &mut roots);

    assert!(runtime.contains(holder));
    assert!(runtime.contains(target));
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(!runtime.contains(holder));
    assert!(!runtime.contains(target));
}

#[test]
fn major_work_is_bounded_per_safe_point() {
    let mut runtime = GenerationalRuntime::with_config(MajorCollectorConfig::new(1));
    let request = object(1, &[0]);
    let first = runtime.allocate_object(&request).expect("first");
    let second = runtime.allocate_object(&request).expect("second");
    let third = runtime.allocate_object(&object(0, &[])).expect("third");
    runtime
        .store_reference(first, ObjectSlot::new(0), Some(second))
        .expect("first edge");
    runtime
        .store_reference(second, ObjectSlot::new(0), Some(third))
        .expect("second edge");
    let root = runtime.retain_root(first).expect("root");
    let mut roots = no_stack_roots(4);

    runtime.request_major_collection();
    let outcome = runtime.safe_point(&mut roots).expect("first bounded slice");

    assert!(outcome.collection().is_none());
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Marking);
    assert!(runtime.major_cycle_active());
    finish_major(&mut runtime, &mut roots);
    runtime.release_root(root).expect("release root");
}

#[test]
fn stale_snapshot_roots_fail_without_mutating_the_heap() {
    let mut runtime = GenerationalRuntime::new();
    let invalid = ManagedReference::new(9_999);
    let roots = RootPublication::new(
        StackMap::new(
            SafePointId::new(5),
            vec![pop_runtime_interface::RootSlot::new(0)],
        )
        .expect("stack map"),
        vec![Some(invalid)],
    )
    .expect("root publication");

    assert!(runtime.start_major_collection(&roots).is_err());
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Idle);
    assert_eq!(runtime.object_count(), 0);
}

#[test]
fn minor_relocation_waits_until_an_active_major_snapshot_finishes() {
    let mut runtime = GenerationalRuntime::with_config(MajorCollectorConfig::new(1));
    let young = runtime
        .allocate_object(&ObjectAllocationRequest::new(
            RuntimeTypeId::new(42),
            AllocationClass::NurseryEligible,
            ObjectMap::new(0, Vec::new()).expect("young map"),
        ))
        .expect("young object");
    let mut roots = one_stack_root(6, young);
    runtime
        .start_major_collection(&roots)
        .expect("start major snapshot");
    runtime.request_minor_collection();

    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("major slice")
            .collection()
            .is_none()
    );
    assert_eq!(roots.managed_references().next(), Some(young));
    finish_major(&mut runtime, &mut roots);
    assert_eq!(roots.managed_references().next(), Some(young));

    let outcome = runtime.safe_point(&mut roots).expect("deferred minor");
    assert!(outcome.collection().is_some());
    assert_ne!(roots.managed_references().next(), Some(young));
}

#[test]
fn roots_and_pins_created_during_marking_are_shaded() {
    let mut runtime = GenerationalRuntime::new();
    let rooted = runtime.allocate_object(&object(0, &[])).expect("rooted");
    let young = runtime
        .allocate_object(&ObjectAllocationRequest::new(
            RuntimeTypeId::new(43),
            AllocationClass::NurseryEligible,
            ObjectMap::new(0, Vec::new()).expect("young map"),
        ))
        .expect("young");
    let mut roots = no_stack_roots(7);
    runtime
        .start_major_collection(&roots)
        .expect("start snapshot");

    let root = runtime.retain_root(rooted).expect("late root");
    let pin = runtime.pin(young).expect("late pin");
    finish_major(&mut runtime, &mut roots);

    assert!(runtime.contains(rooted));
    assert!(runtime.contains(young));
    runtime.release_root(root).expect("release root");
    runtime.unpin(pin).expect("release pin");
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(!runtime.contains(rooted));
    assert!(!runtime.contains(young));
}

#[test]
fn pin_admission_does_not_assist_before_registering_a_mature_target() {
    let mut runtime = GenerationalRuntime::with_memory_config(
        MajorCollectorConfig::new(1),
        AllocationInfrastructureConfig::new(64, 256, 32).expect("allocation geometry"),
        GenerationalMemoryConfig::new(512, 32, 64, 64, 100, 1).expect("memory configuration"),
    );
    let mature = runtime.allocate_object(&object(0, &[])).expect("mature");
    let roots = no_stack_roots(8);
    runtime
        .start_major_collection(&roots)
        .expect("start snapshot without the late pin");

    let pin = runtime
        .pin(mature)
        .expect("pin registration precedes any pressure assist");
    let mut roots = no_stack_roots(9);
    finish_major(&mut runtime, &mut roots);
    assert!(runtime.contains(mature));

    runtime.unpin(pin).expect("release pin");
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(!runtime.contains(mature));
}
