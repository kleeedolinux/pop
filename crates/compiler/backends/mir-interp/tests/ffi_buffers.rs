use pop_backend_mir_interp::ReferenceRuntimeAdapter;
use pop_runtime_interface::{
    FfiAbiLayoutId, FfiBufferOpenFailure, FfiBufferOpenRequest, RuntimeAdapter,
};

fn layout(raw: u64) -> FfiAbiLayoutId {
    FfiAbiLayoutId::new(raw).expect("nonzero layout")
}

#[test]
fn reference_buffers_are_zeroed_bounded_borrowed_and_deterministically_closed() {
    let mut runtime = ReferenceRuntimeAdapter::default();
    let request = FfiBufferOpenRequest::new(2, 4, 4, layout(7)).expect("valid layout");
    let buffer = runtime.ffi_buffer_open(&request).expect("buffer");
    assert_eq!(runtime.ffi_buffer_length(buffer, layout(7)), Ok(2));

    let mut element = [9_u8; 4];
    runtime
        .ffi_buffer_read(buffer, layout(7), 1, &mut element)
        .expect("zeroed read");
    assert_eq!(element, [0; 4]);
    runtime
        .ffi_buffer_write(buffer, layout(7), 2, &[1, 2, 3, 4])
        .expect("write");
    runtime
        .ffi_buffer_read(buffer, layout(7), 2, &mut element)
        .expect("read");
    assert_eq!(element, [1, 2, 3, 4]);

    let before = element;
    assert!(
        runtime
            .ffi_buffer_read(buffer, layout(7), 0, &mut element)
            .is_err()
    );
    assert_eq!(element, before, "failed reads are output-atomic");
    assert!(
        runtime
            .ffi_buffer_read(buffer, layout(7), 3, &mut element)
            .is_err()
    );
    assert!(runtime.ffi_buffer_length(buffer, layout(8)).is_err());

    let borrow = runtime
        .ffi_buffer_borrow(buffer, layout(7))
        .expect("borrow");
    assert!(borrow.address().is_some());
    assert_eq!(borrow.address().expect("address").raw() % 4, 0);
    assert_eq!(borrow.length(), 2);
    assert!(runtime.ffi_buffer_close(buffer).is_err());
    runtime
        .ffi_buffer_end_borrow(buffer, borrow.id())
        .expect("end borrow");
    runtime.ffi_buffer_close(buffer).expect("close");
    runtime.ffi_buffer_close(buffer).expect("idempotent close");
    assert!(runtime.ffi_buffer_length(buffer, layout(7)).is_err());
}

#[test]
fn zero_length_and_allocation_failure_remain_distinct() {
    let mut runtime = ReferenceRuntimeAdapter::default();
    let zero = runtime
        .ffi_buffer_open(&FfiBufferOpenRequest::new(0, 8, 8, layout(1)).expect("zero request"))
        .expect("zero buffer");
    let borrow = runtime
        .ffi_buffer_borrow(zero, layout(1))
        .expect("zero borrow");
    assert_eq!(borrow.address(), None);
    assert_eq!(borrow.length(), 0);
    runtime
        .ffi_buffer_end_borrow(zero, borrow.id())
        .expect("end zero borrow");
    runtime.ffi_buffer_close(zero).expect("close zero");

    let huge = FfiBufferOpenRequest::new(u64::MAX, 1, 1, layout(2)).expect("valid geometry");
    assert_eq!(
        runtime.ffi_buffer_open(&huge),
        Err(FfiBufferOpenFailure::Allocation)
    );
    assert!(FfiBufferOpenRequest::new(u64::MAX, 2, 2, layout(3)).is_err());
    assert!(FfiBufferOpenRequest::new(1, 1, 3, layout(3)).is_err());
}
