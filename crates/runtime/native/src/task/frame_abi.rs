/// Allocates a compiler-owned scalar frame with one exact initial root map.
///
/// # Safety
///
/// When `root_count` is nonzero, `roots` must address that many readable
/// `u32` entries for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_task_frame_create(
    slot_count: u64,
    safe_point: u32,
    roots: *const u32,
    root_count: u64,
) -> u64 {
    let Ok(slot_count) = usize::try_from(slot_count) else {
        return 0;
    };
    let Ok(root_count) = usize::try_from(root_count) else {
        return 0;
    };
    if root_count != 0 && roots.is_null() {
        return 0;
    }
    // SAFETY: The caller promises an immutable array with `root_count`
    // elements for the duration of this call; the null/zero case is closed.
    let roots = if root_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(roots, root_count) }
    };
    let roots = roots.iter().copied().map(RootSlot::new).collect();
    let Ok(frame) = NativeTaskFrame::new(vec![0; slot_count], SafePointId::new(safe_point), roots)
    else {
        return 0;
    };
    Box::into_raw(Box::new(frame)) as usize as u64
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_task_frame_release(frame: u64) -> u8 {
    let Some(frame) = native_task_frame_pointer(frame) else {
        return 0;
    };
    // SAFETY: A nonzero unconsumed frame handle is a Box created above. The
    // ABI makes release/task creation consume that handle exactly once.
    unsafe { drop(Box::from_raw(frame)) };
    1
}
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
/// Loads one scalar slot from a live compiler frame.
///
/// # Safety
///
/// `frame` must be a live frame handle and `output` must be writable for one
/// `u64` for the duration of this call.
pub unsafe extern "C" fn pop_rt_task_frame_load(frame: u64, slot: u32, output: *mut u64) -> u8 {
    if frame == 0 || output.is_null() {
        return 0;
    }
    let Some(frame) = native_task_frame_pointer(frame) else {
        return 0;
    };
    // SAFETY: Frame handles are valid during cold initialization or their
    // generated callback; output is writable for this call.
    let Some(frame) = (unsafe { frame.as_ref() }) else {
        return 0;
    };
    let Ok(value) = frame.slot(slot) else {
        return 0;
    };
    unsafe { output.write(value) };
    1
}
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
/// Stores one scalar slot in a live compiler frame.
///
/// # Safety
///
/// `frame` must be a live uniquely mutable frame handle for this call.
pub unsafe extern "C" fn pop_rt_task_frame_store(frame: u64, slot: u32, value: u64) -> u8 {
    let Some(frame) = native_task_frame_pointer(frame) else {
        return 0;
    };
    let Some(frame) = (unsafe { frame.as_mut() }) else {
        return 0;
    };
    frame.set_slot(slot, value).is_ok().into()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
/// Replaces a live compiler frame's exact safe-point root map.
///
/// # Safety
///
/// `frame` must be a live uniquely mutable frame handle. When `root_count` is
/// nonzero, `roots` must address that many readable `u32` entries for this call.
pub unsafe extern "C" fn pop_rt_task_frame_set_live_map(
    frame: u64,
    safe_point: u32,
    roots: *const u32,
    root_count: u64,
) -> u8 {
    let Ok(root_count) = usize::try_from(root_count) else {
        return 0;
    };
    if root_count != 0 && roots.is_null() {
        return 0;
    }
    let Some(frame) = native_task_frame_pointer(frame) else {
        return 0;
    };
    let Some(frame) = (unsafe { frame.as_mut() }) else {
        return 0;
    };
    let roots = if root_count == 0 {
        &[][..]
    } else {
        unsafe { std::slice::from_raw_parts(roots, root_count) }
    };
    frame
        .set_live_frame(
            SafePointId::new(safe_point),
            roots.iter().copied().map(RootSlot::new).collect(),
        )
        .is_ok()
        .into()
}
