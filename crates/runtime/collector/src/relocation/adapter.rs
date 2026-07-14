//! PLRI adapter for the relocation-conformance collector stage.

use pop_runtime_interface::{
    ArrayAllocationRequest, ArrayElementMap, GarbageCollectorContract, ManagedReference,
    ObjectAllocationRequest, ObjectMap, ObjectSlot, PinHandle, RootHandle, RootPublication,
    RuntimeAdapter, RuntimeFailure, SafePointOutcome, TableAllocationRequest, WriteBarrier,
};

use crate::heap::AllocationKind;

use super::heap::RelocationRuntime;

impl RuntimeAdapter for RelocationRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::relocation_conformance_stage2()
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
        let references = match request.element_map() {
            ArrayElementMap::Scalar => Vec::new(),
            ArrayElementMap::ManagedReference => {
                (0..request.length()).map(ObjectSlot::new).collect()
            }
        };
        let object_map = ObjectMap::new(request.length(), references)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        self.allocate(
            request.type_id(),
            request.allocation_class(),
            AllocationKind::Array(request.element_map()),
            object_map,
        )
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
        let handle = RootHandle::new(self.next_root);
        self.next_root = self
            .next_root
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.roots.insert(handle, reference);
        Ok(handle)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.roots
            .remove(&root)
            .map(|_| ())
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn pin(&mut self, reference: ManagedReference) -> Result<PinHandle, RuntimeFailure> {
        self.validate_reference(reference)?;
        let object = self
            .objects
            .get_mut(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        object.generation = super::heap::CollectorGeneration::Mature;
        let handle = PinHandle::new(self.next_pin);
        self.next_pin = self
            .next_pin
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.pins.insert(handle, reference);
        Ok(handle)
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
        let Some(scheduler) = self.collection_requested else {
            return Ok(SafePointOutcome::no_collection());
        };
        let statistics = self.collect_minor(roots, scheduler)?;
        self.collection_requested = None;
        Ok(SafePointOutcome::collected(statistics))
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.validate_reference(barrier.owner())?;
        if let Some(reference) = barrier.previous() {
            self.validate_reference(reference)?;
        }
        if let Some(reference) = barrier.value() {
            self.validate_reference(reference)?;
        }
        self.apply_reference_barrier(
            barrier.owner(),
            barrier.slot(),
            barrier.previous(),
            barrier.value(),
        )
    }
}
