use pop_runtime_collector::{
    BackgroundWorkerConfig, GenerationalRuntime, MajorCollectorConfig, MajorCyclePhase,
};
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ObjectAllocationRequest, ObjectMap,
    ObjectSlot, RootPublication, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn mature_leaf(type_id: u32) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(type_id),
        AllocationClass::Mature,
        ObjectMap::new(0, Vec::new()).expect("leaf object map"),
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
    for _ in 0..2_048 {
        if runtime
            .safe_point(roots)
            .expect("large-object major slice")
            .collection()
            .is_some()
        {
            return;
        }
    }
    panic!("large-object major collection exceeded its deterministic slice bound");
}

#[test]
fn zero_large_object_scan_geometry_normalizes_to_bounded_progress() {
    let config = MajorCollectorConfig::with_large_object_scan_chunk_slots(0, 0);
    assert_eq!(config.work_budget(), 1);
    assert_eq!(config.large_object_scan_chunk_slots(), 1);
}

#[test]
fn large_pointer_array_scanning_is_chunked_across_safe_points() {
    let config = MajorCollectorConfig::with_large_object_scan_chunk_slots(1, 2);
    let mut runtime = GenerationalRuntime::with_config(config);
    let mut leaves = Vec::new();
    for type_id in 101..106 {
        leaves.push(
            runtime
                .allocate_object(&mature_leaf(type_id))
                .expect("mature leaf"),
        );
    }
    let array = runtime
        .allocate_array(&ArrayAllocationRequest::new(
            RuntimeTypeId::new(106),
            AllocationClass::Large,
            5,
            ArrayElementMap::ManagedReference,
        ))
        .expect("large pointer array");
    for (index, leaf) in leaves.iter().copied().enumerate() {
        runtime
            .store_reference(
                array,
                ObjectSlot::new(u32::try_from(index).expect("small test index")),
                Some(leaf),
            )
            .expect("large array edge");
    }
    let root = runtime.retain_root(array).expect("large array root");
    let mut roots = no_stack_roots(1);

    runtime.request_major_collection();
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("discover large array")
            .collection()
            .is_none()
    );
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Marking);
    assert_eq!(
        runtime
            .major_collection_telemetry()
            .large_object_scan_chunks_completed(),
        0
    );

    let mut previous_chunks = 0;
    let mut interleaved_ordinary_mark_work = false;
    for _ in 0..16 {
        assert!(
            runtime
                .safe_point(&mut roots)
                .expect("advance one bounded mark work item")
                .collection()
                .is_none()
        );
        let chunks = runtime
            .major_collection_telemetry()
            .large_object_scan_chunks_completed();
        assert!(chunks.saturating_sub(previous_chunks) <= 1);
        if chunks == previous_chunks && chunks > 0 && chunks < 3 {
            interleaved_ordinary_mark_work = true;
        }
        previous_chunks = chunks;
        if chunks == 3 {
            break;
        }
    }
    assert_eq!(previous_chunks, 3);
    assert!(interleaved_ordinary_mark_work);
    assert_eq!(
        runtime
            .major_collection_telemetry()
            .maximum_large_object_scan_chunk_slots(),
        2
    );
    assert_eq!(
        runtime
            .major_collection_telemetry()
            .maximum_pending_large_object_scan_chunks(),
        1
    );

    finish_major(&mut runtime, &mut roots);
    assert!(runtime.contains(array));
    assert!(leaves.iter().all(|reference| runtime.contains(*reference)));

    runtime
        .release_root(root)
        .expect("release large array root");
    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert_eq!(runtime.object_count(), 0);
}

#[test]
fn pointer_free_large_objects_require_no_scan_chunks_after_liveness() {
    let config = MajorCollectorConfig::with_large_object_scan_chunk_slots(1, 2);
    let mut runtime = GenerationalRuntime::with_config(config);
    let blob = runtime
        .allocate_array(&ArrayAllocationRequest::new(
            RuntimeTypeId::new(107),
            AllocationClass::Large,
            4_096,
            ArrayElementMap::Scalar,
        ))
        .expect("large pointer-free blob");
    let root = runtime.retain_root(blob).expect("blob root");
    let mut roots = no_stack_roots(2);

    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);

    let telemetry = runtime.major_collection_telemetry();
    assert_eq!(telemetry.pointer_free_large_objects_seen(), 1);
    assert_eq!(telemetry.large_object_scan_chunks_completed(), 0);
    assert_eq!(telemetry.maximum_large_object_scan_chunk_slots(), 0);
    assert!(runtime.contains(blob));
    runtime.release_root(root).expect("release blob root");
}

