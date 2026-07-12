use pop_runtime_collector::{BootstrapRuntime, CollectorMetrics};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, RootPublication,
    RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn empty_roots() -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(1), Vec::new()).expect("empty stack map"),
        Vec::<Option<ManagedReference>>::new(),
    )
    .expect("empty root publication")
}

#[test]
fn bootstrap_metrics_count_real_allocations_and_collections() {
    let mut runtime = BootstrapRuntime::new();
    let request = ObjectAllocationRequest::new(
        RuntimeTypeId::new(1),
        AllocationClass::NurseryEligible,
        ObjectMap::new(0, Vec::new()).expect("empty object map"),
    );
    let first = runtime.allocate_object(&request).expect("first object");
    runtime.allocate_object(&request).expect("second object");
    assert_eq!(runtime.metrics(), CollectorMetrics::new(2, 0, 0, 0));

    let root = runtime.retain_root(first).expect("strong root");
    runtime.request_collection();
    let mut roots = empty_roots();
    runtime.safe_point(&mut roots).expect("rooted collection");
    assert_eq!(runtime.metrics(), CollectorMetrics::new(2, 1, 1, 1));

    runtime.release_root(root).expect("release root");
    runtime.request_collection();
    runtime.safe_point(&mut roots).expect("reclaim collection");
    assert_eq!(runtime.metrics(), CollectorMetrics::new(2, 2, 2, 1));
}
