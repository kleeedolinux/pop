use pop_runtime_collector::{
    AllocationInfrastructureConfig, GenerationalRuntime, HeapDomain, MajorCollectorConfig,
};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ObjectAllocationRequest, ObjectMap,
    ObjectSlot, RootPublication, RootSlot, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn object(
    type_id: u32,
    class: AllocationClass,
    slots: u32,
    references: &[u32],
) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(type_id),
        class,
        ObjectMap::new(
            slots,
            references.iter().copied().map(ObjectSlot::new).collect(),
        )
        .expect("object map"),
    )
}

fn runtime() -> GenerationalRuntime {
    GenerationalRuntime::with_allocation_config(
        MajorCollectorConfig::new(8),
        AllocationInfrastructureConfig::new(64, 256, 32).expect("allocation config"),
    )
}

#[test]
fn same_layout_nursery_allocations_use_a_bounded_pointer_bump_tlab() {
    let mut runtime = runtime();
    let request = object(1, AllocationClass::NurseryEligible, 0, &[]);
    let references: Vec<_> = (0..5)
        .map(|_| {
            runtime
                .allocate_object(&request)
                .expect("nursery allocation")
        })
        .collect();

    let first = runtime.placement(references[0]).expect("first placement");
    for (index, reference) in references.iter().take(4).enumerate() {
        let placement = runtime.placement(*reference).expect("placement");
        assert_eq!(placement.page(), first.page());
        assert_eq!(placement.offset_bytes(), index * 8);
        assert_eq!(placement.domain(), HeapDomain::LocalEden);
    }
    assert_ne!(
        runtime.placement(references[4]).expect("fifth").page(),
        first.page()
    );
    let metrics = runtime.allocation_metrics();
    assert_eq!(metrics.tlab_allocations(), 5);
    assert_eq!(metrics.tlab_refills(), 2);
    assert_eq!(metrics.pages_created(), 2);
}

#[test]
fn pages_are_monomorphic_and_record_precise_pointer_layouts() {
    let mut runtime = runtime();
    let scalar = runtime
        .allocate_object(&object(10, AllocationClass::NurseryEligible, 2, &[]))
        .expect("scalar");
    let traced = runtime
        .allocate_object(&object(11, AllocationClass::NurseryEligible, 2, &[1]))
        .expect("traced");

    let scalar_placement = runtime.placement(scalar).expect("scalar placement");
    let traced_placement = runtime.placement(traced).expect("traced placement");
    assert_ne!(scalar_placement.page(), traced_placement.page());
    let scalar_page = runtime
        .page_descriptor(scalar_placement.page())
        .expect("scalar page");
    let traced_page = runtime
        .page_descriptor(traced_placement.page())
        .expect("traced page");
    assert!(scalar_page.pointer_free());
    assert!(!traced_page.pointer_free());
    assert_eq!(scalar_page.type_id(), RuntimeTypeId::new(10));
    assert_eq!(traced_page.reference_slots(), &[ObjectSlot::new(1)]);
}

#[test]
fn mature_large_and_pinned_allocations_bypass_the_local_eden_tlab() {
    let mut runtime = runtime();
    let mature = runtime
        .allocate_object(&object(20, AllocationClass::Mature, 1, &[]))
        .expect("mature");
    let large = runtime
        .allocate_object(&object(21, AllocationClass::Large, 1, &[]))
        .expect("large");
    let pinned = runtime
        .allocate_object(&object(22, AllocationClass::Pinned, 1, &[]))
        .expect("pinned");

    assert_eq!(
        runtime
            .placement(mature)
            .expect("mature placement")
            .domain(),
        HeapDomain::LocalMature
    );
    assert_eq!(
        runtime.placement(large).expect("large placement").domain(),
        HeapDomain::LargeObject
    );
    assert_eq!(
        runtime
            .placement(pinned)
            .expect("pinned placement")
            .domain(),
        HeapDomain::Pinned
    );
    assert_eq!(runtime.allocation_metrics().tlab_allocations(), 0);
}

#[test]
fn same_layout_mature_allocations_reuse_free_page_capacity() {
    let mut runtime = runtime();
    let request = object(23, AllocationClass::Mature, 1, &[]);
    let references = (0..5)
        .map(|_| {
            runtime
                .allocate_object(&request)
                .expect("mature allocation")
        })
        .collect::<Vec<_>>();
    let first = runtime.placement(references[0]).expect("first placement");

    for (index, reference) in references.iter().enumerate() {
        let placement = runtime.placement(*reference).expect("placement");
        assert_eq!(placement.page(), first.page());
        assert_eq!(placement.offset_bytes(), index * 8);
        assert_eq!(placement.domain(), HeapDomain::LocalMature);
    }
    assert_eq!(runtime.allocation_metrics().pages_created(), 1);
    assert_eq!(runtime.allocation_metrics().mature_page_index_hits(), 4);
}

