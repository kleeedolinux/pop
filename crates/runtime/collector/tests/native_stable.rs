use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, GarbageCollectorStage,
    ObjectAllocationRequest, ObjectMap, RootPublication, RuntimeAdapter, RuntimeTypeId,
    SafePointId, StackMap,
};

fn empty_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

fn nursery_eligible_object() -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(1),
        AllocationClass::NurseryEligible,
        ObjectMap::new(0, Vec::new()).expect("object map"),
    )
}

#[test]
fn native_stable_composition_never_places_abi_one_tokens_in_the_nursery() {
    let mut runtime = StableGenerationalRuntime::new();
    let reference = runtime
        .allocate_object(&nursery_eligible_object())
        .expect("stable native allocation");

    assert_eq!(
        runtime.contract().stage(),
        GarbageCollectorStage::NativeStableGenerationalConformance
    );
    assert!(!runtime.contract().moving_nursery());
    assert_eq!(
        runtime.allocation_class(reference),
        Some(AllocationClass::Mature)
    );
}

#[test]
fn native_stable_composition_reclaims_unreachable_mature_objects() {
    let mut runtime = StableGenerationalRuntime::new();
    let reference = runtime
        .allocate_object(&nursery_eligible_object())
        .expect("stable native allocation");
    runtime.request_collection();
    runtime
        .safe_point(&mut empty_roots(1))
        .expect("mature collection");

    assert!(!runtime.contains(reference));
}

#[test]
fn native_stable_major_sweep_reclaims_pages_in_one_batch() {
    let mut runtime = StableGenerationalRuntime::new();
    for _ in 0..64 {
        runtime
            .allocate_object(&nursery_eligible_object())
            .expect("stable native allocation");
    }
    let passes_before = runtime.allocation_metrics().page_reclamation_passes();

    runtime.request_collection();
    let mut roots = empty_roots(3);
    for _ in 0..4 {
        if !runtime.collection_requested() {
            break;
        }
        runtime.safe_point(&mut roots).expect("mature collection");
    }
    assert!(!runtime.collection_requested());

    assert_eq!(
        runtime
            .allocation_metrics()
            .page_reclamation_passes()
            .saturating_sub(passes_before),
        1
    );
}

#[test]
fn native_allocation_churn_keeps_reclamation_and_page_growth_bounded() {
    let mut runtime = StableGenerationalRuntime::new();
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(2),
        AllocationClass::Mature,
        256,
        ArrayElementMap::Scalar,
    );
    let mut roots = empty_roots(2);
    for index in 1..=20_000_u64 {
        runtime
            .allocate_array_filled(&request, index)
            .expect("churn allocation");
        if index.is_multiple_of(8_192) {
            runtime.safe_point(&mut roots).expect("churn safe point");
        }
    }
    let memory = runtime.memory_telemetry();
    assert!(runtime.object_count() < 4_096);
    assert!(memory.committed_bytes() <= 12 * 1024 * 1024);
}
