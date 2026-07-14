use pop_runtime_collector::{
    AllocationInfrastructureConfig, GenerationalMemoryConfig, GenerationalMemoryConfigError,
    GenerationalRuntime, MajorCollectorConfig, NonHeapMemoryUsage, NonHeapMemoryUsageError,
};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ObjectAllocationRequest, ObjectMap,
    PanicKind, RootPublication, RuntimeAdapter, RuntimeFailure, RuntimeTypeId, SafePointId,
    StackMap, TableAllocationRequest, UnwindReason,
};

fn object(class: AllocationClass, slots: u32) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(91),
        class,
        ObjectMap::new(slots, Vec::new()).expect("object map"),
    )
}

fn no_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

fn memory_config(hard_limit_bytes: usize) -> GenerationalMemoryConfig {
    GenerationalMemoryConfig::new(hard_limit_bytes, 32, 64, 64, 100, 1)
        .expect("memory configuration")
}

fn runtime(hard_limit_bytes: usize) -> GenerationalRuntime {
    GenerationalRuntime::with_memory_config(
        MajorCollectorConfig::new(8),
        AllocationInfrastructureConfig::new(64, 256, 32).expect("allocation geometry"),
        memory_config(hard_limit_bytes),
    )
}

#[test]
fn memory_configuration_rejects_unusable_limits_and_unbounded_assists() {
    assert_eq!(
        GenerationalMemoryConfig::new(0, 0, 0, 0, 100, 1),
        Err(GenerationalMemoryConfigError::ZeroHardLimit)
    );
    assert_eq!(
        GenerationalMemoryConfig::new(128, 64, 64, 1, 100, 1),
        Err(GenerationalMemoryConfigError::ReserveExhaustsLimit)
    );
    assert_eq!(
        GenerationalMemoryConfig::new(128, 8, 8, 113, 100, 1),
        Err(GenerationalMemoryConfigError::HeadroomExceedsOrdinaryLimit)
    );
    assert_eq!(
        GenerationalMemoryConfig::new(128, 8, 8, 16, 0, 1),
        Err(GenerationalMemoryConfigError::ZeroGrowthPercent)
    );
    assert_eq!(
        GenerationalMemoryConfig::new(128, 8, 8, 16, 100, 0),
        Err(GenerationalMemoryConfigError::ZeroAssistBudget)
    );
    assert_eq!(
        NonHeapMemoryUsage::new(usize::MAX, 1, 0, 0, 0, 0),
        Err(NonHeapMemoryUsageError::TotalOverflow)
    );
}

#[test]
fn default_memory_policy_reserves_startup_headroom_for_small_live_heaps() {
    let config = GenerationalMemoryConfig::default();
    let runtime = GenerationalRuntime::new();

    assert_eq!(config.minimum_headroom_bytes(), 16 * 1024 * 1024);
    assert_eq!(
        runtime.memory_telemetry().current_target_bytes(),
        16 * 1024 * 1024
    );
}

#[test]
fn hard_limit_protects_emergency_and_evacuation_reserves_before_mutation() {
    let mut runtime = runtime(256);
    let request = object(AllocationClass::Large, 0);
    runtime
        .allocate_object(&request)
        .expect("first dedicated page");
    runtime
        .allocate_object(&request)
        .expect("second dedicated page");

    let first = runtime
        .allocate_object(&request)
        .expect_err("third page would consume protected reserves");
    let second = runtime
        .allocate_object(&request)
        .expect_err("the same pressure state has the same failure");
    assert_eq!(first, second);
    assert_eq!(
        first,
        RuntimeFailure::Unwind(UnwindReason::Panic(
            pop_runtime_interface::PanicPayload::new(PanicKind::OutOfMemory {
                requested_objects: 1,
                requested_slots: 0,
            })
        ))
    );
    assert_eq!(runtime.object_count(), 2, "failed allocation is atomic");

    let telemetry = runtime.memory_telemetry();
    assert_eq!(telemetry.hard_limit_bytes(), 256);
    assert_eq!(telemetry.ordinary_limit_bytes(), 160);
    assert_eq!(telemetry.committed_bytes(), 128);
    assert_eq!(telemetry.emergency_reserve_bytes(), 32);
    assert_eq!(telemetry.evacuation_reserve_bytes(), 64);
    assert_eq!(telemetry.out_of_memory_failures(), 2);
    assert!(telemetry.allocation_pressure_events() >= 2);
    assert!(telemetry.allocation_debt_bytes() > 0);
    assert_eq!(telemetry.peak_committed_bytes(), 128);
    assert!(telemetry.major_collection_requests() >= 1);
}

