use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, BarrierKind,
    GarbageCollectorContract, GarbageCollectorStage, ManagedReference, ObjectAllocationRequest,
    ObjectMap, ObjectMapError, ObjectSlot, PanicKind, PanicPayload, RootMapError, RootPublication,
    RootSlot, RuntimeFailure, SafePointId, StackMap, TableAllocationRequest, Trap, TrapKind,
    UnwindReason, WriteBarrier,
};

#[test]
fn bootstrap_contract_is_precise_without_claiming_production_gc_features() {
    let bootstrap = GarbageCollectorContract::bootstrap_stage1();

    assert_eq!(
        bootstrap.stage(),
        GarbageCollectorStage::BootstrapPreciseStopTheWorld
    );
    assert!(bootstrap.precise_roots());
    assert!(!bootstrap.moving_nursery());
    assert!(!bootstrap.concurrent_mature_marking());
    assert!(!bootstrap.satb_barrier());
    assert!(!bootstrap.generational_card_barrier());
    assert!(!bootstrap.user_finalizers());
    assert!(!bootstrap.weak_references());

    let production = GarbageCollectorContract::pop_v1();
    assert_eq!(
        production.stage(),
        GarbageCollectorStage::ProductionConcurrentGenerational
    );
    assert!(production.moving_nursery());
    assert!(production.concurrent_mature_marking());
}

#[test]
fn object_and_stack_maps_are_canonical_precise_contracts() {
    let object_map =
        ObjectMap::new(4, vec![ObjectSlot::new(3), ObjectSlot::new(1)]).expect("valid object map");
    assert_eq!(
        object_map.reference_slots(),
        &[ObjectSlot::new(1), ObjectSlot::new(3)]
    );
    assert_eq!(object_map.slot_count(), 4);
    assert!(object_map.is_reference_slot(ObjectSlot::new(1)));
    assert!(!object_map.is_reference_slot(ObjectSlot::new(2)));

    assert_eq!(
        ObjectMap::new(1, vec![ObjectSlot::new(1)]),
        Err(ObjectMapError::SlotOutOfBounds {
            slot: ObjectSlot::new(1),
            slot_count: 1,
        })
    );
    assert_eq!(
        ObjectMap::new(2, vec![ObjectSlot::new(0), ObjectSlot::new(0)]),
        Err(ObjectMapError::DuplicateReferenceSlot(ObjectSlot::new(0)))
    );

    let stack_map = StackMap::new(
        SafePointId::new(9),
        vec![RootSlot::new(4), RootSlot::new(1)],
    )
    .expect("valid stack map");
    assert_eq!(
        stack_map.root_slots(),
        &[RootSlot::new(1), RootSlot::new(4)]
    );
    assert_eq!(
        RootPublication::new(
            stack_map.clone(),
            vec![Some(ManagedReference::new(20)), None],
        )
        .expect("matching root values")
        .managed_references()
        .collect::<Vec<_>>(),
        vec![ManagedReference::new(20)]
    );
    assert_eq!(
        RootPublication::new(stack_map, vec![None]),
        Err(RootMapError::ValueCount {
            expected: 2,
            found: 1,
        })
    );
}

#[test]
fn allocation_and_barrier_requests_are_backend_neutral_and_typed() {
    let type_id = pop_runtime_interface::RuntimeTypeId::new(17);
    let object_map = ObjectMap::new(2, vec![ObjectSlot::new(0)]).expect("object map");
    let object = ObjectAllocationRequest::new(
        type_id,
        AllocationClass::NurseryEligible,
        object_map.clone(),
    );
    assert_eq!(object.type_id(), type_id);
    assert_eq!(object.allocation_class(), AllocationClass::NurseryEligible);
    assert_eq!(object.object_map(), &object_map);

    let array = ArrayAllocationRequest::new(
        type_id,
        AllocationClass::NurseryEligible,
        8,
        ArrayElementMap::ManagedReference,
    );
    assert_eq!(array.length(), 8);
    assert_eq!(array.element_map(), ArrayElementMap::ManagedReference);

    let table = TableAllocationRequest::new(
        type_id,
        AllocationClass::NurseryEligible,
        4,
        ArrayElementMap::ManagedReference,
        ArrayElementMap::Scalar,
    )
    .expect("valid table layout");
    assert_eq!(table.entry_count(), 4);
    assert_eq!(table.key_map(), ArrayElementMap::ManagedReference);
    assert_eq!(table.value_map(), ArrayElementMap::Scalar);
    assert_eq!(table.object_map().slot_count(), 8);
    assert_eq!(
        table.object_map().reference_slots(),
        &[
            ObjectSlot::new(0),
            ObjectSlot::new(2),
            ObjectSlot::new(4),
            ObjectSlot::new(6),
        ]
    );
    assert!(
        TableAllocationRequest::new(
            type_id,
            AllocationClass::NurseryEligible,
            u32::MAX,
            ArrayElementMap::Scalar,
            ArrayElementMap::Scalar,
        )
        .is_err()
    );

    let barrier = WriteBarrier::new(
        BarrierKind::CombinedSatbGenerational,
        ManagedReference::new(4),
        ObjectSlot::new(0),
        Some(ManagedReference::new(5)),
        Some(ManagedReference::new(6)),
    );
    assert_eq!(barrier.owner(), ManagedReference::new(4));
    assert_eq!(barrier.previous(), Some(ManagedReference::new(5)));
    assert_eq!(barrier.value(), Some(ManagedReference::new(6)));
}

#[test]
fn traps_panics_and_unwinds_remain_distinct_portable_failures() {
    let trap = Trap::new(TrapKind::IntegerOverflow);
    assert_eq!(RuntimeFailure::Trap(trap), RuntimeFailure::Trap(trap));

    let panic = PanicPayload::out_of_memory(3, 64);
    assert_eq!(
        panic.kind(),
        PanicKind::OutOfMemory {
            requested_objects: 3,
            requested_slots: 64,
        }
    );
    let failure = RuntimeFailure::from_panic(panic.clone());
    assert_eq!(failure, RuntimeFailure::Unwind(UnwindReason::Panic(panic)));
    assert_ne!(
        failure,
        RuntimeFailure::Trap(Trap::new(TrapKind::DivisionByZero))
    );
}
