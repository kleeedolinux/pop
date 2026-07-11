use pop_runtime_native::{
    pop_rt_abi_major, pop_rt_abi_minor, pop_rt_allocate_array, pop_rt_allocate_object,
    pop_rt_allocate_table, pop_rt_array_get, pop_rt_array_set, pop_rt_field_get, pop_rt_field_set,
    pop_rt_gc_stage, pop_rt_release_root, pop_rt_retain_root,
};

#[test]
fn bootstrap_runtime_exports_a_stable_c_abi_identity() {
    assert_eq!(pop_rt_abi_major(), 1);
    assert_eq!(pop_rt_abi_minor(), 0);
    assert_eq!(pop_rt_gc_stage(), 1);
}

#[test]
fn bootstrap_abi_allocates_and_tracks_opaque_handles() {
    let reference = pop_rt_allocate_array(2, 0);
    assert_ne!(reference, 0);
    let root = pop_rt_retain_root(reference);
    assert_ne!(root, 0);
    assert_eq!(pop_rt_release_root(root), 1);
    assert_ne!(pop_rt_allocate_table(3), 0);
    assert_eq!(pop_rt_array_set(reference, 1, 41), 1);
    assert_eq!(pop_rt_array_get(reference, 1), 41);
    assert_eq!(pop_rt_array_get(reference, 3), 0);
    assert_eq!(pop_rt_array_set(reference, 3, 99), 0);

    let references = pop_rt_allocate_array(2, 1);
    let child = pop_rt_allocate_array(1, 0);
    assert_ne!(references, 0);
    assert_ne!(child, 0);
    assert_eq!(pop_rt_array_set(references, 1, child), 1);
    assert_eq!(pop_rt_array_get(references, 1), child);

    let object = pop_rt_allocate_object(2);
    assert_ne!(object, 0);
    assert_eq!(pop_rt_field_set(object, 1, 77), 1);
    assert_eq!(pop_rt_field_get(object, 1), 77);
}
