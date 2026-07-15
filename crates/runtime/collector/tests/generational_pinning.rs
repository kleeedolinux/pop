use pop_runtime_collector::{GenerationalRuntime, HeapDomain, PinningConfig};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, RootPublication,
    RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn object() -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(121),
        AllocationClass::Mature,
        ObjectMap::new(0, Vec::new()).expect("object map"),
    )
}

fn no_stack_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

#[test]
fn zero_long_lived_pin_threshold_normalizes_to_one_safe_point() {
    assert_eq!(PinningConfig::new(0).long_lived_pin_safe_points(), 1);
}

#[test]
fn pin_telemetry_counts_handles_objects_and_safe_point_age_exactly() {
    let mut runtime = GenerationalRuntime::with_pinning_config(PinningConfig::new(2));
    let reference = runtime.allocate_object(&object()).expect("mature object");
    let first = runtime.pin(reference).expect("first pin");
    let second = runtime.pin(reference).expect("second pin");

    let telemetry = runtime.pinning_telemetry();
    assert_eq!(telemetry.pins_created(), 2);
    assert_eq!(telemetry.pins_released(), 0);
    assert_eq!(telemetry.active_pin_handles(), 2);
    assert_eq!(telemetry.pinned_objects(), 1);
    assert_eq!(telemetry.long_lived_pins_reported(), 0);
    assert_eq!(
        runtime
            .placement(reference)
            .expect("pinned placement")
            .domain(),
        HeapDomain::Pinned
    );

    let mut roots = no_stack_roots(1);
    runtime.safe_point(&mut roots).expect("first safe point");
    let telemetry = runtime.pinning_telemetry();
    assert_eq!(telemetry.safe_points_observed(), 1);
    assert_eq!(telemetry.current_maximum_pin_age_safe_points(), 1);
    assert_eq!(telemetry.long_lived_pins_reported(), 0);

    runtime.safe_point(&mut roots).expect("second safe point");
    let telemetry = runtime.pinning_telemetry();
    assert_eq!(telemetry.current_maximum_pin_age_safe_points(), 2);
    assert_eq!(telemetry.long_lived_pins_reported(), 2);

    runtime.unpin(first).expect("release first pin");
    let telemetry = runtime.pinning_telemetry();
    assert_eq!(telemetry.pins_released(), 1);
    assert_eq!(telemetry.active_pin_handles(), 1);
    assert_eq!(telemetry.pinned_objects(), 1);
    assert_eq!(telemetry.maximum_completed_pin_age_safe_points(), 2);

    runtime.safe_point(&mut roots).expect("third safe point");
    runtime.unpin(second).expect("release second pin");
    let telemetry = runtime.pinning_telemetry();
    assert_eq!(telemetry.pins_released(), 2);
    assert_eq!(telemetry.active_pin_handles(), 0);
    assert_eq!(telemetry.pinned_objects(), 0);
    assert_eq!(telemetry.current_maximum_pin_age_safe_points(), 0);
    assert_eq!(telemetry.maximum_completed_pin_age_safe_points(), 3);
    assert_eq!(telemetry.long_lived_pins_reported(), 2);
}

#[test]
fn failed_pin_transitions_do_not_change_pinning_telemetry() {
    let mut runtime = GenerationalRuntime::with_pinning_config(PinningConfig::new(2));
    let before = runtime.pinning_telemetry();

    assert!(runtime.pin(ManagedReference::new(99_999)).is_err());
    assert_eq!(runtime.pinning_telemetry(), before);
    assert!(
        runtime
            .unpin(pop_runtime_interface::PinHandle::new(88_888))
            .is_err()
    );
    assert_eq!(runtime.pinning_telemetry(), before);
}

#[test]
#[allow(unsafe_code)]
fn immutable_byte_payload_address_survives_forced_nursery_collection() {
    let mut runtime = GenerationalRuntime::new();
    let bytes = runtime
        .allocate_immutable_bytes(&[4, 3, 2, 1])
        .expect("nursery immutable bytes");
    let borrow = runtime.ffi_bytes_borrow(bytes).expect("payload borrow");
    let address = borrow.address().expect("nonempty payload");
    assert_eq!(
        runtime.placement(bytes).expect("pinned placement").domain(),
        HeapDomain::Pinned
    );

    runtime.request_minor_collection();
    runtime
        .safe_point(&mut no_stack_roots(2))
        .expect("forced nursery collection");
    // SAFETY: The exact byte borrow pins the owner and its immutable payload.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(address.raw() as *const u8, 4) },
        [4, 3, 2, 1]
    );
    runtime
        .ffi_bytes_end_borrow(bytes, borrow.id())
        .expect("end payload borrow");
}
