use pop_runtime_collector::{
    BackgroundWorkerConfig, BackgroundWorkerConfigError, BackgroundWorkerStartError,
    GenerationalRuntime,
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
fn worker_pool_can_be_attached_once_to_an_existing_configured_runtime() {
    let config = BackgroundWorkerConfig::new(2, 2).expect("worker configuration");
    let mut runtime = GenerationalRuntime::new();

    runtime
        .start_background_workers(config)
        .expect("start workers");
    assert_eq!(
        runtime.start_background_workers(config),
        Err(BackgroundWorkerStartError::AlreadyStarted)
    );
    assert_eq!(
        runtime
            .background_worker_telemetry()
            .expect("worker telemetry")
            .workers_started(),
        2
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
    assert!((1..=2).contains(&telemetry.worker_threads_used()));
    assert!(telemetry.worker_threads_used() == 2 || telemetry.jobs_stolen() > 0);
    assert!(telemetry.mark_jobs_completed() >= 64);
    assert!(telemetry.sweep_jobs_completed() >= 64);
    assert_eq!(telemetry.jobs_submitted(), telemetry.jobs_completed());
    assert!(telemetry.jobs_stolen() <= telemetry.jobs_completed());
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
fn production_profile_returns_with_mature_worker_work_in_flight() {
    let mut runtime = GenerationalRuntime::production_with_background_workers(
        BackgroundWorkerConfig::new(2, 8).expect("worker config"),
    )
    .expect("production collector workers");
    assert_eq!(
        runtime.contract(),
        pop_runtime_interface::GarbageCollectorContract::pop_v1()
    );
    let request = mature_object(&[0]);
    let owner = runtime.allocate_object(&request).expect("owner");
    let first = runtime
        .allocate_object(&mature_object(&[]))
        .expect("first target");
    let second = runtime
        .allocate_object(&mature_object(&[]))
        .expect("second target");
    runtime
        .store_reference(owner, ObjectSlot::new(0), Some(first))
        .expect("snapshot edge");
    let owner_root = runtime.retain_root(owner).expect("owner root");
    let mut roots = no_stack_roots(90);
    runtime.request_major_collection();

    let first_slice = runtime.safe_point(&mut roots).expect("dispatch mark slice");
    assert!(first_slice.collection().is_none());
    assert!(
        runtime.background_work_in_flight(),
        "safe point must return while immutable mark work remains on host workers"
    );
    runtime
        .store_reference(owner, ObjectSlot::new(0), Some(second))
        .expect("mutator store while mark batch is in flight");

    finish_major(&mut runtime, &mut roots);
    assert!(runtime.contains(first), "SATB snapshot edge survives");
    assert!(runtime.contains(second), "post-scan edge survives");
    assert!(!runtime.background_work_in_flight());
    runtime
        .release_root(owner_root)
        .expect("release owner root");
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

#[test]
fn production_card_refinement_restarts_after_an_overlapped_mutator_store() {
    let config = BackgroundWorkerConfig::new(2, 2).expect("worker configuration");
    let mut runtime =
        GenerationalRuntime::production_with_background_workers(config).expect("production");
    let owner = runtime
        .allocate_object(&mature_object(&[0]))
        .expect("mature owner");
    let first = runtime
        .allocate_object(&young_object())
        .expect("first young child");
    let second = runtime
        .allocate_object(&young_object())
        .expect("second young child");
    runtime
        .store_reference(owner, ObjectSlot::new(0), Some(first))
        .expect("first mature-to-young edge");
    let owner_root = runtime.retain_root(owner).expect("owner root");
    let mut roots = no_stack_roots(91);
    runtime.request_minor_collection();

    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("dispatch card refinement")
            .collection()
            .is_none()
    );
    assert!(runtime.background_work_in_flight());
    runtime
        .store_reference(owner, ObjectSlot::new(0), Some(second))
        .expect("overlapped card mutation");
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("discard stale card result and redispatch")
            .collection()
            .is_none()
    );
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("collect with current refined card")
            .collection()
            .is_some()
    );
    assert_eq!(runtime.object_count(), 2);
    assert!(!runtime.contains(first));
    assert!(!runtime.contains(second));
    runtime.release_root(owner_root).expect("release owner");
}

#[test]
fn production_overlap_stress_preserves_the_latest_edge_across_restarted_refinement() {
    let config = BackgroundWorkerConfig::new(2, 4).expect("worker configuration");
    let mut runtime =
        GenerationalRuntime::production_with_background_workers(config).expect("production");
    let owner = runtime
        .allocate_object(&mature_object(&[0]))
        .expect("mature owner");
    let owner_root = runtime.retain_root(owner).expect("owner root");
    let mut roots = no_stack_roots(92);

    for cycle in 0..64 {
        let first = runtime
            .allocate_object(&young_object())
            .expect("first young child");
        let second = runtime
            .allocate_object(&young_object())
            .expect("second young child");
        runtime
            .store_reference(owner, ObjectSlot::new(0), Some(first))
            .expect("first edge");
        runtime.request_minor_collection();
        assert!(
            runtime
                .safe_point(&mut roots)
                .expect("dispatch refinement")
                .collection()
                .is_none(),
            "cycle {cycle} must return with refinement in flight"
        );
        runtime
            .store_reference(owner, ObjectSlot::new(0), Some(second))
            .expect("overlapped edge");

        let mut collected = false;
        for _ in 0..4 {
            if runtime
                .safe_point(&mut roots)
                .expect("complete restarted refinement")
                .collection()
                .is_some()
            {
                collected = true;
                break;
            }
        }
        assert!(collected, "cycle {cycle} exceeded the bounded retry count");
        let current = runtime
            .load_slot_value(owner, ObjectSlot::new(0))
            .expect("latest edge");
        assert_ne!(current, first.raw());
        assert_ne!(current, second.raw());
        assert!(runtime.contains(pop_runtime_interface::ManagedReference::new(current)));
    }

    assert!(!runtime.background_work_in_flight());
    let telemetry = runtime
        .background_worker_telemetry()
        .expect("worker telemetry");
    assert_eq!(telemetry.jobs_submitted(), telemetry.jobs_completed());
    runtime.release_root(owner_root).expect("release owner");
}
