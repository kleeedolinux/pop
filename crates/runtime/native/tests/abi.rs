use pop_runtime_native::{
    abi_safe_point, allocate_immutable_bytes, allocate_mapped_object, allocate_platform_arguments,
    allocate_process_arguments, allocate_utf8_string_literal, pop_rt_abi_major, pop_rt_abi_minor,
    pop_rt_allocate_array, pop_rt_allocate_array_filled, pop_rt_allocate_initialized_object,
    pop_rt_allocate_object, pop_rt_allocate_table, pop_rt_array_fill, pop_rt_array_get,
    pop_rt_array_get_checked, pop_rt_array_length, pop_rt_array_set, pop_rt_cancel_source_create,
    pop_rt_cancel_source_release, pop_rt_cancel_source_token, pop_rt_cancel_token_release,
    pop_rt_ffi_buffer_borrow, pop_rt_ffi_buffer_close, pop_rt_ffi_buffer_end_borrow,
    pop_rt_ffi_buffer_length, pop_rt_ffi_buffer_open, pop_rt_ffi_buffer_read,
    pop_rt_ffi_buffer_write, pop_rt_ffi_bytes_borrow, pop_rt_ffi_bytes_end_borrow,
    pop_rt_field_get, pop_rt_field_set, pop_rt_gc_safe_point_v2, pop_rt_gc_stage,
    pop_rt_iteration_acquire, pop_rt_iteration_next, pop_rt_list_add, pop_rt_list_create,
    pop_rt_list_get, pop_rt_list_get_checked, pop_rt_list_length, pop_rt_list_set, pop_rt_pin,
    pop_rt_range_create, pop_rt_release_root, pop_rt_resolve_root, pop_rt_resume,
    pop_rt_retain_root, pop_rt_string_concat, pop_rt_string_equal, pop_rt_string_format,
    pop_rt_string_read, pop_rt_supports_abi, pop_rt_suspend, pop_rt_table_get,
    pop_rt_table_get_checked, pop_rt_table_set, pop_rt_task_cancel,
    pop_rt_task_cancellation_requested, pop_rt_unpin, request_abi_collection,
};
use pop_runtime_native_abi::{IterationCollectionKind, IterationStatus, StringFormatTag};
use std::ffi::CString;
use std::sync::{Mutex, MutexGuard, OnceLock};

fn abi_test_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
        .lock()
        .expect("ABI test lock")
}

#[test]
fn native_runtime_exports_the_stable_generational_abi_identity() {
    let _guard = abi_test_lock();
    assert_eq!(pop_rt_abi_major(), 1);
    assert_eq!(pop_rt_abi_minor(), 17);
    assert_eq!(pop_rt_gc_stage(), 2);
    assert_eq!(pop_rt_supports_abi(1, 11), 1);
    assert_eq!(pop_rt_supports_abi(1, 12), 1);
    assert_eq!(pop_rt_supports_abi(1, 13), 1);
    assert_eq!(pop_rt_supports_abi(1, 14), 1);
    assert_eq!(pop_rt_supports_abi(1, 15), 1);
    assert_eq!(pop_rt_supports_abi(1, 16), 1);
    assert_eq!(pop_rt_supports_abi(1, 17), 1);
    assert_eq!(pop_rt_supports_abi(2, 0), 0);
}

#[test]
#[allow(unsafe_code)]
fn native_ffi_bytes_borrow_is_payload_exact_and_failure_atomic() {
    let _guard = abi_test_lock();
    let empty = allocate_immutable_bytes(&[]);
    let mut address = 91_u64;
    let mut length = 92_u64;
    let empty_borrow = unsafe { pop_rt_ffi_bytes_borrow(empty, &raw mut address, &raw mut length) };
    assert_ne!(empty_borrow, 0);
    assert_eq!((address, length), (0, 0));
    assert_eq!(pop_rt_ffi_bytes_end_borrow(empty, empty_borrow), 1);

    let bytes = allocate_immutable_bytes(&[1, 2, 3, 4]);
    let other = allocate_immutable_bytes(&[9]);
    let borrow = unsafe { pop_rt_ffi_bytes_borrow(bytes, &raw mut address, &raw mut length) };
    assert_ne!(borrow, 0);
    assert_ne!(address, 0);
    assert_eq!(length, 4);
    // SAFETY: The ABI borrow remains active until its exact end below.
    assert_eq!(
        unsafe { std::slice::from_raw_parts(address as *const u8, 4) },
        [1, 2, 3, 4]
    );

    let before = (address, length);
    assert_eq!(
        unsafe { pop_rt_ffi_bytes_borrow(bytes, &raw mut address, &raw mut length) },
        0
    );
    assert_eq!((address, length), before);
    assert_eq!(pop_rt_ffi_bytes_end_borrow(other, borrow), 0);
    assert_eq!(pop_rt_ffi_bytes_end_borrow(bytes, borrow + 1), 0);
    assert_eq!(pop_rt_ffi_bytes_end_borrow(bytes, borrow), 1);
    assert_eq!(pop_rt_ffi_bytes_end_borrow(bytes, borrow), 0);

    let forged = pop_rt_allocate_object(0);
    address = 77;
    length = 78;
    assert_eq!(
        unsafe { pop_rt_ffi_bytes_borrow(forged, &raw mut address, &raw mut length) },
        0
    );
    assert_eq!((address, length), (77, 78));
}

