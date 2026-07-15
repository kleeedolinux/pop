use super::{
    AlignedStorage, CLOSED_SLOT, ManagedReference, ObjectSlot, RuntimeAdapter, SUCCESS,
    element_offset, live_state, load_metadata, lock_abi_runtime, lock_registry, next_nonzero,
};

/// Returns a live buffer's element count through `out_length`.
///
/// # Safety
///
/// `out_length` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_buffer_length(
    buffer: u64,
    layout: u64,
    out_length: *mut u64,
) -> u8 {
    if out_length.is_null() {
        return 0;
    }
    let Ok(mut buffers) = lock_registry() else {
        return 0;
    };
    let Ok(metadata) = load_metadata(buffer) else {
        return 0;
    };
    let Ok(state) = live_state(&mut buffers, metadata, layout, buffer) else {
        return 0;
    };
    let length = state.length;
    // SAFETY: The caller contract requires one writable `u64`.
    unsafe { out_length.write(length) };
    SUCCESS
}

/// Copies one exact element into caller-owned output storage.
///
/// # Safety
///
/// `out_element` must address `element_size` writable bytes for this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_buffer_read(
    buffer: u64,
    layout: u64,
    index: u64,
    out_element: *mut u8,
    element_size: u64,
) -> u8 {
    if out_element.is_null() {
        return 0;
    }
    let Ok(mut buffers) = lock_registry() else {
        return 0;
    };
    let Ok(metadata) = load_metadata(buffer) else {
        return 0;
    };
    let Ok(state) = live_state(&mut buffers, metadata, layout, buffer) else {
        return 0;
    };
    let Ok(offset) = element_offset(state, index, element_size) else {
        return 0;
    };
    let Some(storage) = state.storage.as_ref() else {
        return 0;
    };
    let Ok(element_size) = usize::try_from(element_size) else {
        return 0;
    };
    // SAFETY: Bounds were checked against the exact allocation geometry, and
    // the caller contract provides an equally sized writable output.
    unsafe {
        storage
            .pointer()
            .add(offset)
            .copy_to_nonoverlapping(out_element, element_size);
    };
    SUCCESS
}

/// Copies one exact caller-owned element into a live buffer.
///
/// # Safety
///
/// `element` must address `element_size` readable bytes for this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_buffer_write(
    buffer: u64,
    layout: u64,
    index: u64,
    element: *const u8,
    element_size: u64,
) -> u8 {
    if element.is_null() {
        return 0;
    }
    let Ok(mut buffers) = lock_registry() else {
        return 0;
    };
    let Ok(metadata) = load_metadata(buffer) else {
        return 0;
    };
    let Ok(state) = live_state(&mut buffers, metadata, layout, buffer) else {
        return 0;
    };
    let Ok(offset) = element_offset(state, index, element_size) else {
        return 0;
    };
    let Some(storage) = state.storage.as_ref() else {
        return 0;
    };
    let Ok(element_size) = usize::try_from(element_size) else {
        return 0;
    };
    // SAFETY: Bounds were checked against the exact allocation geometry, and
    // the caller contract provides an equally sized readable input.
    unsafe { element.copy_to_nonoverlapping(storage.pointer().add(offset), element_size) };
    SUCCESS
}

/// Starts one lexical borrow and returns its pointer, length, and generation.
///
/// Outputs remain unchanged on failure.
///
/// # Safety
///
/// Every output must address one writable value of its declared type.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_buffer_borrow(
    buffer: u64,
    layout: u64,
    out_pointer: *mut *mut u8,
    out_length: *mut u64,
    out_borrow: *mut u64,
) -> u8 {
    if out_pointer.is_null() || out_length.is_null() || out_borrow.is_null() {
        return 0;
    }
    let Ok(mut buffers) = lock_registry() else {
        return 0;
    };
    let Ok(metadata) = load_metadata(buffer) else {
        return 0;
    };
    let Ok(borrow) = next_nonzero(&mut buffers.next_borrow) else {
        return 0;
    };
    let Ok(state) = live_state(&mut buffers, metadata, layout, buffer) else {
        return 0;
    };
    if state.borrow.is_some() {
        return 0;
    }
    let pointer = state
        .storage
        .as_ref()
        .map_or(std::ptr::null_mut(), AlignedStorage::pointer);
    let length = state.length;
    state.borrow = Some(borrow);
    // SAFETY: The caller contract requires three writable output values.
    unsafe {
        out_pointer.write(pointer);
        out_length.write(length);
        out_borrow.write(borrow);
    }
    SUCCESS
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_ffi_buffer_end_borrow(buffer: u64, borrow: u64) -> u8 {
    if borrow == 0 {
        return 0;
    }
    let Ok(mut buffers) = lock_registry() else {
        return 0;
    };
    let Ok(metadata) = load_metadata(buffer) else {
        return 0;
    };
    let layout = metadata.layout;
    let Ok(state) = live_state(&mut buffers, metadata, layout, buffer) else {
        return 0;
    };
    if state.borrow != Some(borrow) {
        return 0;
    }
    state.borrow = None;
    SUCCESS
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_ffi_buffer_close(buffer: u64) -> u8 {
    let Ok(mut buffers) = lock_registry() else {
        return 0;
    };
    let Ok(metadata) = load_metadata(buffer) else {
        return 0;
    };
    if metadata.closed {
        return SUCCESS;
    }
    let Ok(state) = live_state(&mut buffers, metadata, metadata.layout, buffer) else {
        return 0;
    };
    if state.borrow.is_some() {
        return 0;
    }
    let root = state.root;
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    if runtime
        .store_slot_value(
            ManagedReference::new(buffer),
            ObjectSlot::new(CLOSED_SLOT),
            1,
        )
        .is_err()
    {
        return 0;
    }
    if runtime.release_root(root).is_err() {
        return 0;
    }
    buffers.live.remove(&metadata.resource);
    SUCCESS
}