#[test]
fn pressure_collection_reclaims_pages_and_restores_allocation_capacity() {
    let mut runtime = runtime(256);
    let request = object(AllocationClass::Large, 0);
    runtime
        .allocate_object(&request)
        .expect("first garbage page");
    runtime
        .allocate_object(&request)
        .expect("second garbage page");
    runtime
        .allocate_object(&request)
        .expect_err("pressure requests collection before deterministic OOM");

    let mut roots = no_roots(1);
    let outcome = runtime
        .safe_point(&mut roots)
        .expect("service pressure collection");
    assert!(outcome.collection().is_some());
    assert_eq!(runtime.object_count(), 0);
    assert_eq!(runtime.memory_telemetry().committed_bytes(), 0);
    assert_eq!(runtime.memory_telemetry().allocation_debt_bytes(), 0);
    assert!(runtime.memory_telemetry().pages_returned() >= 2);

    runtime
        .allocate_object(&request)
        .expect("reclaimed capacity can be reused");
}

#[test]
fn allocation_pressure_performs_only_the_configured_major_assist_budget() {
    let mut runtime = runtime(512);
    let request = object(AllocationClass::Large, 0);
    for _ in 0..3 {
        runtime.allocate_object(&request).expect("dedicated page");
    }
    let roots = no_roots(2);
    runtime
        .start_major_collection(&roots)
        .expect("active mature snapshot");

    runtime
        .allocate_object(&request)
        .expect("pressure allocation after one bounded assist");

    let telemetry = runtime.memory_telemetry();
    assert_eq!(telemetry.mutator_assist_slices(), 1);
    assert_eq!(telemetry.mutator_assist_work_units(), 1);
    assert!(telemetry.committed_bytes() <= telemetry.ordinary_limit_bytes());
}

#[test]
fn telemetry_reports_live_bytes_by_allocation_domain_and_adaptive_target() {
    let mut runtime = runtime(512);
    runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 2))
        .expect("local allocation");
    runtime
        .allocate_object(&object(AllocationClass::Large, 4))
        .expect("large allocation");
    runtime
        .allocate_object(&object(AllocationClass::Pinned, 1))
        .expect("pinned allocation");

    let telemetry = runtime.memory_telemetry();
    assert_eq!(telemetry.live_bytes(), 16 + 32 + 8);
    assert_eq!(telemetry.local_bytes(), 16);
    assert_eq!(telemetry.large_object_bytes(), 32);
    assert_eq!(telemetry.pinned_bytes(), 8);
    assert!(telemetry.current_target_bytes() >= telemetry.live_bytes());
    assert!(telemetry.current_target_bytes() <= telemetry.ordinary_limit_bytes());
}

#[test]
fn arrays_and_tables_use_the_same_atomic_byte_admission_contract() {
    let mut runtime = runtime(256);
    let array = ArrayAllocationRequest::new(
        RuntimeTypeId::new(92),
        AllocationClass::Large,
        8,
        ArrayElementMap::ManagedReference,
    );
    runtime.allocate_array(&array).expect("first array page");
    runtime.allocate_array(&array).expect("second array page");
    let failure = runtime
        .allocate_array(&array)
        .expect_err("third array would consume protected reserves");
    assert_eq!(
        failure,
        RuntimeFailure::Unwind(UnwindReason::Panic(
            pop_runtime_interface::PanicPayload::new(PanicKind::OutOfMemory {
                requested_objects: 1,
                requested_slots: 8,
            })
        ))
    );
    assert_eq!(runtime.object_count(), 2);

    let table = TableAllocationRequest::new(
        RuntimeTypeId::new(93),
        AllocationClass::Large,
        4,
        ArrayElementMap::ManagedReference,
        ArrayElementMap::ManagedReference,
    )
    .expect("table layout");
    assert_eq!(
        runtime
            .allocate_table(&table)
            .expect_err("table uses the same committed-byte limit"),
        failure
    );
    assert_eq!(runtime.object_count(), 2);
}

#[test]
fn non_heap_domains_reduce_capacity_and_failed_snapshots_are_atomic() {
    let mut runtime = runtime(256);
    let usage = NonHeapMemoryUsage::new(8, 8, 8, 8, 4, 4).expect("non-heap usage");
    runtime
        .set_non_heap_memory_usage(usage)
        .expect("account non-heap memory");
    assert_eq!(runtime.memory_telemetry().ordinary_limit_bytes(), 120);
    assert_eq!(runtime.memory_telemetry().non_heap_bytes(), 40);
    assert_eq!(runtime.memory_telemetry().stack_bytes(), 8);

    let request = object(AllocationClass::Large, 0);
    runtime
        .allocate_object(&request)
        .expect("one dedicated page");
    runtime
        .allocate_object(&request)
        .expect_err("second page plus non-heap usage exceeds ordinary capacity");

    let rejected = NonHeapMemoryUsage::new(120, 0, 0, 0, 0, 0).expect("large snapshot");
    assert_eq!(
        runtime
            .set_non_heap_memory_usage(rejected)
            .expect_err("snapshot cannot consume the reserves"),
        RuntimeFailure::Unwind(UnwindReason::Panic(
            pop_runtime_interface::PanicPayload::new(PanicKind::OutOfMemory {
                requested_objects: 0,
                requested_slots: 0,
            })
        ))
    );
    assert_eq!(runtime.memory_telemetry().non_heap_bytes(), 40);
}
