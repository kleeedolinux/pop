use pop_runtime_collector::{GenerationalRuntime, ObjectOwnership, SchedulerId};
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, ObjectSlot, RootPublication,
    RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn object(class: AllocationClass, references: &[u32]) -> ObjectAllocationRequest {
    let slots = references
        .iter()
        .copied()
        .max()
        .map_or(0, |maximum| maximum + 1);
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(91),
        class,
        ObjectMap::new(
            slots,
            references.iter().copied().map(ObjectSlot::new).collect(),
        )
        .expect("object map"),
    )
}

fn no_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

#[test]
fn each_scheduler_reuses_its_own_tlab_and_page_metadata_names_the_owner() {
    let mut runtime = GenerationalRuntime::new();
    let request = object(AllocationClass::NurseryEligible, &[]);
    let first = runtime
        .allocate_object(&request)
        .expect("scheduler one object");
    let first_placement = runtime.placement(first).expect("first placement");

    runtime.select_scheduler(SchedulerId::new(2));
    let second = runtime
        .allocate_object(&request)
        .expect("scheduler two object");
    let second_placement = runtime.placement(second).expect("second placement");
    assert_ne!(first_placement.page(), second_placement.page());
    assert_eq!(
        runtime
            .page_descriptor(first_placement.page())
            .expect("first page")
            .scheduler(),
        Some(SchedulerId::new(1))
    );
    assert_eq!(
        runtime
            .page_descriptor(second_placement.page())
            .expect("second page")
            .scheduler(),
        Some(SchedulerId::new(2))
    );

    runtime.select_scheduler(SchedulerId::new(1));
    let third = runtime
        .allocate_object(&request)
        .expect("scheduler one reuse");
    assert_eq!(
        runtime.placement(third).expect("third placement").page(),
        first_placement.page()
    );
    assert_eq!(runtime.allocation_metrics().tlab_refills(), 2);
    assert_eq!(
        runtime.ownership(second),
        Some(ObjectOwnership::SchedulerLocal(SchedulerId::new(2)))
    );
}

#[test]
fn scheduler_local_collection_does_not_touch_another_scheduler_nursery() {
    let mut runtime = GenerationalRuntime::new();
    let request = object(AllocationClass::NurseryEligible, &[]);
    let first = runtime
        .allocate_object(&request)
        .expect("scheduler one object");
    runtime.select_scheduler(SchedulerId::new(2));
    let second = runtime
        .allocate_object(&request)
        .expect("scheduler two object");
    runtime.select_scheduler(SchedulerId::new(1));
    let mut roots = no_roots(1);

    runtime.request_minor_collection();
    runtime
        .safe_point(&mut roots)
        .expect("scheduler one collection");

    assert!(!runtime.contains(first));
    assert!(runtime.contains(second));
    assert_eq!(
        runtime.ownership(second),
        Some(ObjectOwnership::SchedulerLocal(SchedulerId::new(2)))
    );
}

#[test]
fn direct_edges_between_scheduler_local_heaps_are_rejected() {
    let mut runtime = GenerationalRuntime::new();
    let owner = runtime
        .allocate_object(&object(AllocationClass::Mature, &[0]))
        .expect("scheduler one owner");
    runtime.select_scheduler(SchedulerId::new(2));
    let target = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, &[]))
        .expect("scheduler two target");

    assert!(
        runtime
            .store_reference(owner, ObjectSlot::new(0), Some(target))
            .is_err()
    );
}

#[test]
fn minor_requests_remain_attached_to_the_requesting_scheduler() {
    let mut runtime = GenerationalRuntime::new();
    let request = object(AllocationClass::NurseryEligible, &[]);
    let first = runtime
        .allocate_object(&request)
        .expect("scheduler one object");
    runtime.request_minor_collection();
    runtime.select_scheduler(SchedulerId::new(2));
    let second = runtime
        .allocate_object(&request)
        .expect("scheduler two object");
    let mut roots = no_roots(2);

    let outcome = runtime
        .safe_point(&mut roots)
        .expect("scheduler two safe point");
    assert!(outcome.collection().is_none());
    assert!(runtime.contains(first));
    assert!(runtime.contains(second));

    runtime.select_scheduler(SchedulerId::new(1));
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("scheduler one collection")
            .collection()
            .is_some()
    );
    assert!(!runtime.contains(first));
    assert!(runtime.contains(second));
}
