//! Deterministic reference implementation of the backend-neutral PLRI adapter.
//!
//! This adapter records typed runtime events for interpreter/native differential
//! tests. It models runtime capabilities without importing native ABI names or
//! collector implementation details.
use pop_runtime_interface::{
    ArrayAllocationRequest, ArrayElementMap, FfiAbiLayoutId, FfiBufferBorrow, FfiBufferBorrowId,
    FfiBufferOpenFailure, FfiBufferOpenRequest, ForeignAddress, GarbageCollectorContract,
    ManagedReference, ObjectAllocationRequest, ObjectMap, ObjectSlot, PinHandle, RootHandle,
    RootPublication, RuntimeAdapter, RuntimeFailure, RuntimeTypeId, SafePointId, SafePointOutcome,
    TableAllocationRequest, Trap, WriteBarrier,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ReferenceRuntimeEvent {
    AllocateObject {
        type_id: RuntimeTypeId,
        object_map: ObjectMap,
    },
    AllocateArray {
        type_id: RuntimeTypeId,
        length: u32,
        element_map: ArrayElementMap,
    },
    AllocateTable {
        type_id: RuntimeTypeId,
        entry_count: u32,
        key_map: ArrayElementMap,
        value_map: ArrayElementMap,
    },
    RetainRoot(ManagedReference),
    ReleaseRoot(RootHandle),
    Pin(ManagedReference),
    Unpin(PinHandle),
    SafePoint {
        safe_point: SafePointId,
        roots: Vec<ManagedReference>,
    },
    WriteBarrier(WriteBarrier),
    Trap(Trap),
    Panic(pop_runtime_interface::PanicPayload),
}

#[derive(Default)]
pub struct ReferenceRuntimeAdapter {
    allocations: BTreeMap<ManagedReference, ObjectMap>,
    roots: BTreeMap<RootHandle, ManagedReference>,
    pins: BTreeMap<PinHandle, ManagedReference>,
    ffi_buffers: BTreeMap<ManagedReference, ReferenceFfiBuffer>,
    next_reference: u64,
    next_root: u64,
    next_pin: u64,
    next_ffi_borrow: u64,
    next_foreign_address: u64,
    events: Vec<ReferenceRuntimeEvent>,
}

struct ReferenceFfiBuffer {
    layout: FfiAbiLayoutId,
    element_size: u64,
    length: u64,
    storage: Vec<u8>,
    address: Option<ForeignAddress>,
    borrow: Option<FfiBufferBorrowId>,
    closed: bool,
}

fn live_ffi_buffer(
    buffers: &BTreeMap<ManagedReference, ReferenceFfiBuffer>,
    buffer: ManagedReference,
    layout: FfiAbiLayoutId,
) -> Result<&ReferenceFfiBuffer, RuntimeFailure> {
    let state = buffers
        .get(&buffer)
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    if state.closed || state.layout != layout {
        return Err(RuntimeFailure::runtime_invariant());
    }
    Ok(state)
}

fn live_ffi_buffer_mut(
    buffers: &mut BTreeMap<ManagedReference, ReferenceFfiBuffer>,
    buffer: ManagedReference,
    layout: FfiAbiLayoutId,
) -> Result<&mut ReferenceFfiBuffer, RuntimeFailure> {
    let state = buffers
        .get_mut(&buffer)
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    if state.closed || state.layout != layout {
        return Err(RuntimeFailure::runtime_invariant());
    }
    Ok(state)
}

fn ffi_element_range(
    state: &ReferenceFfiBuffer,
    index: u64,
    supplied_size: usize,
) -> Result<std::ops::Range<usize>, RuntimeFailure> {
    if index == 0 || index > state.length || u64::try_from(supplied_size) != Ok(state.element_size)
    {
        return Err(RuntimeFailure::runtime_invariant());
    }
    let start = (index - 1)
        .checked_mul(state.element_size)
        .and_then(|value| usize::try_from(value).ok())
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    let end = start
        .checked_add(supplied_size)
        .filter(|end| *end <= state.storage.len())
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    Ok(start..end)
}

impl ReferenceRuntimeAdapter {
    #[must_use]
    pub fn events(&self) -> &[ReferenceRuntimeEvent] {
        &self.events
    }

    fn allocate_map(&mut self, map: ObjectMap) -> ManagedReference {
        self.next_reference = self.next_reference.saturating_add(1).max(1);
        let reference = ManagedReference::new(self.next_reference);
        self.allocations.insert(reference, map);
        reference
    }

    fn valid_reference(&self, reference: ManagedReference) -> Result<(), RuntimeFailure> {
        if self.allocations.contains_key(&reference) {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }
}

impl RuntimeAdapter for ReferenceRuntimeAdapter {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::bootstrap_stage1()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.events.push(ReferenceRuntimeEvent::AllocateObject {
            type_id: request.type_id(),
            object_map: request.object_map().clone(),
        });
        Ok(self.allocate_map(request.object_map().clone()))
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.events.push(ReferenceRuntimeEvent::AllocateArray {
            type_id: request.type_id(),
            length: request.length(),
            element_map: request.element_map(),
        });
        let references = match request.element_map() {
            ArrayElementMap::Scalar => Vec::new(),
            ArrayElementMap::ManagedReference => {
                (0..request.length()).map(ObjectSlot::new).collect()
            }
        };
        let map = ObjectMap::new(request.length(), references)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        Ok(self.allocate_map(map))
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.events.push(ReferenceRuntimeEvent::AllocateTable {
            type_id: request.type_id(),
            entry_count: request.entry_count(),
            key_map: request.key_map(),
            value_map: request.value_map(),
        });
        Ok(self.allocate_map(request.object_map().clone()))
    }

    fn ffi_buffer_open(
        &mut self,
        request: &FfiBufferOpenRequest,
    ) -> Result<ManagedReference, FfiBufferOpenFailure> {
        let byte_length =
            usize::try_from(request.byte_length()).map_err(|_| FfiBufferOpenFailure::Allocation)?;
        let mut storage = Vec::new();
        storage
            .try_reserve_exact(byte_length)
            .map_err(|_| FfiBufferOpenFailure::Allocation)?;
        storage.resize(byte_length, 0);
        let address = if byte_length == 0 {
            None
        } else {
            let alignment = request.alignment();
            let base = self.next_foreign_address.max(0x1000);
            let aligned = base
                .checked_add(alignment - 1)
                .map(|value| value & !(alignment - 1))
                .and_then(ForeignAddress::new)
                .ok_or(FfiBufferOpenFailure::Allocation)?;
            self.next_foreign_address = aligned
                .raw()
                .checked_add(request.byte_length().max(1))
                .ok_or(FfiBufferOpenFailure::Allocation)?;
            Some(aligned)
        };
        let reference =
            self.allocate_map(ObjectMap::new(0, Vec::new()).map_err(|_| {
                FfiBufferOpenFailure::Invariant(RuntimeFailure::runtime_invariant())
            })?);
        self.ffi_buffers.insert(
            reference,
            ReferenceFfiBuffer {
                layout: request.layout(),
                element_size: request.element_size(),
                length: request.length(),
                storage,
                address,
                borrow: None,
                closed: false,
            },
        );
        Ok(reference)
    }

    fn ffi_buffer_length(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
    ) -> Result<u64, RuntimeFailure> {
        let state = live_ffi_buffer(&self.ffi_buffers, buffer, layout)?;
        Ok(state.length)
    }

    fn ffi_buffer_read(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
        index: u64,
        output: &mut [u8],
    ) -> Result<(), RuntimeFailure> {
        let state = live_ffi_buffer(&self.ffi_buffers, buffer, layout)?;
        let range = ffi_element_range(state, index, output.len())?;
        output.copy_from_slice(&state.storage[range]);
        Ok(())
    }

    fn ffi_buffer_write(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
        index: u64,
        element: &[u8],
    ) -> Result<(), RuntimeFailure> {
        let state = live_ffi_buffer_mut(&mut self.ffi_buffers, buffer, layout)?;
        let range = ffi_element_range(state, index, element.len())?;
        state.storage[range].copy_from_slice(element);
        Ok(())
    }

    fn ffi_buffer_borrow(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
    ) -> Result<FfiBufferBorrow, RuntimeFailure> {
        self.next_ffi_borrow = self
            .next_ffi_borrow
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let borrow = FfiBufferBorrowId::new(self.next_ffi_borrow)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let state = live_ffi_buffer_mut(&mut self.ffi_buffers, buffer, layout)?;
        if state.borrow.is_some() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        state.borrow = Some(borrow);
        Ok(FfiBufferBorrow::new(borrow, state.address, state.length))
    }

    fn ffi_buffer_end_borrow(
        &mut self,
        buffer: ManagedReference,
        borrow: FfiBufferBorrowId,
    ) -> Result<(), RuntimeFailure> {
        let state = self
            .ffi_buffers
            .get_mut(&buffer)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if state.closed || state.borrow != Some(borrow) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        state.borrow = None;
        Ok(())
    }

    fn ffi_buffer_close(&mut self, buffer: ManagedReference) -> Result<(), RuntimeFailure> {
        let state = self
            .ffi_buffers
            .get_mut(&buffer)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if state.borrow.is_some() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        if !state.closed {
            state.storage.clear();
            state.address = None;
            state.closed = true;
        }
        Ok(())
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.valid_reference(reference)?;
        self.events
            .push(ReferenceRuntimeEvent::RetainRoot(reference));
        self.next_root = self.next_root.saturating_add(1).max(1);
        let root = RootHandle::new(self.next_root);
        self.roots.insert(root, reference);
        Ok(root)
    }

    fn resolve_root(&mut self, root: RootHandle) -> Result<ManagedReference, RuntimeFailure> {
        self.roots
            .get(&root)
            .copied()
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        let result = self
            .roots
            .remove(&root)
            .map(|_| ())
            .ok_or_else(RuntimeFailure::runtime_invariant);
        if result.is_ok() {
            self.events.push(ReferenceRuntimeEvent::ReleaseRoot(root));
        }
        result
    }

    fn pin(&mut self, reference: ManagedReference) -> Result<PinHandle, RuntimeFailure> {
        self.valid_reference(reference)?;
        self.events.push(ReferenceRuntimeEvent::Pin(reference));
        self.next_pin = self.next_pin.saturating_add(1).max(1);
        let pin = PinHandle::new(self.next_pin);
        self.pins.insert(pin, reference);
        Ok(pin)
    }

    fn unpin(&mut self, pin: PinHandle) -> Result<(), RuntimeFailure> {
        let result = self
            .pins
            .remove(&pin)
            .map(|_| ())
            .ok_or_else(RuntimeFailure::runtime_invariant);
        if result.is_ok() {
            self.events.push(ReferenceRuntimeEvent::Unpin(pin));
        }
        result
    }

    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        for reference in roots.managed_references() {
            self.valid_reference(reference)?;
        }
        self.events.push(ReferenceRuntimeEvent::SafePoint {
            safe_point: roots.stack_map().safe_point(),
            roots: roots.managed_references().collect(),
        });
        Ok(SafePointOutcome::no_collection())
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.valid_reference(barrier.owner())?;
        if let Some(previous) = barrier.previous() {
            self.valid_reference(previous)?;
        }
        if let Some(value) = barrier.value() {
            self.valid_reference(value)?;
        }
        if !self
            .allocations
            .get(&barrier.owner())
            .is_some_and(|map| map.is_reference_slot(barrier.slot()))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.events
            .push(ReferenceRuntimeEvent::WriteBarrier(barrier));
        Ok(())
    }

    fn raise_trap(&mut self, trap: Trap) -> RuntimeFailure {
        self.events.push(ReferenceRuntimeEvent::Trap(trap));
        RuntimeFailure::Trap(trap)
    }

    fn begin_panic(&mut self, payload: pop_runtime_interface::PanicPayload) -> RuntimeFailure {
        self.events
            .push(ReferenceRuntimeEvent::Panic(payload.clone()));
        RuntimeFailure::from_panic(payload)
    }
}
