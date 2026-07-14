use pop_runtime_collector::{
    AllocationInfrastructureConfig, EvacuationSelectionConfig, EvacuationSelectionConfigError,
    GenerationalMemoryConfig, GenerationalRuntime, HeapDomain, MajorCollectorConfig,
    MajorCyclePhase, RegionState, SchedulerId,
};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RootPublication, RootSlot, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn object(type_id: u32, class: AllocationClass, slots: u32) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(type_id),
        class,
        ObjectMap::new(slots, Vec::new()).expect("object map"),
    )
}

fn reference_object(type_id: u32, class: AllocationClass) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(type_id),
        class,
        ObjectMap::new(1, vec![ObjectSlot::new(0)]).expect("object map"),
    )
}

fn runtime(evacuation_reserve_bytes: usize) -> GenerationalRuntime {
    GenerationalRuntime::with_memory_config(
        MajorCollectorConfig::new(1),
        AllocationInfrastructureConfig::new(64, 256, 32).expect("allocation geometry"),
        GenerationalMemoryConfig::new(4 * 1024, 64, evacuation_reserve_bytes, 64, 100, 1)
            .expect("memory configuration"),
    )
}

fn one_root(id: u32, reference: ManagedReference) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), vec![RootSlot::new(0)]).expect("stack map"),
        vec![Some(reference)],
    )
    .expect("root publication")
}

fn region_for(runtime: &GenerationalRuntime, reference: ManagedReference) -> u64 {
    let placement = runtime.placement(reference).expect("placement");
    runtime
        .page_descriptor(placement.page())
        .expect("page descriptor")
        .region()
        .raw()
}

fn finish_major(runtime: &mut GenerationalRuntime, roots: &mut RootPublication) {
    for _ in 0..64 {
        if runtime
            .safe_point(roots)
            .expect("major slice")
            .collection()
            .is_some()
        {
            return;
        }
    }
    panic!("major collection must complete within its work bound");
}

#[test]
fn region_telemetry_keeps_domains_and_scheduler_owners_homogeneous() {
    let mut runtime = runtime(256);
    let first = runtime
        .allocate_object(&object(1, AllocationClass::Mature, 1))
        .expect("first mature object");
    let second = runtime
        .allocate_object(&object(2, AllocationClass::Mature, 1))
        .expect("second mature object");
    runtime.select_scheduler(SchedulerId::new(2));
    let other_scheduler = runtime
        .allocate_object(&object(3, AllocationClass::Mature, 1))
        .expect("other scheduler object");
    let large = runtime
        .allocate_object(&object(4, AllocationClass::Large, 16))
        .expect("large object");
    let pinned = runtime
        .allocate_object(&object(5, AllocationClass::Pinned, 1))
        .expect("pinned object");

    assert_eq!(region_for(&runtime, first), region_for(&runtime, second));
    assert_ne!(
        region_for(&runtime, first),
        region_for(&runtime, other_scheduler)
    );
    assert_ne!(region_for(&runtime, large), region_for(&runtime, pinned));

    let regions = runtime.region_telemetry();
    let local = regions
        .iter()
        .find(|region| region.id().raw() == region_for(&runtime, first))
        .expect("scheduler-one mature region");
    assert_eq!(local.state(), RegionState::LocalMature);
    assert_eq!(local.domain(), HeapDomain::LocalMature);
    assert_eq!(local.scheduler(), Some(SchedulerId::new(1)));
    assert_eq!(local.capacity_bytes(), 256);
    assert_eq!(local.committed_bytes(), 128);
    assert_eq!(local.live_bytes(), 16);
    assert_eq!(local.fragmented_bytes(), 112);
    assert_eq!(local.free_bytes(), 240);
    assert_eq!(local.page_count(), 2);
    assert_eq!(local.object_count(), 2);

    let large_region = regions
        .iter()
        .find(|region| region.id().raw() == region_for(&runtime, large))
        .expect("large region");
    assert_eq!(large_region.state(), RegionState::LargeObject);
    assert_eq!(large_region.capacity_bytes(), 256);
    assert_eq!(large_region.committed_bytes(), 128);
    assert_eq!(large_region.pin_density_percent(), 0);

    let pinned_region = regions
        .iter()
        .find(|region| region.id().raw() == region_for(&runtime, pinned))
        .expect("pinned region");
    assert_eq!(pinned_region.state(), RegionState::Pinned);
    assert_eq!(pinned_region.pinned_bytes(), 8);
    assert_eq!(pinned_region.pin_density_percent(), 100);
}

