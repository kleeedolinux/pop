use pop_runtime_collector::{CollectorGeneration, RelocationRuntime};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot,
    RootPublication, RootSlot, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

fn object(class: AllocationClass, slots: u32, references: &[u32]) -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(1),
        class,
        ObjectMap::new(
            slots,
            references.iter().copied().map(ObjectSlot::new).collect(),
        )
        .expect("object map"),
    )
}

fn roots(id: u32, values: Vec<Option<ManagedReference>>) -> RootPublication {
    RootPublication::new(
        StackMap::new(
            SafePointId::new(id),
            (0..values.len())
                .map(|slot| RootSlot::new(u32::try_from(slot).expect("root slot")))
                .collect(),
        )
        .expect("stack map"),
        values,
    )
    .expect("root publication")
}

fn force_minor(runtime: &mut RelocationRuntime, roots: &mut RootPublication) {
    runtime.request_minor_collection();
    assert!(
        runtime
            .safe_point(roots)
            .expect("minor collection")
            .collection()
            .is_some()
    );
}

#[test]
fn minor_collection_copies_live_graphs_updates_roots_and_rejects_old_tokens() {
    let mut runtime = RelocationRuntime::new();
    let request = object(AllocationClass::NurseryEligible, 1, &[0]);
    let parent = runtime.allocate_object(&request).expect("parent");
    let child = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("child");
    let garbage = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("garbage");
    let parent_identity = runtime.object_identity(parent).expect("parent identity");
    runtime
        .store_reference(parent, ObjectSlot::new(0), Some(child))
        .expect("parent edge");
    let mut publication = roots(1, vec![Some(parent)]);

    force_minor(&mut runtime, &mut publication);

    let relocated_parent = publication
        .managed_references()
        .next()
        .expect("relocated parent");
    let relocated_child = runtime
        .load_reference(relocated_parent, ObjectSlot::new(0))
        .expect("relocated child slot")
        .expect("child remains reachable");
    assert_ne!(relocated_parent, parent);
    assert_eq!(
        runtime.object_identity(relocated_parent),
        Some(parent_identity)
    );
    assert_ne!(relocated_child, child);
    assert!(!runtime.contains(parent));
    assert!(!runtime.contains(child));
    assert!(!runtime.contains(garbage));
    assert_eq!(runtime.object_count(), 2);
    assert!(runtime.load_reference(parent, ObjectSlot::new(0)).is_err());
}

#[test]
fn strong_handles_and_pins_follow_relocated_targets() {
    let mut runtime = RelocationRuntime::new();
    let request = object(AllocationClass::NurseryEligible, 0, &[]);
    let rooted = runtime.allocate_object(&request).expect("rooted object");
    let root = runtime.retain_root(rooted).expect("strong root");
    let mut no_roots = roots(2, Vec::new());

    force_minor(&mut runtime, &mut no_roots);
    assert!(!runtime.contains(rooted));
    assert_eq!(runtime.object_count(), 1);
    force_minor(&mut runtime, &mut no_roots);
    assert_eq!(runtime.object_count(), 1);
    runtime.release_root(root).expect("release updated root");
    force_minor(&mut runtime, &mut no_roots);
    assert_eq!(runtime.object_count(), 1);

    let pinned = runtime.allocate_object(&request).expect("pinned object");
    let pin = runtime.pin(pinned).expect("pin");
    force_minor(&mut runtime, &mut no_roots);
    assert!(!runtime.contains(pinned));
    assert_eq!(runtime.object_count(), 2);
    force_minor(&mut runtime, &mut no_roots);
    assert_eq!(runtime.object_count(), 2);
    runtime.unpin(pin).expect("release updated pin");
    force_minor(&mut runtime, &mut no_roots);
    assert_eq!(runtime.object_count(), 2);
}

#[test]
fn survivor_age_promotes_deterministically_and_mature_tokens_stop_moving() {
    let mut runtime = RelocationRuntime::new();
    let young = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("young object");
    let mut publication = roots(3, vec![Some(young)]);

    force_minor(&mut runtime, &mut publication);
    let first_survivor = publication.managed_references().next().expect("survivor");
    assert_eq!(
        runtime.generation(first_survivor),
        Some(CollectorGeneration::Nursery { age: 1 })
    );

    force_minor(&mut runtime, &mut publication);
    let promoted = publication.managed_references().next().expect("promoted");
    assert_ne!(promoted, first_survivor);
    assert_eq!(
        runtime.generation(promoted),
        Some(CollectorGeneration::Mature)
    );

    force_minor(&mut runtime, &mut publication);
    assert_eq!(publication.managed_references().next(), Some(promoted));
}

#[test]
fn remembered_cards_preserve_mature_to_young_edges_only_as_needed() {
    let mut runtime = RelocationRuntime::new();
    let mature = runtime
        .allocate_object(&object(AllocationClass::Mature, 2, &[0]))
        .expect("mature owner");
    let young = runtime
        .allocate_object(&object(AllocationClass::NurseryEligible, 0, &[]))
        .expect("young child");
    runtime
        .store_scalar(mature, ObjectSlot::new(1), young.raw())
        .expect("scalar resembling reference");
    assert_eq!(runtime.dirty_card_count(), 0);
    runtime
        .store_reference(mature, ObjectSlot::new(0), Some(young))
        .expect("mature to young edge");
    assert_eq!(runtime.dirty_card_count(), 1);
    let mut no_roots = roots(4, Vec::new());

    force_minor(&mut runtime, &mut no_roots);
    let relocated = runtime
        .load_reference(mature, ObjectSlot::new(0))
        .expect("remembered slot")
        .expect("remembered child");
    assert_ne!(relocated, young);
    assert!(runtime.contains(relocated));
    assert_eq!(runtime.dirty_card_count(), 1);

    runtime
        .store_reference(mature, ObjectSlot::new(0), None)
        .expect("clear remembered edge");
    force_minor(&mut runtime, &mut no_roots);
    assert!(!runtime.contains(relocated));
    assert_eq!(runtime.dirty_card_count(), 0);
}
