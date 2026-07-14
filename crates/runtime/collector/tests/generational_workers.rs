use pop_runtime_collector::{
    BackgroundWorkerConfig, BackgroundWorkerConfigError, GenerationalRuntime,
};
use pop_runtime_interface::{
    AllocationClass, ObjectAllocationRequest, ObjectMap, ObjectSlot, RootPublication,
    RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn mature_object(reference_slots: &[u32]) -> ObjectAllocationRequest {
    let slot_count = reference_slots
        .iter()
        .copied()
        .max()
        .map_or(0, |maximum| maximum + 1);
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(71),
        AllocationClass::Mature,
        ObjectMap::new(
            slot_count,
            reference_slots
                .iter()
                .copied()
                .map(ObjectSlot::new)
                .collect(),
        )
        .expect("object map"),
    )
}

fn young_object() -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(72),
        AllocationClass::NurseryEligible,
        ObjectMap::new(0, Vec::new()).expect("young object map"),
    )
}

fn no_stack_roots(id: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), Vec::new()).expect("stack map"),
        Vec::new(),
    )
    .expect("root publication")
}

fn finish_major(runtime: &mut GenerationalRuntime, roots: &mut RootPublication) {
    for _ in 0..256 {
        if runtime
            .safe_point(roots)
            .expect("background major slice")
            .collection()
            .is_some()
        {
            return;
        }
    }
    panic!("background major collection exceeded its deterministic slice bound");
}

#[test]
fn background_worker_configuration_rejects_zero_or_unbounded_geometry() {
    assert_eq!(
        BackgroundWorkerConfig::new(0, 1),
        Err(BackgroundWorkerConfigError::ZeroWorkers)
    );
    assert_eq!(
        BackgroundWorkerConfig::new(1, 0),
        Err(BackgroundWorkerConfigError::ZeroQueueCapacity)
    );
}

#[test]
fn background_workers_scan_and_sweep_without_changing_reachability() {
    let config = BackgroundWorkerConfig::new(2, 4).expect("worker configuration");
    let mut runtime =
        GenerationalRuntime::with_background_workers(config).expect("background workers");
    let leaf = runtime.allocate_object(&mature_object(&[])).expect("leaf");
    let mut previous = leaf;
    for _ in 0..63 {
        let current = runtime
            .allocate_object(&mature_object(&[0]))
            .expect("chain object");
        runtime
            .store_reference(current, ObjectSlot::new(0), Some(previous))
            .expect("chain edge");
        previous = current;
    }
    let root = runtime.retain_root(previous).expect("chain root");
    let mut roots = no_stack_roots(1);

    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert_eq!(runtime.object_count(), 64);

    runtime.release_root(root).expect("release chain root");
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert_eq!(runtime.object_count(), 0);

    let telemetry = runtime
        .background_worker_telemetry()
        .expect("worker telemetry");
    assert_eq!(telemetry.workers_started(), 2);
    assert_eq!(telemetry.worker_threads_used(), 2);
    assert!(telemetry.mark_jobs_completed() >= 64);
    assert!(telemetry.sweep_jobs_completed() >= 64);
    assert_eq!(telemetry.jobs_submitted(), telemetry.jobs_completed());
    assert!(telemetry.maximum_batch_size() <= 64);
}

#[test]
fn repeated_worker_pool_shutdown_does_not_leave_live_jobs() {
    let config = BackgroundWorkerConfig::new(2, 1).expect("worker configuration");
    for _ in 0..16 {
        let runtime =
            GenerationalRuntime::with_background_workers(config).expect("background workers");
        let telemetry = runtime
            .background_worker_telemetry()
            .expect("worker telemetry");
        assert_eq!(telemetry.jobs_submitted(), 0);
        assert_eq!(telemetry.jobs_completed(), 0);
        drop(runtime);
    }
}

#[test]
fn dirty_cards_are_refined_by_workers_before_minor_evacuation() {
    let config = BackgroundWorkerConfig::new(2, 2).expect("worker configuration");
    let mut runtime =
        GenerationalRuntime::with_background_workers(config).expect("background workers");
    let owner = runtime
        .allocate_object(&mature_object(&[0]))
        .expect("mature owner");
    let young = runtime
        .allocate_object(&young_object())
        .expect("young child");
    runtime
        .store_reference(owner, ObjectSlot::new(0), Some(young))
        .expect("mature-to-young edge");
    let owner_root = runtime.retain_root(owner).expect("owner root");
    let mut roots = no_stack_roots(2);

    runtime.request_minor_collection();
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("minor collection")
            .collection()
            .is_some()
    );

    assert_eq!(runtime.object_count(), 2);
    assert!(!runtime.contains(young));
    let telemetry = runtime
        .background_worker_telemetry()
        .expect("worker telemetry");
    assert_eq!(telemetry.card_refinement_jobs_completed(), 1);
    assert_eq!(telemetry.jobs_submitted(), telemetry.jobs_completed());
    runtime
        .release_root(owner_root)
        .expect("release owner root");
}
