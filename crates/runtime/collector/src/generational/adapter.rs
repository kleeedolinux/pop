//! PLRI adapter for incremental generational conformance.

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
        self.mark_new_allocation(reference);
        Ok(reference)
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference = self.nursery.allocate_array(request)?;
        self.mark_new_allocation(reference);
        Ok(reference)
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference = self.nursery.allocate_table(request)?;
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
        if self.minor_requested && !self.major_cycle_active() {
            self.nursery.request_minor_collection();
            self.minor_requested = false;
        }
        let minor = self.nursery.safe_point(roots)?;
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
