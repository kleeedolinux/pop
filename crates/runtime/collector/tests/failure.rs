use pop_runtime_collector::{BootstrapRuntime, HeapLimits};
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, PanicKind, RootPublication,
    RuntimeAdapter, RuntimeFailure, RuntimeTypeId, SafePointId, StackMap, UnwindReason,
};

fn empty_object() -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(1),
        AllocationClass::NurseryEligible,
        ObjectMap::new(0, Vec::new()).expect("empty object map"),
    )
}

fn no_roots() -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(0), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

#[test]
fn out_of_memory_is_a_deterministic_panic_unwind_after_collection_cannot_progress() {
    let mut runtime = BootstrapRuntime::with_limits(HeapLimits::new(1, 0));
    let live = runtime
        .allocate_object(&empty_object())
        .expect("first object");
    let root = runtime.retain_root(live).expect("root live object");

    let first = runtime
        .allocate_object(&empty_object())
        .expect_err("rooted heap is at its object limit");
    let second = runtime
        .allocate_object(&empty_object())
        .expect_err("same state produces the same failure");
    assert_eq!(first, second);
    assert_eq!(
        first,
        RuntimeFailure::Unwind(UnwindReason::Panic(
            pop_runtime_interface::PanicPayload::new(PanicKind::OutOfMemory {
                requested_objects: 1,
                requested_slots: 0,
            })
        ))
    );
    assert!(runtime.contains(live));

    runtime.release_root(root).expect("release live root");
    runtime.collect(&no_roots()).expect("reclaim live object");
    assert!(runtime.allocate_object(&empty_object()).is_ok());
}

#[test]
fn invalid_references_fail_as_runtime_invariant_panics_not_host_panics() {
    let mut runtime = BootstrapRuntime::new();
    let invalid = pop_runtime_interface::ManagedReference::new(999);
    let failure = runtime.retain_root(invalid).expect_err("invalid reference");

    assert!(matches!(
        failure,
        RuntimeFailure::Unwind(UnwindReason::Panic(payload))
            if payload.kind() == PanicKind::RuntimeInvariant
    ));
}
