use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, GarbageCollectorStage,
    ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot, RootPublication, RootSlot,
    RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};
use pop_runtime_native::{BootstrapRuntime, HeapLimits};

fn object(type_id: u32, slots: u32, references: &[u32]) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(type_id),
        AllocationClass::NurseryEligible,
        ObjectMap::new(
            slots,
            references.iter().copied().map(ObjectSlot::new).collect(),
        )
        .expect("object map"),
    )
}

fn no_stack_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("empty stack map"),
        Vec::new(),
    )
    .expect("empty root publication")
}

#[test]
fn precise_tracing_preserves_transitive_cycles_and_reclaims_them_after_root_release() {
    let mut runtime = BootstrapRuntime::new();
    assert_eq!(
        runtime.contract().stage(),
        GarbageCollectorStage::BootstrapPreciseStopTheWorld
    );
    let first = runtime.allocate_object(&object(1, 1, &[0])).expect("first");
    let second = runtime
        .allocate_object(&object(1, 1, &[0]))
        .expect("second");
    runtime
        .store_reference(first, ObjectSlot::new(0), Some(second))
        .expect("first to second");
    runtime
        .store_reference(second, ObjectSlot::new(0), Some(first))
        .expect("second to first");
    let root = runtime.retain_root(first).expect("root");

    let live = runtime
        .collect(&no_stack_roots(1))
        .expect("rooted collection");
    assert_eq!(live.live_objects(), 2);
    assert_eq!(live.reclaimed_objects(), 0);
    assert!(runtime.contains(first));
    assert!(runtime.contains(second));

    runtime.release_root(root).expect("release root");
    let reclaimed = runtime
        .collect(&no_stack_roots(2))
        .expect("unrooted collection");
    assert_eq!(reclaimed.reclaimed_objects(), 2);
    assert!(!runtime.contains(first));
    assert!(!runtime.contains(second));
}

#[test]
fn pins_remain_precise_strong_roots_until_each_handle_is_released() {
    let mut runtime = BootstrapRuntime::new();
    let target = runtime.allocate_object(&object(8, 0, &[])).expect("target");
    let first = runtime.pin(target).expect("first pin");
    let second = runtime.pin(target).expect("second pin");

    runtime
        .collect(&no_stack_roots(20))
        .expect("pinned collection");
    assert!(runtime.contains(target));

    runtime.unpin(first).expect("release first pin");
    runtime
        .collect(&no_stack_roots(21))
        .expect("remaining pin collection");
    assert!(runtime.contains(target));

    runtime.unpin(second).expect("release second pin");
    runtime
        .collect(&no_stack_roots(22))
        .expect("unpin collection");
    assert!(!runtime.contains(target));
    assert!(runtime.unpin(second).is_err());
}

#[test]
fn reference_arrays_trace_elements_and_scalar_slots_never_become_conservative_roots() {
    let mut runtime = BootstrapRuntime::new();
    let child = runtime.allocate_object(&object(2, 0, &[])).expect("child");
    let array = runtime
        .allocate_array(&ArrayAllocationRequest::new(
            RuntimeTypeId::new(3),
            AllocationClass::NurseryEligible,
            2,
            ArrayElementMap::ManagedReference,
        ))
        .expect("array");
    runtime
        .store_reference(array, ObjectSlot::new(1), Some(child))
        .expect("array element");
    let array_root = runtime.retain_root(array).expect("array root");

    runtime.collect(&no_stack_roots(3)).expect("array trace");
    assert!(runtime.contains(child));

    runtime
        .store_reference(array, ObjectSlot::new(1), None)
        .expect("clear element");
    runtime.collect(&no_stack_roots(4)).expect("clear trace");
    assert!(!runtime.contains(child));
    runtime.release_root(array_root).expect("release array");

    let target = runtime.allocate_object(&object(4, 0, &[])).expect("target");
    let scalar_owner = runtime
        .allocate_object(&object(5, 1, &[]))
        .expect("scalar owner");
    runtime
        .store_scalar(scalar_owner, ObjectSlot::new(0), target.raw())
        .expect("scalar resembling handle");
    let scalar_root = runtime.retain_root(scalar_owner).expect("scalar root");

    runtime.collect(&no_stack_roots(5)).expect("precise trace");
    assert!(runtime.contains(scalar_owner));
    assert!(!runtime.contains(target));
    runtime
        .release_root(scalar_root)
        .expect("release scalar owner");
}

#[test]
fn requested_collection_runs_at_a_safe_point_with_published_stack_roots() {
    let mut runtime = BootstrapRuntime::new();
    let stack_live = runtime
        .allocate_object(&object(6, 0, &[]))
        .expect("stack live");
    let stack_map = StackMap::new(SafePointId::new(10), vec![RootSlot::new(0)]).expect("stack map");
    let roots = RootPublication::new(stack_map, vec![Some(stack_live)]).expect("roots");

    runtime.request_collection();
    let outcome = RuntimeAdapter::safe_point(&mut runtime, &roots).expect("safe point");
    assert!(outcome.collection().is_some());
    assert!(runtime.contains(stack_live));

    runtime.request_collection();
    let outcome =
        RuntimeAdapter::safe_point(&mut runtime, &no_stack_roots(11)).expect("second safe point");
    assert_eq!(
        outcome
            .collection()
            .expect("forced collection")
            .reclaimed_objects(),
        1
    );
    assert!(!runtime.contains(stack_live));
}

#[test]
fn heap_limits_are_explicit_in_the_bootstrap_runtime() {
    let runtime = BootstrapRuntime::with_limits(HeapLimits::new(7, 32));
    assert_eq!(runtime.limits(), HeapLimits::new(7, 32));
    assert_eq!(runtime.object_count(), 0);
    assert_eq!(runtime.slot_count(), 0);
    let _: Option<ManagedReference> = None;
}
