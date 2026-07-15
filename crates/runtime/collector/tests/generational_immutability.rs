use pop_runtime_collector::{GenerationalRuntime, ObjectMutability, ObjectOwnership};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ObjectAllocationRequest, ObjectMap,
    ObjectSlot, RuntimeAdapter, RuntimeTypeId,
};

fn object(references: &[u32]) -> ObjectAllocationRequest {
    let slots = references
        .iter()
        .copied()
        .max()
        .map_or(1, |maximum| maximum + 1);
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(101),
        AllocationClass::Mature,
        ObjectMap::new(
            slots,
            references.iter().copied().map(ObjectSlot::new).collect(),
        )
        .expect("object map"),
    )
}

#[test]
fn freezing_shared_graph_is_atomic_and_keeps_ownership_distinct() {
    let mut runtime = GenerationalRuntime::new();
    let child = runtime.allocate_object(&object(&[])).expect("child");
    let parent = runtime.allocate_object(&object(&[0])).expect("parent");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("local edge");
    runtime.publish_shared(parent).expect("shared graph");
    let parent_placement = runtime.placement(parent).expect("parent placement");
    let child_placement = runtime.placement(child).expect("child placement");

    let frozen = runtime.freeze_shared(parent).expect("immutable graph");

    assert_eq!(frozen.objects_frozen(), 2);
    assert_eq!(
        runtime.mutability(parent),
        Some(ObjectMutability::SharedImmutable)
    );
    assert_eq!(
        runtime.mutability(child),
        Some(ObjectMutability::SharedImmutable)
    );
    assert_eq!(runtime.ownership(parent), Some(ObjectOwnership::Shared));
    assert_eq!(runtime.ownership(child), Some(ObjectOwnership::Shared));
    assert_eq!(runtime.placement(parent), Some(parent_placement));
    assert_eq!(runtime.placement(child), Some(child_placement));

    let local = runtime.allocate_object(&object(&[])).expect("local object");
    assert!(runtime.freeze_shared(local).is_err());
    assert_eq!(runtime.mutability(local), Some(ObjectMutability::Mutable));
}

#[test]
fn immutable_shared_payload_rejects_scalar_reference_and_bulk_mutation() {
    let mut runtime = GenerationalRuntime::new();
    let target = runtime.allocate_object(&object(&[])).expect("target");
    let owner = runtime.allocate_object(&object(&[0])).expect("owner");
    runtime.publish_shared(owner).expect("owner publication");
    runtime.publish_shared(target).expect("target publication");
    runtime.freeze_shared(owner).expect("owner freeze");

    assert!(
        runtime
            .store_reference(owner, ObjectSlot::new(0), Some(target))
            .is_err()
    );
    assert_eq!(
        runtime
            .load_reference(owner, ObjectSlot::new(0))
            .expect("unchanged edge"),
        None
    );

    let scalar = runtime
        .allocate_array(&ArrayAllocationRequest::new(
            RuntimeTypeId::new(102),
            AllocationClass::Mature,
            2,
            ArrayElementMap::Scalar,
        ))
        .expect("scalar array");
    runtime.publish_shared(scalar).expect("scalar publication");
    runtime.freeze_shared(scalar).expect("scalar freeze");
    assert!(runtime.store_scalar(scalar, ObjectSlot::new(0), 9).is_err());
    assert_eq!(
        runtime
            .load_scalar(scalar, ObjectSlot::new(0))
            .expect("scalar"),
        0
    );
    assert!(runtime.fill_array_value(scalar, 7).is_err());
    assert_eq!(
        runtime
            .scalar_array_values(scalar, RuntimeTypeId::new(102))
            .expect("scalar array")
            .collect::<Vec<_>>(),
        vec![0, 0]
    );
}
