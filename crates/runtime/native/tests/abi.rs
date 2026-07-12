use pop_runtime_native::{
    abi_safe_point, allocate_mapped_object, allocate_utf8_string_literal, pop_rt_abi_major,
    pop_rt_abi_minor, pop_rt_allocate_array, pop_rt_allocate_object, pop_rt_allocate_table,
    pop_rt_array_get, pop_rt_array_set, pop_rt_field_get, pop_rt_field_set, pop_rt_gc_stage,
    pop_rt_release_root, pop_rt_retain_root, pop_rt_string_equal, request_abi_collection,
};
use std::sync::{Mutex, MutexGuard, OnceLock};

fn abi_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("ABI test lock")
}

#[test]
fn bootstrap_runtime_exports_a_stable_c_abi_identity() {
    let _guard = abi_test_lock();
    assert_eq!(pop_rt_abi_major(), 1);
    assert_eq!(pop_rt_abi_minor(), 0);
    assert_eq!(pop_rt_gc_stage(), 1);
}

#[test]
fn bootstrap_strings_preserve_valid_utf8_and_compare_by_value() {
    let _guard = abi_test_lock();
    let pop = "Pop 🫧".as_bytes();
    let lua = "Lua".as_bytes();
    let first = allocate_utf8_string_literal(pop);
    let second = allocate_utf8_string_literal(pop);
    let different = allocate_utf8_string_literal(lua);
    assert_ne!(first, 0);
    assert_ne!(second, 0);
    assert_ne!(different, 0);
    assert_eq!(pop_rt_string_equal(first, second), 1);
    assert_eq!(pop_rt_string_equal(first, different), 0);

    let invalid = [0xff_u8];
    assert_eq!(allocate_utf8_string_literal(&invalid), 0);
}

#[test]
fn bootstrap_abi_allocates_and_tracks_opaque_handles() {
    let _guard = abi_test_lock();
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

#[test]
fn mapped_object_abi_preserves_precise_reference_slots() {
    let _guard = abi_test_lock();
    let parent = allocate_mapped_object(2, &[1]);
    let child = pop_rt_allocate_object(0);
    assert_ne!(parent, 0);
    assert_ne!(child, 0);
    assert_eq!(pop_rt_field_set(parent, 1, 77), 1);
    assert_eq!(pop_rt_field_set(parent, 2, child), 1);
    assert_eq!(pop_rt_field_get(parent, 1), 77);
    assert_eq!(pop_rt_field_get(parent, 2), child);
    assert_eq!(pop_rt_field_set(parent, 2, u64::MAX), 0);
}

#[test]
fn abi_safe_points_publish_exact_transitive_roots() {
    let _guard = abi_test_lock();
    let parent = allocate_mapped_object(1, &[0]);
    let child = pop_rt_allocate_object(0);
    assert_eq!(pop_rt_field_set(parent, 1, child), 1);

    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(1, &[parent]), 1);
    assert_eq!(abi_safe_point(2, &[child]), 1);

    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(3, &[]), 1);
    assert_eq!(abi_safe_point(4, &[child]), 0);
}