#[test]
#[allow(unsafe_code)]
fn native_ffi_buffers_preserve_layout_bounds_borrows_and_close() {
    let _guard = abi_test_lock();
    let mut buffer = 91_u64;
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_open(2, 4, 16, 7, &raw mut buffer) },
        1
    );
    assert_ne!(buffer, 0);

    let mut length = 99_u64;
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_length(buffer, 7, &raw mut length) },
        1
    );
    assert_eq!(length, 2);

    let mut element = [9_u8; 4];
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_read(buffer, 7, 1, element.as_mut_ptr(), 4) },
        1
    );
    assert_eq!(element, [0; 4]);
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_write(buffer, 7, 2, [1_u8, 2, 3, 4].as_ptr(), 4) },
        1
    );
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_read(buffer, 7, 2, element.as_mut_ptr(), 4) },
        1
    );
    assert_eq!(element, [1, 2, 3, 4]);

    let before = element;
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_read(buffer, 7, 0, element.as_mut_ptr(), 4) },
        0
    );
    assert_eq!(element, before);
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_read(buffer, 7, 3, element.as_mut_ptr(), 4) },
        0
    );
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_length(buffer, 8, &raw mut length) },
        0
    );

    let forged = pop_rt_allocate_object(7);
    assert_ne!(forged, 0);
    for field in 1..=7 {
        assert_eq!(
            pop_rt_field_set(forged, field, pop_rt_field_get(buffer, field)),
            1
        );
    }
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_length(forged, 7, &raw mut length) },
        0
    );
    assert_eq!(pop_rt_ffi_buffer_close(forged), 0);

    let mut pointer = std::ptr::dangling_mut::<u8>();
    let mut borrowed_length = 99_u64;
    let mut borrow = 0_u64;
    assert_eq!(
        unsafe {
            pop_rt_ffi_buffer_borrow(
                buffer,
                7,
                &raw mut pointer,
                &raw mut borrowed_length,
                &raw mut borrow,
            )
        },
        1
    );
    assert!(!pointer.is_null());
    assert_eq!(pointer.addr() % 16, 0);
    assert_eq!(borrowed_length, 2);
    assert_ne!(borrow, 0);
    assert_eq!(pop_rt_ffi_buffer_close(buffer), 0);
    assert_eq!(pop_rt_ffi_buffer_end_borrow(buffer, borrow + 1), 0);
    assert_eq!(pop_rt_ffi_buffer_end_borrow(buffer, borrow), 1);
    assert_eq!(pop_rt_ffi_buffer_close(buffer), 1);
    assert_eq!(pop_rt_ffi_buffer_close(buffer), 1);
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_length(buffer, 7, &raw mut length) },
        0
    );
}

#[test]
#[allow(unsafe_code)]
fn native_ffi_buffer_open_separates_zero_length_allocation_and_invariants() {
    let _guard = abi_test_lock();
    let mut zero = 0_u64;
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_open(0, 8, 8, 11, &raw mut zero) },
        1
    );
    assert_ne!(zero, 0);
    let mut pointer = std::ptr::dangling_mut::<u8>();
    let mut length = 91_u64;
    let mut borrow = 0_u64;
    assert_eq!(
        unsafe {
            pop_rt_ffi_buffer_borrow(zero, 11, &raw mut pointer, &raw mut length, &raw mut borrow)
        },
        1
    );
    assert!(pointer.is_null());
    assert_eq!(length, 0);
    assert_ne!(borrow, 0);
    assert_eq!(pop_rt_ffi_buffer_end_borrow(zero, borrow), 1);
    assert_eq!(pop_rt_ffi_buffer_close(zero), 1);

    let mut unchanged = 77_u64;
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_open(u64::MAX, 2, 2, 12, &raw mut unchanged) },
        2
    );
    assert_eq!(unchanged, 77);
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_open(u64::MAX, 1, 1, 12, &raw mut unchanged) },
        0
    );
    assert_eq!(unchanged, 77);
    assert_eq!(
        unsafe { pop_rt_ffi_buffer_open(1, 1, 3, 12, &raw mut unchanged) },
        2
    );
    assert_eq!(unchanged, 77);
}

