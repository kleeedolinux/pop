use pop_runtime_native::{
    abi_safe_point, allocate_mapped_object, allocate_platform_arguments,
    allocate_process_arguments, allocate_utf8_string_literal, pop_rt_abi_major, pop_rt_abi_minor,
    pop_rt_allocate_array, pop_rt_allocate_array_filled, pop_rt_allocate_object,
    pop_rt_allocate_table, pop_rt_array_fill, pop_rt_array_get, pop_rt_array_get_checked,
    pop_rt_array_length, pop_rt_array_set, pop_rt_field_get, pop_rt_field_set, pop_rt_gc_stage,
    pop_rt_pin, pop_rt_release_root, pop_rt_retain_root, pop_rt_string_concat, pop_rt_string_equal,
    pop_rt_string_format, pop_rt_string_read, pop_rt_unpin, request_abi_collection,
};
use pop_runtime_native_abi::StringFormatTag;
use std::ffi::CString;
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
    assert_eq!(pop_rt_abi_minor(), 5);
    assert_eq!(pop_rt_gc_stage(), 1);
}

#[test]
#[allow(unsafe_code)]
fn bulk_scalar_array_operations_distinguish_zero_values_from_failure() {
    let _guard = abi_test_lock();
    let array = pop_rt_allocate_array_filled(4, 0, 7);
    assert_ne!(array, 0);

    let mut length = u64::MAX;
    let mut value = u64::MAX;
    // SAFETY: Both output pointers address live writable `u64` values.
    unsafe {
        assert_eq!(pop_rt_array_length(array, &raw mut length), 1);
        assert_eq!(pop_rt_array_get_checked(array, 1, &raw mut value), 1);
    }
    assert_eq!(length, 4);
    assert_eq!(value, 7);

    assert_eq!(pop_rt_array_fill(array, 0), 1);
    // SAFETY: `value` remains a live writable `u64`.
    unsafe {
        assert_eq!(pop_rt_array_get_checked(array, 4, &raw mut value), 1);
        assert_eq!(pop_rt_array_get_checked(array, 5, &raw mut value), 0);
    }
    assert_eq!(value, 0);
}

#[test]
#[allow(unsafe_code)]
fn array_operations_reject_non_array_allocations() {
    let _guard = abi_test_lock();
    let object = pop_rt_allocate_object(2);
    assert_ne!(object, 0);
    assert_eq!(pop_rt_array_set(object, 1, 9), 0);
    assert_eq!(pop_rt_array_get(object, 1), 0);
    assert_eq!(pop_rt_array_fill(object, 9), 0);
    let mut output = 0;
    // SAFETY: `output` remains live and writable for both ABI calls.
    unsafe {
        assert_eq!(pop_rt_array_length(object, &raw mut output), 0);
        assert_eq!(pop_rt_array_get_checked(object, 1, &raw mut output), 0);
    }
}

#[test]
fn bootstrap_abi_pins_keep_handles_alive_until_explicit_unpin() {
    let _guard = abi_test_lock();
    let reference = pop_rt_allocate_object(0);
    let pin = pop_rt_pin(reference);
    assert_ne!(reference, 0);
    assert_ne!(pin, 0);

    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(20, &[]), 1);
    let root = pop_rt_retain_root(reference);
    assert_ne!(root, 0, "pin must retain the managed object");
    assert_eq!(pop_rt_release_root(root), 1);

    assert_eq!(pop_rt_unpin(pin), 1);
    assert_eq!(pop_rt_unpin(pin), 0, "pin handles are single-use");
    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(21, &[]), 1);
    assert_eq!(pop_rt_retain_root(reference), 0);
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
#[allow(unsafe_code)]
fn bootstrap_string_read_preserves_empty_ascii_and_non_ascii_utf8() {
    let _guard = abi_test_lock();
    for expected in ["", "teste", "Pop 🫧"] {
        let reference = allocate_utf8_string_literal(expected.as_bytes());
        let encoded_length = unsafe { pop_rt_string_read(reference, std::ptr::null_mut(), 0) };
        assert_eq!(
            encoded_length,
            u64::try_from(expected.len()).expect("portable test length") + 1
        );

        let mut bytes = vec![0_u8; expected.len()];
        let copied = unsafe {
            pop_rt_string_read(
                reference,
                bytes.as_mut_ptr(),
                u64::try_from(bytes.len()).expect("portable test length"),
            )
        };
        assert_eq!(copied, encoded_length);
        assert_eq!(bytes, expected.as_bytes());
    }
}

#[test]
#[allow(unsafe_code)]
fn bootstrap_string_read_rejects_invalid_handles_and_small_buffers() {
    let _guard = abi_test_lock();
    let string = allocate_utf8_string_literal(b"teste");
    let non_string = pop_rt_allocate_object(1);
    let mut bytes = [0xAA_u8; 4];

    assert_eq!(unsafe { pop_rt_string_read(0, std::ptr::null_mut(), 0) }, 0);
    assert_eq!(
        unsafe { pop_rt_string_read(non_string, std::ptr::null_mut(), 0) },
        0
    );
    let copied = unsafe { pop_rt_string_read(string, bytes.as_mut_ptr(), 4) };
    assert_eq!(copied, 0);
    assert_eq!(bytes, [0xAA; 4]);
}

