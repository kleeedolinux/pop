use pop_runtime_collector::{
    AllocationInfrastructureConfig, ArenaAllocationRequest, ArenaConfig, ArenaConfigError,
    ArenaSlotValue, GenerationalMemoryConfig, GenerationalRuntime, MajorCollectorConfig,
    SchedulerId,
};
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, ObjectSlot, RootPublication,
    RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn no_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

#[test]
fn arena_configuration_and_layout_fail_closed() {
    assert_eq!(ArenaConfig::new(0), Err(ArenaConfigError::ZeroCapacity));
    assert!(ArenaAllocationRequest::new(RuntimeTypeId::new(1), 1, vec![0], vec![0]).is_err());
    assert!(ArenaAllocationRequest::new(RuntimeTypeId::new(1), 1, vec![1], Vec::new()).is_err());
}

#[test]
fn arenas_bump_allocate_typed_objects_and_bulk_close() {
    let mut runtime = GenerationalRuntime::new();
    let arena = runtime
        .create_arena(ArenaConfig::new(128).expect("arena configuration"))
        .expect("arena");
    let request = ArenaAllocationRequest::new(RuntimeTypeId::new(101), 2, vec![0], vec![1])
        .expect("arena layout");
    let first = runtime.allocate_in_arena(arena, &request).expect("first");
    let second = runtime.allocate_in_arena(arena, &request).expect("second");
    assert!(second.offset_bytes() > first.offset_bytes());
    runtime
        .store_arena_reference(first, ObjectSlot::new(1), Some(second))
        .expect("same-arena edge");
    assert_eq!(
        runtime.load_arena_slot(first, ObjectSlot::new(1)),
        Ok(ArenaSlotValue::ArenaReference(Some(second)))
    );

    let statistics = runtime.close_arena(arena).expect("bulk close");
    assert_eq!(statistics.objects_reclaimed(), 2);
    assert!(runtime.load_arena_slot(first, ObjectSlot::new(1)).is_err());
    let telemetry = runtime.arena_telemetry();
    assert_eq!(telemetry.arenas_created(), 1);
    assert_eq!(telemetry.arenas_closed(), 1);
    assert_eq!(telemetry.objects_bulk_reclaimed(), 2);
}

#[test]
fn managed_arena_slots_are_precise_roots_and_follow_minor_relocation() {
    let mut runtime = GenerationalRuntime::new();
    let arena = runtime
        .create_arena(ArenaConfig::new(64).expect("arena configuration"))
        .expect("arena");
    let arena_object = runtime
        .allocate_in_arena(
            arena,
            &ArenaAllocationRequest::new(RuntimeTypeId::new(102), 1, vec![0], Vec::new())
                .expect("arena layout"),
        )
        .expect("arena object");
    let managed = runtime
        .allocate_object(&ObjectAllocationRequest::new(
            RuntimeTypeId::new(103),
            AllocationClass::NurseryEligible,
            ObjectMap::new(0, Vec::new()).expect("managed map"),
        ))
        .expect("managed target");
    runtime
        .store_arena_managed_reference(arena_object, ObjectSlot::new(0), Some(managed))
        .expect("managed arena edge");
    let mut roots = no_roots(1);

    runtime.request_minor_collection();
    runtime.safe_point(&mut roots).expect("minor collection");

    let ArenaSlotValue::ManagedReference(Some(relocated)) = runtime
        .load_arena_slot(arena_object, ObjectSlot::new(0))
        .expect("relocated arena root")
    else {
        panic!("managed arena slot was not retained");
    };
    assert_ne!(relocated, managed);
    assert!(runtime.contains(relocated));
    runtime.close_arena(arena).expect("close arena");
    runtime.request_minor_collection();
    runtime.safe_point(&mut roots).expect("reclaim target");
    assert!(!runtime.contains(relocated));
}

#[test]
fn arena_edges_cannot_cross_arena_or_scheduler_boundaries() {
    let mut runtime = GenerationalRuntime::new();
    let config = ArenaConfig::new(64).expect("arena configuration");
    let first_arena = runtime.create_arena(config).expect("first arena");
    let request = ArenaAllocationRequest::new(RuntimeTypeId::new(104), 1, Vec::new(), vec![0])
        .expect("arena layout");
    let first = runtime
        .allocate_in_arena(first_arena, &request)
        .expect("first object");
    runtime.select_scheduler(SchedulerId::new(2));
    let second_arena = runtime.create_arena(config).expect("second arena");
    let second = runtime
        .allocate_in_arena(second_arena, &request)
        .expect("second object");

    assert!(
        runtime
            .store_arena_reference(first, ObjectSlot::new(0), Some(second))
            .is_err()
    );
}

#[test]
fn arena_allocation_obeys_the_global_hard_limit_before_mutation() {
    let mut runtime = GenerationalRuntime::with_memory_config(
        MajorCollectorConfig::new(1),
        AllocationInfrastructureConfig::new(64, 256, 32).expect("allocation geometry"),
        GenerationalMemoryConfig::new(256, 32, 32, 32, 100, 1).expect("memory configuration"),
    );
    let arena = runtime
        .create_arena(ArenaConfig::new(256).expect("arena configuration"))
        .expect("arena");
    let oversized =
        ArenaAllocationRequest::new(RuntimeTypeId::new(105), 25, Vec::new(), Vec::new())
            .expect("arena layout");

    assert!(runtime.allocate_in_arena(arena, &oversized).is_err());
    assert_eq!(runtime.arena_telemetry().live_bytes(), 0);
    assert_eq!(runtime.memory_telemetry().arena_bytes(), 0);
    assert_eq!(runtime.memory_telemetry().out_of_memory_failures(), 1);
}