#[test]
fn oversized_mature_pointer_layouts_cannot_bypass_the_chunk_budget() {
    let config = MajorCollectorConfig::with_large_object_scan_chunk_slots(1, 2);
    let mut runtime = GenerationalRuntime::with_config(config);
    let object = runtime
        .allocate_object(&ObjectAllocationRequest::new(
            RuntimeTypeId::new(113),
            AllocationClass::Mature,
            ObjectMap::new(5, (0..5).map(ObjectSlot::new).collect()).expect("pointer layout"),
        ))
        .expect("oversized mature pointer layout");
    let root = runtime.retain_root(object).expect("object root");
    let mut roots = no_stack_roots(5);

    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);

    let telemetry = runtime.major_collection_telemetry();
    assert_eq!(telemetry.large_object_scan_chunks_completed(), 3);
    assert_eq!(telemetry.maximum_large_object_scan_chunk_slots(), 2);
    runtime.release_root(root).expect("release object root");
}

#[test]
fn mutations_between_large_object_chunks_preserve_snapshot_and_new_edges() {
    let config = MajorCollectorConfig::with_large_object_scan_chunk_slots(1, 2);
    let mut runtime = GenerationalRuntime::with_config(config);
    let snapshot_target = runtime
        .allocate_object(&mature_leaf(110))
        .expect("snapshot target");
    let new_target = runtime
        .allocate_object(&mature_leaf(111))
        .expect("new target");
    let array = runtime
        .allocate_array(&ArrayAllocationRequest::new(
            RuntimeTypeId::new(112),
            AllocationClass::Large,
            4,
            ArrayElementMap::ManagedReference,
        ))
        .expect("large pointer array");
    runtime
        .store_reference(array, ObjectSlot::new(3), Some(snapshot_target))
        .expect("snapshot edge");
    let root = runtime.retain_root(array).expect("large array root");
    let mut roots = no_stack_roots(4);

    runtime.request_major_collection();
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("discover large array")
            .collection()
            .is_none()
    );
    assert!(
        runtime
            .safe_point(&mut roots)
            .expect("scan first chunk")
            .collection()
            .is_none()
    );
    runtime
        .store_reference(array, ObjectSlot::new(3), None)
        .expect("overwrite unscanned snapshot edge");
    runtime
        .store_reference(array, ObjectSlot::new(0), Some(new_target))
        .expect("publish new edge into scanned chunk");

    finish_major(&mut runtime, &mut roots);
    assert!(runtime.contains(snapshot_target));
    assert!(runtime.contains(new_target));

    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);
    assert!(!runtime.contains(snapshot_target));
    assert!(runtime.contains(new_target));
    runtime
        .release_root(root)
        .expect("release large array root");
}

#[test]
fn background_workers_receive_bounded_large_object_scan_chunks() {
    let workers = BackgroundWorkerConfig::new(2, 64).expect("worker configuration");
    let mut runtime =
        GenerationalRuntime::with_background_workers(workers).expect("background workers");
    let mut leaves = Vec::new();
    for _ in 0..513 {
        leaves.push(
            runtime
                .allocate_object(&mature_leaf(108))
                .expect("mature leaf"),
        );
    }
    let array = runtime
        .allocate_array(&ArrayAllocationRequest::new(
            RuntimeTypeId::new(109),
            AllocationClass::Large,
            513,
            ArrayElementMap::ManagedReference,
        ))
        .expect("large pointer array");
    for (index, leaf) in leaves.iter().copied().enumerate() {
        runtime
            .store_reference(
                array,
                ObjectSlot::new(u32::try_from(index).expect("small test index")),
                Some(leaf),
            )
            .expect("large array edge");
    }
    let root = runtime.retain_root(array).expect("large array root");
    let mut roots = no_stack_roots(3);

    runtime.request_major_collection();
    finish_major(&mut runtime, &mut roots);

    let major = runtime.major_collection_telemetry();
    assert_eq!(major.large_object_scan_chunks_completed(), 3);
    assert_eq!(major.maximum_large_object_scan_chunk_slots(), 256);
    let worker = runtime
        .background_worker_telemetry()
        .expect("worker telemetry");
    assert_eq!(worker.jobs_submitted(), worker.jobs_completed());
    assert!(worker.maximum_batch_size() <= 64);
    assert!(runtime.contains(array));
    assert!(leaves.iter().all(|reference| runtime.contains(*reference)));
    runtime
        .release_root(root)
        .expect("release large array root");
}
