use pop_runtime_collector::{GenerationalRuntime, HeapDomain, ObjectOwnership, SchedulerId};
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, ObjectSlot, RuntimeAdapter, RuntimeTypeId,
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
