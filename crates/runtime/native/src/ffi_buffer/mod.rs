//! Native ABI storage for owned FFI buffers.

use std::alloc::{Layout, alloc_zeroed, dealloc};
use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard, OnceLock};

use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot, RootHandle,
    RuntimeAdapter, RuntimeTypeId,
};

use crate::state::lock_abi_runtime;

const BUFFER_SLOT_COUNT: u32 = 7;
const MAGIC_SLOT: u32 = 0;
const RESOURCE_SLOT: u32 = 1;
const LAYOUT_SLOT: u32 = 2;
const ELEMENT_SIZE_SLOT: u32 = 3;
const LENGTH_SLOT: u32 = 4;
const ALIGNMENT_SLOT: u32 = 5;
const CLOSED_SLOT: u32 = 6;
const BUFFER_MAGIC: u64 = 0x504f_5046_4649_4255;
const BUFFER_RUNTIME_TYPE: RuntimeTypeId = RuntimeTypeId::new(207);

const OPEN_ALLOCATION_FAILURE: u8 = 0;
const SUCCESS: u8 = 1;
const OPEN_INVARIANT_FAILURE: u8 = 2;

struct AlignedStorage {
    address: usize,
    layout: Layout,
}

impl AlignedStorage {
    #[allow(unsafe_code)]
    fn zeroed(size: usize, alignment: usize) -> Result<Option<Self>, u8> {
        if size == 0 {
            return Ok(None);
        }
        Layout::from_size_align(0, alignment).map_err(|_| OPEN_INVARIANT_FAILURE)?;
        let layout =
            Layout::from_size_align(size, alignment).map_err(|_| OPEN_ALLOCATION_FAILURE)?;
        // SAFETY: `layout` is nonzero and valid. Ownership remains with this
        // value until its matching `Drop` call.
        let pointer = unsafe { alloc_zeroed(layout) };
        if pointer.is_null() {
            return Err(OPEN_ALLOCATION_FAILURE);
        }
        Ok(Some(Self {
            address: pointer.addr(),
            layout,
        }))
    }

    fn pointer(&self) -> *mut u8 {
        self.address as *mut u8
    }
}

#[allow(unsafe_code)]
impl Drop for AlignedStorage {
    fn drop(&mut self) {
        // SAFETY: `address` was returned by `alloc_zeroed` for this exact
        // layout and this owner deallocates it exactly once.
        unsafe { dealloc(self.pointer(), self.layout) };
    }
}

struct BufferState {
    root: RootHandle,
    layout: u64,
    element_size: u64,
    length: u64,
    alignment: u64,
    storage: Option<AlignedStorage>,
    borrow: Option<u64>,
}

#[derive(Default)]
struct BufferRegistry {
    next_resource: u64,
    next_borrow: u64,
    live: BTreeMap<u64, BufferState>,
}

#[derive(Clone, Copy)]
struct BufferMetadata {
    resource: u64,
    layout: u64,
    element_size: u64,
    length: u64,
    alignment: u64,
    closed: bool,
}

fn registry() -> &'static Mutex<BufferRegistry> {
    static REGISTRY: OnceLock<Mutex<BufferRegistry>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(BufferRegistry::default()))
}

fn lock_registry() -> Result<MutexGuard<'static, BufferRegistry>, ()> {
    registry().lock().map_err(|_| ())
}

fn next_nonzero(value: &mut u64) -> Result<u64, ()> {
    *value = value.checked_add(1).ok_or(())?;
    if *value == 0 {
        return Err(());
    }
    Ok(*value)
}

fn load_metadata(buffer: u64) -> Result<BufferMetadata, ()> {
    if buffer == 0 {
        return Err(());
    }
    let runtime = lock_abi_runtime().map_err(|_| ())?;
    let owner = ManagedReference::new(buffer);
    if runtime.allocation_type(owner) != Some(BUFFER_RUNTIME_TYPE) {
        return Err(());
    }
    let load = |slot| {
        runtime
            .load_slot_value(owner, ObjectSlot::new(slot))
            .map_err(|_| ())
    };
    if load(MAGIC_SLOT)? != BUFFER_MAGIC {
        return Err(());
    }
    let closed = match load(CLOSED_SLOT)? {
        0 => false,
        1 => true,
        _ => return Err(()),
    };
    Ok(BufferMetadata {
        resource: load(RESOURCE_SLOT)?,
        layout: load(LAYOUT_SLOT)?,
        element_size: load(ELEMENT_SIZE_SLOT)?,
        length: load(LENGTH_SLOT)?,
        alignment: load(ALIGNMENT_SLOT)?,
        closed,
    })
}