#[test]
fn scalar_array_bulk_initialization_constructs_the_final_payload_once() {
    let mut runtime = runtime();
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(26),
        AllocationClass::Mature,
        256,
        ArrayElementMap::Scalar,
    );
    let array = runtime
        .allocate_array_filled(&request, 42)
        .expect("bulk initialized array");

    assert_eq!(
        runtime
            .load_array_value(array, ObjectSlot::new(0))
            .expect("first value"),
        42
    );
    assert_eq!(
        runtime
            .load_array_value(array, ObjectSlot::new(255))
            .expect("last value"),
        42
    );
}

#[test]
fn managed_array_bulk_initialization_installs_the_precise_value_before_publication() {
    let mut runtime = runtime();
    let child = runtime
        .allocate_object(&object(27, AllocationClass::Mature, 0, &[]))
        .expect("managed child");
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(28),
        AllocationClass::Mature,
        256,
        ArrayElementMap::ManagedReference,
    );
    let array = runtime
        .allocate_array_filled(&request, child.raw())
        .expect("bulk initialized managed array");

    assert_eq!(
        runtime
            .load_array_value(array, ObjectSlot::new(0))
            .expect("first reference"),
        child.raw()
    );
    assert_eq!(
        runtime
            .load_array_value(array, ObjectSlot::new(255))
            .expect("last reference"),
        child.raw()
    );
}

#[test]
fn mature_page_reuse_preserves_monomorphic_layout_and_scheduler() {
    let mut runtime = runtime();
    let first = runtime
        .allocate_object(&object(24, AllocationClass::Mature, 2, &[]))
        .expect("first layout");
    let traced = runtime
        .allocate_object(&object(24, AllocationClass::Mature, 2, &[1]))
        .expect("traced layout");
    let other_type = runtime
        .allocate_object(&object(25, AllocationClass::Mature, 2, &[]))
        .expect("other type");

    assert_ne!(
        runtime.placement(first).expect("first").page(),
        runtime.placement(traced).expect("traced").page()
    );
    assert_ne!(
        runtime.placement(first).expect("first").page(),
        runtime.placement(other_type).expect("other type").page()
    );
}

#[test]
fn invalid_page_region_or_tlab_geometry_fails_closed() {
    assert!(AllocationInfrastructureConfig::new(0, 256, 32).is_err());
    assert!(AllocationInfrastructureConfig::new(64, 250, 32).is_err());
    assert!(AllocationInfrastructureConfig::new(64, 256, 80).is_err());
    assert!(AllocationInfrastructureConfig::new(63, 252, 31).is_err());
}

#[test]
fn nursery_copying_replaces_eden_placement_with_survivor_then_mature_pages() {
    let mut runtime = runtime();
    let request = object(30, AllocationClass::NurseryEligible, 0, &[]);
    let young = runtime.allocate_object(&request).expect("young");
    let garbage = runtime.allocate_object(&request).expect("garbage");
    let mut roots = RootPublication::new(
        StackMap::new(SafePointId::new(1), vec![RootSlot::new(0)]).expect("stack map"),
        vec![Some(young)],
    )
    .expect("roots");

    runtime.request_minor_collection();
    runtime.safe_point(&mut roots).expect("first minor");
    let survivor = roots.managed_references().next().expect("survivor");
    assert!(runtime.placement(young).is_none());
    assert!(runtime.placement(garbage).is_none());
    assert_eq!(
        runtime
            .placement(survivor)
            .expect("survivor placement")
            .domain(),
        HeapDomain::LocalSurvivor
    );

    runtime.request_minor_collection();
    runtime.safe_point(&mut roots).expect("second minor");
    let mature = roots.managed_references().next().expect("mature");
    assert!(runtime.placement(survivor).is_none());
    assert_eq!(
        runtime
            .placement(mature)
            .expect("mature placement")
            .domain(),
        HeapDomain::LocalMature
    );
}

#[test]
fn pinning_moves_a_young_placement_to_stable_pinned_space_immediately() {
    let mut runtime = runtime();
    let young = runtime
        .allocate_object(&object(31, AllocationClass::NurseryEligible, 1, &[0]))
        .expect("young");
    assert_eq!(
        runtime.placement(young).expect("eden placement").domain(),
        HeapDomain::LocalEden
    );

    let pin = runtime.pin(young).expect("pin");

    assert_eq!(
        runtime.placement(young).expect("pinned placement").domain(),
        HeapDomain::Pinned
    );
    runtime.unpin(pin).expect("unpin");
}
