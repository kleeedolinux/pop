use pop_runtime_collector::{
    AllocationInfrastructureConfig, GenerationalRuntime, HeapDomain, MajorCollectorConfig,
};
use pop_runtime_interface::{
    AllocationClass, AllocationSiteDescriptor, ArrayAllocationRequest, ArrayElementMap,
    ObjectAllocationRequest, ObjectMap, ObjectSlot, RootPublication, RootSlot, RuntimeAdapter,
    RuntimeAllocationSiteId, RuntimeTypeId, SafePointId, StackMap,
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
fn monomorphic_pages_own_the_contiguous_physical_payload_words() {
    let mut runtime = runtime();
    let request = object(230, AllocationClass::Mature, 2, &[]);
    let first = runtime
        .allocate_object(&request)
        .expect("first page-backed object");
    let second = runtime
        .allocate_object(&request)
        .expect("second page-backed object");
    let first_placement = runtime.placement(first).expect("first placement");
    let second_placement = runtime.placement(second).expect("second placement");
    assert_eq!(first_placement.page(), second_placement.page());
    assert!(runtime.payload_is_page_backed(first));
    assert!(runtime.payload_is_page_backed(second));
    assert!(runtime.layout_is_page_shared(first));
    assert!(runtime.layout_is_page_shared(second));

    runtime
        .store_scalar(first, ObjectSlot::new(0), 11)
        .expect("first payload store");
    runtime
        .store_scalar(first, ObjectSlot::new(1), 12)
        .expect("second payload store");
    runtime
        .store_scalar(second, ObjectSlot::new(0), 21)
        .expect("third payload store");
    runtime
        .store_scalar(second, ObjectSlot::new(1), 22)
        .expect("fourth payload store");

    let page = first_placement.page();
    assert_eq!(
        runtime
            .page_descriptor(page)
            .expect("page descriptor")
            .payload_word_capacity(),
        8
    );
    assert_eq!(runtime.page_payload_word(page, 0), Some(11));
    assert_eq!(runtime.page_payload_word(page, 1), Some(12));
    assert_eq!(runtime.page_payload_word(page, 2), Some(21));
    assert_eq!(runtime.page_payload_word(page, 3), Some(22));
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
    assert!(runtime.payload_is_page_backed(survivor));
    assert!(runtime.layout_is_page_shared(survivor));

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
    assert!(runtime.payload_is_page_backed(mature));
    assert!(runtime.layout_is_page_shared(mature));
}

#[test]
fn nursery_relocation_invalidates_a_retained_direct_page_access() {
    let mut runtime = runtime();
    let request = object(301, AllocationClass::NurseryEligible, 1, &[]);
    let young = runtime.allocate_object(&request).expect("young");
    let access = runtime
        .direct_object_page_access(young)
        .expect("direct page access");
    assert_eq!(access.load(young, ObjectSlot::new(0)), Some(0));
    assert!(access.store_scalar(young, ObjectSlot::new(0), 41));
    assert_eq!(access.load(young, ObjectSlot::new(0)), Some(41));
    let mut roots = RootPublication::new(
        StackMap::new(SafePointId::new(301), vec![RootSlot::new(0)]).expect("stack map"),
        vec![Some(young)],
    )
    .expect("roots");

    runtime.request_minor_collection();
    runtime.safe_point(&mut roots).expect("minor collection");

    assert_eq!(access.load(young, ObjectSlot::new(0)), None);
    assert!(!access.store_scalar(young, ObjectSlot::new(0), 99));
    let relocated = roots.managed_references().next().expect("relocated root");
    assert_eq!(
        runtime
            .load_scalar(relocated, ObjectSlot::new(0))
            .expect("relocated scalar"),
        41
    );
}

#[test]
fn direct_mature_reference_store_is_invalidated_before_major_marking() {
    let mut runtime = runtime();
    let child_request = object(302, AllocationClass::Mature, 1, &[]);
    let first = runtime
        .allocate_object(&child_request)
        .expect("first child");
    let second = runtime
        .allocate_object(&child_request)
        .expect("second child");
    let array_request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(303),
        AllocationClass::Mature,
        2,
        ArrayElementMap::ManagedReference,
    );
    let array = runtime
        .allocate_array_filled(&array_request, first.raw())
        .expect("managed array");
    let store = runtime
        .direct_array_reference_store_access(array)
        .expect("direct reference store");
    let target = runtime
        .direct_reference_validation(second)
        .expect("direct target validation");

    assert!(store.store(array, ObjectSlot::new(1), Some((second, &target))));
    assert_eq!(
        runtime
            .load_array_value(array, ObjectSlot::new(1))
            .expect("stored reference"),
        second.raw()
    );

    let roots = RootPublication::new(
        StackMap::new(SafePointId::new(302), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("roots");
    runtime
        .start_major_collection(&roots)
        .expect("start major collection");
    assert!(!store.store(array, ObjectSlot::new(0), None));
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
    assert!(runtime.payload_is_page_backed(young));
    assert!(runtime.layout_is_page_shared(young));
    runtime.unpin(pin).expect("unpin");
}

#[test]
fn sustained_site_survival_adaptively_pretenures_only_that_exact_layout() {
    let mut runtime = runtime();
    let descriptor = AllocationSiteDescriptor::new(
        RuntimeAllocationSiteId::new(7, 11, 13),
        RuntimeTypeId::new(310),
        AllocationClass::NurseryEligible,
        ObjectMap::scalar(1),
    );
    let request = ObjectAllocationRequest::from_descriptor(&descriptor);
    let mut roots = RootPublication::new(
        StackMap::new(SafePointId::new(310), (0..4).map(RootSlot::new).collect())
            .expect("four-root stack map"),
        (0..4)
            .map(|_| runtime.allocate_object(&request).map(Some))
            .collect::<Result<Vec<_>, _>>()
            .expect("site allocations"),
    )
    .expect("site roots");

    for _ in 0..2 {
        runtime.request_minor_collection();
        runtime
            .safe_point(&mut roots)
            .expect("high-survival minor collection");
    }

    let pretenured = runtime
        .allocate_object(&request)
        .expect("adaptively pretenured allocation");
    assert_eq!(
        runtime
            .placement(pretenured)
            .expect("pretenured placement")
            .domain(),
        HeapDomain::LocalMature
    );
    assert_eq!(
        runtime.pretenured_allocation_sites(),
        [descriptor.site()].into_iter().collect()
    );

    let unrelated = runtime
        .allocate_object(&ObjectAllocationRequest::new(
            descriptor.type_id(),
            AllocationClass::NurseryEligible,
            descriptor.object_map().clone(),
        ))
        .expect("unrelated non-site allocation");
    assert_eq!(
        runtime
            .placement(unrelated)
            .expect("unrelated placement")
            .domain(),
        HeapDomain::LocalEden
    );
}