#[test]
fn bootstrap_string_composition_is_typed_utf8_and_locale_independent() {
    let _guard = abi_test_lock();
    let left = allocate_utf8_string_literal("Pop ".as_bytes());
    let right = allocate_utf8_string_literal("🫧".as_bytes());
    let joined = pop_rt_string_concat(left, right);
    assert_eq!(read_string(joined), "Pop 🫧");

    let signed = pop_rt_string_format(StringFormatTag::Int8 as u32, u64::from((-12_i8) as u8));
    let negative_zero = pop_rt_string_format(StringFormatTag::Float64 as u32, (-0.0_f64).to_bits());
    let boolean = pop_rt_string_format(StringFormatTag::Boolean as u32, 1);
    assert_eq!(read_string(signed), "-12");
    assert_eq!(read_string(negative_zero), "-0");
    assert_eq!(read_string(boolean), "true");
    assert_eq!(pop_rt_string_format(u32::MAX, 0), 0);
}

#[allow(unsafe_code)]
fn read_string(reference: u64) -> String {
    // SAFETY: A null target performs the documented length query.
    let encoded = unsafe { pop_rt_string_read(reference, std::ptr::null_mut(), 0) };
    assert_ne!(encoded, 0);
    let length = usize::try_from(encoded - 1).expect("portable string length");
    let mut bytes = vec![0; length];
    // SAFETY: `bytes` exposes exactly `length` writable bytes.
    assert_eq!(
        unsafe { pop_rt_string_read(reference, bytes.as_mut_ptr(), length as u64) },
        encoded
    );
    String::from_utf8(bytes).expect("runtime strings are UTF-8")
}

#[test]
fn process_arguments_preserve_order_empty_and_non_ascii_utf8() {
    let _guard = abi_test_lock();
    let arguments =
        allocate_process_arguments(&[b"first".as_slice(), b"".as_slice(), "Pop 🫧".as_bytes()]);
    assert_ne!(arguments, 0);

    for (index, expected) in ["first", "", "Pop 🫧"].iter().enumerate() {
        let actual = pop_rt_array_get(arguments, u64::try_from(index + 1).expect("index"));
        let expected = allocate_utf8_string_literal(expected.as_bytes());
        assert_ne!(actual, 0);
        assert_eq!(pop_rt_string_equal(actual, expected), 1);
    }
    assert_eq!(pop_rt_array_get(arguments, 4), 0);
}

#[test]
fn process_arguments_reject_invalid_utf8_without_partial_success() {
    let _guard = abi_test_lock();
    assert_eq!(
        allocate_process_arguments(&[b"valid".as_slice(), &[0xff_u8]]),
        0
    );
}

#[test]
fn platform_argument_adapter_omits_the_executable_path() {
    let _guard = abi_test_lock();
    let executable = CString::new("/tmp/pop-program").expect("executable");
    let first = CString::new("first").expect("argument");
    let unicode = CString::new("Pop 🫧").expect("argument");
    let arguments = allocate_platform_arguments(&[&executable, &first, &unicode]);
    assert_ne!(arguments, 0);

    let expected_first = allocate_utf8_string_literal(b"first");
    let expected_unicode = allocate_utf8_string_literal("Pop 🫧".as_bytes());
    assert_eq!(
        pop_rt_string_equal(pop_rt_array_get(arguments, 1), expected_first),
        1
    );
    assert_eq!(
        pop_rt_string_equal(pop_rt_array_get(arguments, 2), expected_unicode),
        1
    );
    assert_eq!(pop_rt_array_get(arguments, 3), 0);
}

#[test]
fn bootstrap_abi_allocates_and_tracks_opaque_handles() {
    let _guard = abi_test_lock();
    let reference = pop_rt_allocate_array(2, 0);
    assert_ne!(reference, 0);
    let root = pop_rt_retain_root(reference);
    assert_ne!(root, 0);
    assert_eq!(pop_rt_release_root(root), 1);
    assert_ne!(pop_rt_allocate_table(3, 1, 0), 0);
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
fn typed_table_abi_traces_managed_keys_without_marking_scalar_values() {
    let _guard = abi_test_lock();
    let table = pop_rt_allocate_table(1, 1, 0);
    let key = pop_rt_allocate_object(0);
    assert_ne!(table, 0);
    assert_ne!(key, 0);
    assert_eq!(pop_rt_field_set(table, 1, key), 1);
    assert_eq!(pop_rt_field_set(table, 2, 42), 1);

    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(20, &[table]), 1);
    assert_eq!(abi_safe_point(21, &[key]), 1);
    assert_eq!(pop_rt_field_get(table, 2), 42);
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
