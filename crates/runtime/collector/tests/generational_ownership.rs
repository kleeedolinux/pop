use pop_runtime_collector::{GenerationalRuntime, HeapDomain, ObjectOwnership, SchedulerId};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RootPublication, RootSlot, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn object(class: AllocationClass, references: &[u32]) -> ObjectAllocationRequest {
    let slots = references
        .iter()
        .copied()
        .max()
        .map_or(0, |maximum| maximum + 1);
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(81),
        class,
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

#[test]
fn explicit_publication_moves_the_complete_local_graph_to_shared_ownership() {
    let mut runtime = GenerationalRuntime::new();
    let child = runtime
        .allocate_object(&object(AllocationClass::Mature, &[]))
        .expect("child");
    let parent = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("parent");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("local edge");

    assert_eq!(
        runtime.ownership(parent),
        Some(ObjectOwnership::SchedulerLocal(SchedulerId::new(1)))
    );
    let publication = runtime.publish_shared(parent).expect("publish graph");

    assert_eq!(publication.objects_published(), 2);
    assert_eq!(runtime.ownership(parent), Some(ObjectOwnership::Shared));
    assert_eq!(runtime.ownership(child), Some(ObjectOwnership::Shared));
    assert_eq!(
        runtime
            .placement(parent)
            .expect("parent placement")
            .domain(),
        HeapDomain::Shared
    );
    assert_eq!(
        runtime.placement(child).expect("child placement").domain(),
        HeapDomain::Shared
    );
}

#[test]
fn shared_objects_reject_direct_edges_to_scheduler_local_memory_before_mutation() {
    let mut runtime = GenerationalRuntime::new();
    let shared = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("shared candidate");
    runtime.publish_shared(shared).expect("publish owner");
    let local = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, &[]))
        .expect("local child");

    assert!(
        runtime
            .store_reference(shared, ObjectSlot::new(0), Some(local))
            .is_err()
    );
    assert_eq!(runtime.ownership(shared), Some(ObjectOwnership::Shared));
    assert_eq!(
        runtime.ownership(local),
        Some(ObjectOwnership::SchedulerLocal(SchedulerId::new(1)))
    );
}

#[test]
fn pinned_placement_remains_distinct_when_its_ownership_becomes_shared() {
    let mut runtime = GenerationalRuntime::new();
    let value = runtime
        .allocate_object(&object(AllocationClass::Pinned, &[]))
        .expect("pinned value");
    assert_eq!(
        runtime.placement(value).expect("pinned placement").domain(),
        HeapDomain::Pinned
    );

    runtime.publish_shared(value).expect("publish pinned value");

    assert_eq!(runtime.ownership(value), Some(ObjectOwnership::Shared));
    assert_eq!(
        runtime.placement(value).expect("pinned placement").domain(),
        HeapDomain::Pinned
    );
}

#[test]
fn isolated_graph_transfer_is_zero_copy_and_changes_only_the_owner() {
    let mut runtime = GenerationalRuntime::new();
    let child = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("child");
    let parent = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("parent");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("parent edge");
    runtime
        .store_reference(child, ObjectSlot::new(0), Some(parent))
        .expect("cycle edge");
    let owner = runtime.retain_root(parent).expect("unique owner");
    let roots = no_stack_roots(10);

    let isolated = runtime
        .isolate_graph(owner, &roots, SchedulerId::new(1))
        .expect("isolate graph");
    let region = isolated.region();
    assert_eq!(isolated.objects_isolated(), 2);
    assert_eq!(
        runtime.ownership(parent),
        Some(ObjectOwnership::Isolated(region))
    );
    assert_eq!(
        runtime.ownership(child),
        Some(ObjectOwnership::Isolated(region))
    );
    let parent_page = runtime.placement(parent).expect("parent placement").page();
    assert_eq!(
        runtime
            .placement(parent)
            .expect("parent placement")
            .domain(),
        HeapDomain::Isolated
    );

    runtime
        .transfer_isolated(region, SchedulerId::new(1), SchedulerId::new(2))
        .expect("zero-copy transfer");

    assert_eq!(
        runtime.isolated_region_owner(region),
        Some(SchedulerId::new(2))
    );
    assert_eq!(
        runtime.placement(parent).expect("same placement").page(),
        parent_page
    );
    assert!(runtime.contains(parent));
    assert!(runtime.contains(child));
    let telemetry = runtime.isolation_telemetry();
    assert_eq!(telemetry.regions_created(), 1);
    assert_eq!(telemetry.transfers_completed(), 1);
    assert_eq!(telemetry.objects_transferred(), 2);
    assert!(runtime.memory_telemetry().isolated_region_bytes() > 0);

    runtime
        .dissolve_isolated(region, SchedulerId::new(2))
        .expect("dissolve region");
    assert_eq!(
        runtime.ownership(parent),
        Some(ObjectOwnership::SchedulerLocal(SchedulerId::new(2)))
    );
    assert_eq!(
        runtime.placement(parent).expect("local placement").domain(),
        HeapDomain::LocalMature
    );
    assert_eq!(runtime.isolated_region_owner(region), None);
    assert_eq!(runtime.isolation_telemetry().regions_dissolved(), 1);
    runtime
        .release_root(owner)
        .expect("release dissolved owner");
}

