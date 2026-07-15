//! Native ABI 1.17 immutable `Bytes` payload borrowing.

use pop_runtime_interface::{FfiBytesBorrowId, ManagedReference, RuntimeAdapter};

use crate::state::lock_abi_runtime;

/// Allocates the trusted packed immutable-byte representation for native
/// library adapters and tests.
#[must_use]
pub fn allocate_immutable_bytes(bytes: &[u8]) -> u64 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    runtime
        .allocate_immutable_bytes(bytes)
        .map_or(0, ManagedReference::raw)
}

/// Borrows only the immutable byte payload and leaves outputs unchanged on
/// failure.
///
/// # Safety
///
/// Both outputs must address writable `u64` values for the duration of this
/// call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_bytes_borrow(
    bytes: u64,
    out_address: *mut u64,
    out_length: *mut u64,
) -> u64 {
    if bytes == 0 || out_address.is_null() || out_length.is_null() {
        return 0;
    }
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Ok(borrow) = runtime.ffi_bytes_borrow(ManagedReference::new(bytes)) else {
        return 0;
    };
    let address = borrow
        .address()
        .map_or(0, pop_runtime_interface::ForeignAddress::raw);
    // SAFETY: The caller contract requires two writable `u64` outputs.
    unsafe {
        out_address.write(address);
        out_length.write(borrow.length());
    }
    borrow.id().raw()
}

/// Ends one exact immutable byte-payload borrow.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_ffi_bytes_end_borrow(bytes: u64, borrow: u64) -> u8 {
    let Some(borrow) = FfiBytesBorrowId::new(borrow) else {
        return 0;
    };
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    u8::from(
        runtime
            .ffi_bytes_end_borrow(ManagedReference::new(bytes), borrow)
            .is_ok(),
    )
}
