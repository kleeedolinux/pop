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

#[test]
#[allow(unsafe_code)]
fn immutable_bytes_borrow_exposes_only_one_stable_payload_region() {
    let mut runtime = StableGenerationalRuntime::new();
    let empty = runtime
        .allocate_immutable_bytes(&[])
        .expect("empty immutable bytes");
    let empty_borrow = runtime.ffi_bytes_borrow(empty).expect("empty borrow");
    assert_eq!(empty_borrow.address(), None);
    assert_eq!(empty_borrow.length(), 0);
    runtime
        .ffi_bytes_end_borrow(empty, empty_borrow.id())
        .expect("end empty borrow");

    let bytes = runtime
        .allocate_immutable_bytes(&[1, 2, 3, 4])
        .expect("immutable bytes");
    let other = runtime
        .allocate_immutable_bytes(&[9])
        .expect("other immutable bytes");
    let borrow = runtime.ffi_bytes_borrow(bytes).expect("payload borrow");
    assert_eq!(borrow.length(), 4);
    let address = borrow.address().expect("nonempty payload address");
    // SAFETY: The active borrow owns an immutable payload of the reported
    // length until the exact matching end operation below.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(address.raw() as *const u8, 4) },
        [1, 2, 3, 4]
    );
    assert!(runtime.ffi_bytes_borrow(bytes).is_err());
    assert!(runtime.ffi_bytes_end_borrow(other, borrow.id()).is_err());

    runtime.request_collection();
    runtime
        .safe_point(&mut empty_roots(4))
        .expect("collection while payload is pinned");
    // SAFETY: Collection cannot move or reclaim the active borrowed payload.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(address.raw() as *const u8, 4) },
        [1, 2, 3, 4]
    );
    runtime
        .ffi_bytes_end_borrow(bytes, borrow.id())
        .expect("end exact payload borrow");
    assert!(runtime.ffi_bytes_end_borrow(bytes, borrow.id()).is_err());
}