#[test]
fn isolation_rejects_additional_external_owners_without_partial_transition() {
    let mut runtime = GenerationalRuntime::new();
    let child = runtime
        .allocate_object(&object(AllocationClass::Mature, &[]))
        .expect("child");
    let parent = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("parent");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("parent edge");
    let owner = runtime.retain_root(parent).expect("owner");
    let additional = runtime.retain_root(child).expect("additional owner");

    assert!(
        runtime
            .isolate_graph(owner, &no_stack_roots(11), SchedulerId::new(1))
            .is_err()
    );
    assert_eq!(
        runtime.ownership(parent),
        Some(ObjectOwnership::SchedulerLocal(SchedulerId::new(1)))
    );
    runtime
        .release_root(additional)
        .expect("remove additional owner");
    assert!(
        runtime
            .isolate_graph(owner, &one_stack_root(12, child), SchedulerId::new(1))
            .is_err()
    );
    assert_eq!(
        runtime.ownership(child),
        Some(ObjectOwnership::SchedulerLocal(SchedulerId::new(1)))
    );
}

#[test]
fn isolated_regions_reject_new_external_object_edges_and_roots() {
    let mut runtime = GenerationalRuntime::new();
    let isolated_value = runtime
        .allocate_object(&object(AllocationClass::Mature, &[]))
        .expect("isolated value");
    let owner = runtime.retain_root(isolated_value).expect("owner");
    let isolated = runtime
        .isolate_graph(owner, &no_stack_roots(13), SchedulerId::new(1))
        .expect("isolate value");
    let local = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("local owner");

    assert!(
        runtime
            .store_reference(local, ObjectSlot::new(0), Some(isolated_value))
            .is_err()
    );
    assert!(runtime.retain_root(isolated_value).is_err());
    assert!(runtime.release_root(owner).is_err());
    assert!(
        runtime
            .transfer_isolated(isolated.region(), SchedulerId::new(2), SchedulerId::new(3),)
            .is_err()
    );
    assert_eq!(
        runtime.isolated_region_owner(isolated.region()),
        Some(SchedulerId::new(1))
    );
}

#[test]
fn isolation_rejects_outside_incoming_edges_and_pins() {
    let mut runtime = GenerationalRuntime::new();
    let value = runtime
        .allocate_object(&object(AllocationClass::Mature, &[]))
        .expect("candidate");
    let owner = runtime.retain_root(value).expect("owner");
    let outside = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("outside object");
    runtime
        .store_reference(outside, ObjectSlot::new(0), Some(value))
        .expect("outside incoming edge");

    assert!(
        runtime
            .isolate_graph(owner, &no_stack_roots(14), SchedulerId::new(1))
            .is_err()
    );
    runtime
        .store_reference(outside, ObjectSlot::new(0), None)
        .expect("remove outside edge");
    let pin = runtime.pin(value).expect("pin candidate");
    assert!(
        runtime
            .isolate_graph(owner, &no_stack_roots(15), SchedulerId::new(1))
            .is_err()
    );
    runtime.unpin(pin).expect("unpin candidate");
    assert!(
        runtime
            .isolate_graph(owner, &no_stack_roots(16), SchedulerId::new(1))
            .is_ok()
    );
}
