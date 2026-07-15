//! PLRI adapter implementation for the Stage-1 bootstrap collector.

use pop_runtime_interface::{
    ArrayAllocationRequest, ArrayElementMap, GarbageCollectorContract, ManagedReference,
    ObjectAllocationRequest, ObjectMap, ObjectSlot, PinHandle, RootHandle, RootPublication,
    RuntimeAdapter, RuntimeFailure, SafePointOutcome, TableAllocationRequest, WriteBarrier,
};

use crate::heap::{AllocationKind, BootstrapRuntime, SlotValue};

impl BootstrapRuntime {
    /// Allocates and initializes one fixed array before publication.
    ///
    /// # Errors
    ///
    /// Returns a typed allocation failure or rejects an invalid managed
    /// initial value without exposing a partial array.
    pub fn allocate_array_filled(
        &mut self,
        request: &ArrayAllocationRequest,
        initial_value: u64,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let object_map = array_object_map(request)?;
        if request.element_map() == ArrayElementMap::ManagedReference && initial_value != 0 {
            self.validate_reference(ManagedReference::new(initial_value))?;
        }
        self.allocate_filled(
            request.type_id(),
            request.allocation_class(),
            AllocationKind::Array(request.element_map()),
            object_map,
            initial_value,
        )
    }
}

impl RuntimeAdapter for BootstrapRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::bootstrap_stage1()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocate(
            request.type_id(),
            request.allocation_class(),
            AllocationKind::Object,
            request.object_map().clone(),
        )
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocate_array_filled(request, 0)
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocate(
            request.type_id(),
            request.allocation_class(),
            AllocationKind::Table,
            request.object_map().clone(),
        )
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.validate_reference(reference)?;
        let root = RootHandle::new(self.next_root);
        self.next_root = self
            .next_root
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
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
        self.roots
            .remove(&root)
            .map(|_| ())
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn pin(&mut self, reference: ManagedReference) -> Result<PinHandle, RuntimeFailure> {
        self.validate_reference(reference)?;
        let pin = PinHandle::new(self.next_pin);
        self.next_pin = self
            .next_pin
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.pins.insert(pin, reference);
        Ok(pin)
    }

    fn unpin(&mut self, pin: PinHandle) -> Result<(), RuntimeFailure> {
        self.pins
            .remove(&pin)
            .map(|_| ())
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        for reference in roots.managed_references() {
            self.validate_reference(reference)?;
        }
        if !self.collection_requested {
            return Ok(SafePointOutcome::no_collection());
        }
        self.collection_requested = false;
        self.collect(roots).map(SafePointOutcome::collected)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.validate_reference(barrier.owner())?;
        if let Some(reference) = barrier.previous() {
            self.validate_reference(reference)?;
        }
        if let Some(reference) = barrier.value() {
            self.validate_reference(reference)?;
        }
        let allocation = self
            .objects
            .get(&barrier.owner())
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if !allocation.object_map.is_reference_slot(barrier.slot()) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let current = allocation.slots.get(barrier.slot().raw() as usize);
        if current.copied().map(SlotValue::as_reference) != Some(barrier.previous()) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        Ok(())
    }
}

fn array_object_map(request: &ArrayAllocationRequest) -> Result<ObjectMap, RuntimeFailure> {
    let reference_slots = match request.element_map() {
        ArrayElementMap::Scalar => Vec::new(),
        ArrayElementMap::ManagedReference => {
            let length = usize::try_from(request.length())
                .map_err(|_| BootstrapRuntime::out_of_memory(1, usize::MAX))?;
            let mut slots = Vec::new();
            slots
                .try_reserve_exact(length)
                .map_err(|_| BootstrapRuntime::out_of_memory(1, length))?;
            slots.extend((0..request.length()).map(ObjectSlot::new));
            slots
        }
    };
    ObjectMap::new(request.length(), reference_slots)
        .map_err(|_| RuntimeFailure::runtime_invariant())
}