#[test]
fn evacuation_selection_is_bounded_profitable_and_reserve_admitted() {
    let mut runtime = runtime(256);
    let mut shared = Vec::new();
    for type_id in 10..15 {
        let reference = runtime
            .allocate_object(&object(type_id, AllocationClass::Mature, 1))
            .expect("local mature object");
        runtime.publish_shared(reference).expect("publish object");
        shared.push(reference);
    }
    let selected_region = region_for(&runtime, shared[0]);
    assert!(
        shared[..4]
            .iter()
            .all(|reference| region_for(&runtime, *reference) == selected_region)
    );
    assert_ne!(region_for(&runtime, shared[4]), selected_region);

    let candidates = runtime
        .select_evacuation_candidates(
            EvacuationSelectionConfig::new(1, 50).expect("selection config"),
        )
        .expect("select candidate");
    assert_eq!(candidates.len(), 1);
    let candidate = candidates[0];
    assert_eq!(candidate.region().raw(), selected_region);
    assert_eq!(candidate.live_bytes(), 32);
    assert_eq!(candidate.reclaimable_bytes(), 224);
    assert_eq!(candidate.copy_cost_bytes(), 32);
    assert_eq!(candidate.reference_update_cost_bytes(), 0);
    assert_eq!(candidate.estimated_benefit_bytes(), 192);
    assert_eq!(candidate.object_count(), 4);
    assert_eq!(
        runtime.region_telemetry()[0].state(),
        RegionState::EvacuationCandidate
    );

    let additional = runtime
        .allocate_object(&object(20, AllocationClass::Mature, 1))
        .expect("new local object");
    runtime
        .publish_shared(additional)
        .expect("publish after selection");
    assert_ne!(region_for(&runtime, additional), selected_region);
    assert!(
        runtime
            .select_evacuation_candidates(
                EvacuationSelectionConfig::new(1, 50).expect("selection config")
            )
            .expect("do not duplicate selected region")
            .is_empty()
    );

    assert_eq!(runtime.cancel_evacuation_candidates(), 1);
    assert!(
        runtime
            .region_telemetry()
            .iter()
            .any(|region| region.id().raw() == selected_region
                && region.state() == RegionState::SharedAllocating)
    );
}

#[test]
fn evacuation_selection_excludes_unprofitable_pinned_large_and_over_reserve_regions() {
    assert_eq!(
        EvacuationSelectionConfig::new(0, 50),
        Err(EvacuationSelectionConfigError::ZeroRegionLimit)
    );
    assert_eq!(
        EvacuationSelectionConfig::new(1, 0),
        Err(EvacuationSelectionConfigError::InvalidFragmentationPercent)
    );
    assert_eq!(
        EvacuationSelectionConfig::new(1, 101),
        Err(EvacuationSelectionConfigError::InvalidFragmentationPercent)
    );

    let mut runtime = runtime(16);
    for type_id in 30..34 {
        let reference = runtime
            .allocate_object(&object(type_id, AllocationClass::Mature, 1))
            .expect("local mature object");
        runtime.publish_shared(reference).expect("publish object");
    }
    runtime
        .allocate_object(&object(40, AllocationClass::Large, 1))
        .expect("large object");
    runtime
        .allocate_object(&object(41, AllocationClass::Pinned, 1))
        .expect("pinned object");

    assert!(
        runtime
            .select_evacuation_candidates(
                EvacuationSelectionConfig::new(8, 50).expect("selection config")
            )
            .expect("reserve rejection is not an error")
            .is_empty()
    );
    assert!(runtime.region_telemetry().iter().all(|region| !matches!(
        region.state(),
        RegionState::EvacuationCandidate | RegionState::Evacuating
    )));
}

#[test]
fn shared_region_states_follow_major_mark_and_sweep_transitions() {
    let mut runtime = runtime(256);
    let shared = runtime
        .allocate_object(&object(50, AllocationClass::Mature, 1))
        .expect("local object");
    runtime.publish_shared(shared).expect("publish object");
    let mut roots = one_root(1, shared);

    runtime
        .start_major_collection(&roots)
        .expect("start major collection");
    assert_eq!(
        runtime.region_telemetry()[0].state(),
        RegionState::SharedMarking
    );

    for _ in 0..8 {
        runtime.safe_point(&mut roots).expect("major slice");
        if runtime.major_phase() == MajorCyclePhase::Sweeping {
            break;
        }
    }
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Sweeping);
    assert_eq!(
        runtime.region_telemetry()[0].state(),
        RegionState::SharedSweeping
    );

    for _ in 0..8 {
        if runtime
            .safe_point(&mut roots)
            .expect("finish major collection")
            .collection()
            .is_some()
        {
            break;
        }
    }
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Idle);
    assert_eq!(
        runtime.region_telemetry()[0].state(),
        RegionState::SharedAllocating
    );
}