fn live_state(
    registry: &mut BufferRegistry,
    metadata: BufferMetadata,
    layout: u64,
    buffer: u64,
) -> Result<&mut BufferState, ()> {
    if metadata.closed || layout == 0 || metadata.layout != layout {
        return Err(());
    }
    let state = registry.live.get_mut(&metadata.resource).ok_or(())?;
    if state.layout != metadata.layout
        || state.element_size != metadata.element_size
        || state.length != metadata.length
        || state.alignment != metadata.alignment
    {
        return Err(());
    }
    let mut runtime = lock_abi_runtime().map_err(|_| ())?;
    if runtime.resolve_root(state.root).map_err(|_| ())?.raw() != buffer {
        return Err(());
    }
    Ok(state)
}

fn element_offset(state: &BufferState, index: u64, supplied_size: u64) -> Result<usize, ()> {
    if supplied_size != state.element_size || index == 0 || index > state.length {
        return Err(());
    }
    let offset = (index - 1).checked_mul(state.element_size).ok_or(())?;
    usize::try_from(offset).map_err(|_| ())
}

/// Opens zero-initialized, target-aligned foreign storage.
///
/// Returns zero for allocation failure, one for success, and two for invariant
/// failure. `out_buffer` remains unchanged on failure.
///
/// # Safety
///
/// `out_buffer` must address one writable `u64` for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_ffi_buffer_open(
    length: u64,
    element_size: u64,
    alignment: u64,
    layout: u64,
    out_buffer: *mut u64,
) -> u8 {
    if out_buffer.is_null()
        || element_size == 0
        || alignment == 0
        || !alignment.is_power_of_two()
        || layout == 0
    {
        return OPEN_INVARIANT_FAILURE;
    }
    let Some(byte_length) = length.checked_mul(element_size) else {
        return OPEN_INVARIANT_FAILURE;
    };
    let Ok(byte_length) = usize::try_from(byte_length) else {
        return OPEN_ALLOCATION_FAILURE;
    };
    let Ok(alignment_usize) = usize::try_from(alignment) else {
        return OPEN_INVARIANT_FAILURE;
    };
    let storage = match AlignedStorage::zeroed(byte_length, alignment_usize) {
        Ok(storage) => storage,
        Err(status) => return status,
    };
    let Ok(mut buffers) = lock_registry() else {
        return OPEN_INVARIANT_FAILURE;
    };
    let Ok(resource) = next_nonzero(&mut buffers.next_resource) else {
        return OPEN_INVARIANT_FAILURE;
    };
    let request = ObjectAllocationRequest::new(
        BUFFER_RUNTIME_TYPE,
        AllocationClass::Mature,
        ObjectMap::scalar(BUFFER_SLOT_COUNT),
    );
    let values = [
        BUFFER_MAGIC,
        resource,
        layout,
        element_size,
        length,
        alignment,
        0,
    ];
    let Ok(mut runtime) = lock_abi_runtime() else {
        return OPEN_INVARIANT_FAILURE;
    };
    let Ok(buffer) = runtime.allocate_object_initialized(&request, &values) else {
        return OPEN_ALLOCATION_FAILURE;
    };
    let Ok(root) = runtime.retain_root(buffer) else {
        return OPEN_INVARIANT_FAILURE;
    };
    buffers.live.insert(
        resource,
        BufferState {
            root,
            layout,
            element_size,
            length,
            alignment,
            storage,
            borrow: None,
        },
    );
    // SAFETY: The caller contract requires one writable `u64`.
    unsafe { out_buffer.write(buffer.raw()) };
    SUCCESS
}

mod operations;

pub use operations::*;
