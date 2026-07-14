//! PLRI adapter for incremental generational conformance.

use std::collections::BTreeMap;

use pop_runtime_interface::{
    ArrayAllocationRequest, GarbageCollectorContract, ManagedReference, ObjectAllocationRequest,
    PinHandle, RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, SafePointOutcome,
    TableAllocationRequest, WriteBarrier,
};

use super::heap::GenerationalRuntime;

impl RuntimeAdapter for GenerationalRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::relocation_conformance_stage2()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference = self.nursery.allocate_object(request)?;
        self.allocation.place(
            reference,
            request.type_id(),
            request.allocation_class(),
            request.object_map(),
        )?;
        self.mark_new_allocation(reference);
        Ok(reference)
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference = self.nursery.allocate_array(request)?;
        let object_map = self
            .nursery
            .objects
            .get(&reference)
            .map(|object| object.allocation.object_map.clone())
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.allocation.place(
            reference,
            request.type_id(),
            request.allocation_class(),
            &object_map,
        )?;
        self.mark_new_allocation(reference);
        Ok(reference)
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference = self.nursery.allocate_table(request)?;
        self.allocation.place(
            reference,
            request.type_id(),
            request.allocation_class(),
            request.object_map(),
        )?;
        self.mark_new_allocation(reference);
        Ok(reference)
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        let root = self.nursery.retain_root(reference)?;
        self.shade_new_root(reference);
        Ok(root)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.nursery.release_root(root)
    }

    fn pin(&mut self, reference: ManagedReference) -> Result<PinHandle, RuntimeFailure> {
        let pin = self.nursery.pin(reference)?;
        let (type_id, object_map) = self
            .nursery
            .objects
            .get(&reference)
            .map(|object| {
                (
                    object.allocation.type_id,
                    object.allocation.object_map.clone(),
                )
            })
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.allocation
            .move_to_pinned(reference, type_id, &object_map)?;
        self.mark_new_allocation(reference);
        Ok(pin)
    }

    fn unpin(&mut self, pin: PinHandle) -> Result<(), RuntimeFailure> {
        self.nursery.unpin(pin)
    }

    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        let servicing_minor = self.minor_requested && !self.major_cycle_active();
        let identities_before: BTreeMap<_, _> = servicing_minor
            .then(|| {
                self.nursery
                    .objects
                    .iter()
                    .map(|(reference, object)| (object.identity, *reference))
                    .collect()
            })
            .unwrap_or_default();
        if servicing_minor {
            self.nursery.request_minor_collection();
            self.minor_requested = false;
        }
        let minor = self.nursery.safe_point(roots)?;
        if servicing_minor && minor.collection().is_some() {
            self.allocation
                .reconcile_after_minor(&identities_before, &self.nursery.objects)?;
        }
        if self.major_requested && !self.major_cycle_active() {
            self.begin_major(roots)?;
        }
        if let Some(statistics) = self.advance_major()? {
            return Ok(SafePointOutcome::collected(statistics));
        }
        Ok(minor)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.nursery.write_barrier(barrier)?;
        self.record_satb(barrier.previous());
        self.record_post_scan_edge(barrier.owner(), barrier.value());
        Ok(())
    }
}