#[test]
#[allow(unsafe_code)]
fn writable_abi_two_safe_point_is_failure_atomic_but_not_advertised() {
    let _guard = abi_test_lock();
    let root = pop_rt_allocate_object(0);
    assert_ne!(root, 0);
    let mut roots = [root];
    assert!(request_abi_collection());

    // SAFETY: The pointer addresses the exact writable root array declared by
    // the count for the duration of the call.
    assert_eq!(
        unsafe { pop_rt_gc_safe_point_v2(9, roots.as_mut_ptr(), roots.len() as u64) },
        1
    );
    assert_eq!(roots, [root], "stable conformance does not relocate tokens");

    let mut invalid = [u64::MAX];
    let before = invalid;
    // SAFETY: The pointer is writable and the invalid token must be rejected
    // without changing the caller's array.
    assert_eq!(
        unsafe { pop_rt_gc_safe_point_v2(10, invalid.as_mut_ptr(), invalid.len() as u64) },
        0
    );
    assert_eq!(invalid, before);

    // SAFETY: A null pointer is deliberately supplied with a nonzero count to
    // verify the closed ABI rejection.
    assert_eq!(
        unsafe { pop_rt_gc_safe_point_v2(11, std::ptr::null_mut(), 1) },
        0
    );
}

#[test]
fn native_task_abi_preserves_scalar_completion_and_explicit_cancellation_authority() {
    let _guard = abi_test_lock();
    assert_eq!(pop_rt_suspend(42), 42);
    assert_eq!(pop_rt_resume(7), 7);

    let source = pop_rt_cancel_source_create();
    assert_ne!(source, 0);
    let token = pop_rt_cancel_source_token(source);
    assert_ne!(token, 0);
    assert_ne!(source, token);
    assert_eq!(pop_rt_task_cancellation_requested(token), 0);
    assert_eq!(
        pop_rt_task_cancel(token),
        0,
        "a token is not cancellation authority"
    );
    assert_eq!(pop_rt_task_cancel(source), 1);
    assert_eq!(pop_rt_task_cancel(source), 1, "requests are idempotent");
    assert_eq!(pop_rt_task_cancellation_requested(token), 1);
    assert_eq!(pop_rt_cancel_source_release(source), 1);
    assert_eq!(pop_rt_task_cancel(source), 0);
    assert_eq!(pop_rt_task_cancellation_requested(token), 1);
    assert_eq!(pop_rt_cancel_token_release(token), 1);
    assert_eq!(pop_rt_task_cancellation_requested(token), 0);
    assert_eq!(pop_rt_task_cancel(0), 0);
}

#[test]
#[allow(unsafe_code)]
fn initialized_object_allocation_publishes_one_complete_typed_payload() {
    let _guard = abi_test_lock();
    let child = pop_rt_allocate_object(0);
    assert_ne!(child, 0);
    let reference_slots = [1_u32];
    let initial_values = [77_u64, child];

    // SAFETY: Both pointers address the exact immutable arrays declared by the
    // accompanying counts for the duration of the call.
    let object = unsafe {
        pop_rt_allocate_initialized_object(
            2,
            reference_slots.as_ptr(),
            reference_slots.len() as u64,
            initial_values.as_ptr(),
            initial_values.len() as u64,
        )
    };
    assert_ne!(object, 0);
    assert_eq!(pop_rt_field_get(object, 1), 77);
    assert_eq!(pop_rt_field_get(object, 2), child);

    // SAFETY: The pointers remain valid; the deliberately mismatched value
    // count and invalid managed token must fail before publication.
    unsafe {
        assert_eq!(
            pop_rt_allocate_initialized_object(
                2,
                reference_slots.as_ptr(),
                1,
                initial_values.as_ptr(),
                1,
            ),
            0
        );
        let invalid_values = [77_u64, u64::MAX];
        assert_eq!(
            pop_rt_allocate_initialized_object(
                2,
                reference_slots.as_ptr(),
                1,
                invalid_values.as_ptr(),
                2,
            ),
            0
        );
        assert_eq!(
            pop_rt_allocate_initialized_object(1, std::ptr::null(), 0, std::ptr::null(), 1),
            0
        );
    }
}