#[test]
fn selected_shared_regions_copy_objects_and_rewrite_every_reference_kind() {
    let mut runtime = runtime(512);
    let request = reference_object(60, AllocationClass::Mature);
    let mut shared = Vec::new();
    for _ in 0..5 {
        let reference = runtime.allocate_object(&request).expect("mature object");
        runtime.publish_shared(reference).expect("publish object");
        shared.push(reference);
    }
    let selected_region = region_for(&runtime, shared[0]);
    assert!(
        shared[..4]
            .iter()
            .all(|reference| region_for(&runtime, *reference) == selected_region)
    );
    assert_ne!(region_for(&runtime, shared[4]), selected_region);

    runtime
        .store_reference(shared[0], ObjectSlot::new(0), Some(shared[1]))
        .expect("selected-to-selected edge");
    runtime
        .store_reference(shared[4], ObjectSlot::new(0), Some(shared[0]))
        .expect("outside-to-selected edge");
    let child_root = runtime.retain_root(shared[1]).expect("strong child root");
    let mut roots = RootPublication::new(
        StackMap::new(
            SafePointId::new(60),
            vec![RootSlot::new(0), RootSlot::new(1)],
        )
        .expect("stack map"),
        vec![Some(shared[0]), Some(shared[4])],
    )
    .expect("root publication");

    runtime
        .select_evacuation_candidates(
            EvacuationSelectionConfig::new(1, 50).expect("selection config"),
        )
        .expect("select candidate");
    let statistics = runtime
        .evacuate_selected_regions(&mut roots)
        .expect("evacuate selected region");

    assert_eq!(statistics.regions_evacuated(), 1);
    assert_eq!(statistics.objects_evacuated(), 4);
    assert_eq!(statistics.bytes_copied(), 32);
    assert_eq!(statistics.stack_roots_updated(), 1);
    assert_eq!(statistics.strong_handles_updated(), 1);
    assert_eq!(statistics.object_fields_updated(), 2);
    assert!(statistics.committed_bytes_reclaimed() > 0);
    assert!(statistics.peak_committed_bytes() >= 320);
    let mut updated_roots = roots.managed_references();
    let relocated_parent = updated_roots.next().expect("relocated parent root");
    assert_eq!(updated_roots.next(), Some(shared[4]));
    let relocated_child = runtime
        .load_reference(relocated_parent, ObjectSlot::new(0))
        .expect("relocated child slot")
        .expect("relocated child");
    assert_ne!(relocated_parent, shared[0]);
    assert_ne!(relocated_child, shared[1]);
    assert_eq!(
        runtime
            .load_reference(shared[4], ObjectSlot::new(0))
            .expect("outside edge"),
        Some(relocated_parent)
    );
    assert!(
        shared[..4]
            .iter()
            .all(|reference| !runtime.contains(*reference))
    );
    assert!(runtime.contains(shared[4]));
    assert_ne!(region_for(&runtime, relocated_parent), selected_region);
    assert!(
        runtime
            .region_telemetry()
            .iter()
            .all(|region| region.id().raw() != selected_region)
    );

    let mut no_roots = RootPublication::new(
        StackMap::new(SafePointId::new(61), Vec::new()).expect("empty stack map"),
        Vec::new(),
    )
    .expect("empty roots");
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut no_roots);
    assert!(runtime.contains(relocated_child));
    runtime
        .release_root(child_root)
        .expect("release relocated root");
}

#[test]
fn evacuation_rejects_stale_publications_without_partial_relocation() {
    let mut runtime = runtime(256);
    let original = runtime
        .allocate_object(&object(70, AllocationClass::Mature, 1))
        .expect("mature object");
    runtime.publish_shared(original).expect("publish object");
    let selected_region = region_for(&runtime, original);
    runtime
        .select_evacuation_candidates(
            EvacuationSelectionConfig::new(1, 50).expect("selection config"),
        )
        .expect("select candidate");
    let mut stale_roots = one_root(70, ManagedReference::new(u64::MAX));

    assert!(runtime.evacuate_selected_regions(&mut stale_roots).is_err());
    assert!(runtime.contains(original));
    assert_eq!(runtime.object_count(), 1);
    assert_eq!(region_for(&runtime, original), selected_region);
    assert!(runtime.region_telemetry().iter().any(|region| {
        region.id().raw() == selected_region && region.state() == RegionState::EvacuationCandidate
    }));
    assert_eq!(
        stale_roots.managed_references().next(),
        Some(ManagedReference::new(u64::MAX))
    );
}