#[test]
fn stable_generational_safe_points_preserve_live_native_tokens() {
    let _guard = abi_test_lock();
    let reference = pop_rt_allocate_object(0);
    assert_ne!(reference, 0);
    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(19, &[reference]), 1);
    let root = pop_rt_retain_root(reference);
    assert_ne!(root, 0, "ABI 1 native tokens must not relocate");
    assert_eq!(pop_rt_resolve_root(root), reference);
    assert_eq!(pop_rt_release_root(root), 1);
    assert_eq!(pop_rt_resolve_root(root), 0, "closed handles are stale");
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
fn allocation_churn_uses_the_native_stable_generational_path() {
    let _guard = abi_test_lock();
    let mut total = 0_u64;
    let mut value = 0_u64;
    let mut last = 0_u64;
    for index in 1..=20_000_u64 {
        let array = pop_rt_allocate_array_filled(256, 0, index);
        assert_ne!(array, 0);
        last = array;
        // SAFETY: `value` is live and writable for the complete ABI call.
        assert_eq!(
            unsafe { pop_rt_array_get_checked(array, 1, &raw mut value) },
            1
        );
        total = total.checked_add(value).expect("benchmark checksum");
        if index.is_multiple_of(8_192) {
            let safe_point = u32::try_from(index).expect("bounded safe-point identity");
            assert_eq!(abi_safe_point(safe_point, &[]), 1);
        }
    }
    assert_eq!(total, 200_010_000);
    for safe_point in 40_000..41_024 {
        let _ = request_abi_collection();
        assert_eq!(abi_safe_point(safe_point, &[]), 1);
    }
    assert_eq!(pop_rt_array_get(last, 1), 0);
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
fn native_abi_pins_keep_handles_alive_until_explicit_unpin() {
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
fn native_strings_preserve_valid_utf8_and_compare_by_value() {
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
fn native_string_read_preserves_empty_ascii_and_non_ascii_utf8() {
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
fn native_string_read_rejects_invalid_handles_and_small_buffers() {
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
fn native_string_composition_is_typed_utf8_and_locale_independent() {
    let _guard = abi_test_lock();
    let left = allocate_utf8_string_literal("Pop ".as_bytes());
    let right = allocate_utf8_string_literal("🫧".as_bytes());
    let joined = pop_rt_string_concat(left, right);
    assert_eq!(read_string(joined), "Pop 🫧");

    let signed = pop_rt_string_format(
        StringFormatTag::Int8 as u32,
        u64::from((-12_i8).cast_unsigned()),
    );
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
fn native_abi_allocates_and_tracks_opaque_tokens() {
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
#[allow(unsafe_code)]
fn typed_table_abi_replaces_and_grows_without_changing_identity() {
    let _guard = abi_test_lock();
    let table = pop_rt_allocate_table(1, 0, 0);
    assert_ne!(table, 0);
    assert_eq!(pop_rt_table_get(table, 7, 0), 0);
    assert_eq!(pop_rt_table_set(table, 7, 10, 0, 0), 1);
    assert_eq!(pop_rt_table_get(table, 7, 0), 10);
    assert_eq!(pop_rt_table_set(table, 7, 11, 0, 0), 1);
    assert_eq!(pop_rt_table_get(table, 7, 0), 11);
    assert_eq!(pop_rt_table_set(table, 8, 12, 0, 0), 1);
    assert_eq!(pop_rt_table_get(table, 8, 0), 12);

    assert_eq!(pop_rt_table_set(table, 9, 0, 0, 0), 1);
    let mut present_zero = u64::MAX;
    let mut missing = u64::MAX;
    // SAFETY: Both output pointers address live writable `u64` values.
    unsafe {
        assert_eq!(
            pop_rt_table_get_checked(table, 9, 0, &raw mut present_zero),
            1
        );
        assert_eq!(pop_rt_table_get_checked(table, 10, 0, &raw mut missing), 0);
    }
    assert_eq!(present_zero, 0);

    let first = allocate_utf8_string_literal(b"alice");
    let equal = allocate_utf8_string_literal(b"alice");
    let strings = pop_rt_allocate_table(0, 1, 0);
    assert_ne!(first, 0);
    assert_ne!(equal, 0);
    assert_ne!(strings, 0);
    assert_eq!(pop_rt_table_set(strings, first, 42, 1, 0), 1);
    assert_eq!(pop_rt_table_get(strings, equal, 1), 42);
    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(30, &[strings]), 1);
    assert_eq!(abi_safe_point(31, &[first]), 1);
    assert_eq!(abi_safe_point(32, &[equal]), 0);

    let managed_values = pop_rt_allocate_table(0, 0, 1);
    let child = pop_rt_allocate_object(0);
    assert_ne!(managed_values, 0);
    assert_ne!(child, 0);
    assert_eq!(pop_rt_table_set(managed_values, 1, child, 0, 1), 1);
    assert!(request_abi_collection());
    assert_eq!(abi_safe_point(33, &[managed_values]), 1);
    assert_eq!(abi_safe_point(34, &[child]), 1);
}

#[test]
#[allow(unsafe_code)]
fn growable_list_abi_preserves_identity_order_bounds_and_zero_values() {
    let _guard = abi_test_lock();
    let list = pop_rt_list_create(1, 0);
    assert_ne!(list, 0);
    assert_eq!(pop_rt_list_add(list, 0, 0), 1);
    assert_eq!(pop_rt_list_add(list, 42, 0), 1);

    let mut length = u64::MAX;
    let mut first = u64::MAX;
    let mut second = u64::MAX;
    // SAFETY: Every output pointer addresses one live writable `u64`.
    unsafe {
        assert_eq!(pop_rt_list_length(list, &raw mut length), 1);
        assert_eq!(pop_rt_list_get(list, 1, &raw mut first), 1);
        assert_eq!(pop_rt_list_get_checked(list, 2, &raw mut second), 1);
        assert_eq!(pop_rt_list_get(list, 3, &raw mut second), 0);
    }
    assert_eq!(length, 2);
    assert_eq!(first, 0);
    assert_eq!(second, 42);
    assert_eq!(pop_rt_list_set(list, 2, 7, 0), 1);
    assert_eq!(pop_rt_list_set(list, 3, 9, 0), 0);
    // SAFETY: `second` remains live and writable.
    unsafe {
        assert_eq!(pop_rt_list_get_checked(list, 2, &raw mut second), 1);
    }
    assert_eq!(second, 7);
}

#[test]
#[allow(unsafe_code)]
fn integer_range_abi_iterates_without_materializing_items() {
    let _guard = abi_test_lock();
    let range = pop_rt_range_create(1, 5, 2, true, 64);
    assert_ne!(range, 0);
    let iterator = pop_rt_iteration_acquire(range, IterationCollectionKind::Range as u8);
    assert_ne!(iterator, 0);
    let mut value = 0;
    for expected in [1, 3, 5] {
        // SAFETY: `value` is live and writable for the complete call.
        let status = unsafe { pop_rt_iteration_next(iterator, &raw mut value) };
        assert_eq!(status, IterationStatus::Item as u8);
        assert_eq!(value, expected);
    }
    // SAFETY: `value` is live and writable for the complete call.
    let status = unsafe { pop_rt_iteration_next(iterator, &raw mut value) };
    assert_eq!(status, IterationStatus::End as u8);

    for (first, last, step, signed, expected) in [
        (
            u64::from_ne_bytes(i64::MIN.to_ne_bytes()),
            u64::from_ne_bytes((i64::MIN + 1).to_ne_bytes()),
            1,
            true,
            [
                u64::from_ne_bytes(i64::MIN.to_ne_bytes()),
                u64::from_ne_bytes((i64::MIN + 1).to_ne_bytes()),
            ],
        ),
        (u64::MAX - 1, u64::MAX, 1, false, [u64::MAX - 1, u64::MAX]),
    ] {
        let range = pop_rt_range_create(first, last, step, signed, 64);
        let iterator = pop_rt_iteration_acquire(range, IterationCollectionKind::Range as u8);
        for expected in expected {
            // SAFETY: `value` is live and writable for the complete call.
            let status = unsafe { pop_rt_iteration_next(iterator, &raw mut value) };
            assert_eq!(status, IterationStatus::Item as u8);
            assert_eq!(value, expected);
        }
        // SAFETY: `value` is live and writable for the complete call.
        let status = unsafe { pop_rt_iteration_next(iterator, &raw mut value) };
        assert_eq!(status, IterationStatus::End as u8);
    }
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
