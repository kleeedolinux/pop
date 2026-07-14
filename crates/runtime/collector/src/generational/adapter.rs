//! PLRI adapter for incremental generational conformance.

use std::collections::BTreeMap;

use pop_runtime_interface::{
    ArrayAllocationRequest, ArrayElementMap, GarbageCollectorContract, ManagedReference,
    ObjectAllocationRequest, ObjectMap, ObjectSlot, PinHandle, RootHandle, RootPublication,
    RuntimeAdapter, RuntimeFailure, SafePointOutcome, TableAllocationRequest, WriteBarrier,
};

use crate::heap::BootstrapRuntime;

use super::heap::GenerationalRuntime;

impl GenerationalRuntime {
    fn prepare_allocation(
        &mut self,
        type_id: pop_runtime_interface::RuntimeTypeId,
        class: pop_runtime_interface::AllocationClass,
        object_map: &ObjectMap,
        allow_assist: bool,
    ) -> Result<(), RuntimeFailure> {
        let requested_slots = usize::try_from(object_map.slot_count())
            .map_err(|_| BootstrapRuntime::out_of_memory(1, usize::MAX))?;
        let requirement = self
            .allocation
            .placement_requirement(type_id, class, object_map)?;
        let committed_after = self
            .allocation
            .committed_bytes()
            .saturating_add(requirement.additional_committed_bytes);
        if self.memory.pressure_for(committed_after) {
            self.memory.record_pressure(committed_after);
            if class == pop_runtime_interface::AllocationClass::NurseryEligible {
                self.request_minor_collection();
            } else {
                self.request_major_collection();
            }
            if allow_assist && self.major_cycle_active() {
                let budget = self.memory.assist_work_budget();
                let (statistics, completed_work) = self.advance_major_with_budget(budget)?;
                self.memory.record_assist(completed_work);
                if statistics.is_some() {
                    self.update_memory_target();
                } else {
                    self.memory
                        .observe_committed(self.allocation.committed_bytes());
                }
            }
        }

        let requirement = self
            .allocation
            .placement_requirement(type_id, class, object_map)?;
        let committed_after = self
            .allocation
            .committed_bytes()
            .saturating_add(requirement.additional_committed_bytes);
        if !self.memory.admits(committed_after) {
            if class == pop_runtime_interface::AllocationClass::NurseryEligible {
                self.request_major_collection();
            }
            self.memory.record_out_of_memory();
            return Err(BootstrapRuntime::out_of_memory(1, requested_slots));
        }
        let _ = requirement.object_bytes;
        Ok(())
    }

    fn finish_allocation(
        &mut self,
        reference: ManagedReference,
        type_id: pop_runtime_interface::RuntimeTypeId,
        class: pop_runtime_interface::AllocationClass,
        object_map: &ObjectMap,
    ) -> Result<ManagedReference, RuntimeFailure> {
        if let Err(error) = self.allocation.place(reference, type_id, class, object_map) {
            self.nursery.discard_unpublished(reference)?;
            return Err(error);
        }
        self.mark_new_allocation(reference);
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(reference)
    }

    fn array_object_map(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ObjectMap, RuntimeFailure> {
        let references = match request.element_map() {
            ArrayElementMap::Scalar => Vec::new(),
            ArrayElementMap::ManagedReference => {
                let length = usize::try_from(request.length()).map_err(|_| {
                    self.memory.record_out_of_memory();
                    BootstrapRuntime::out_of_memory(1, usize::MAX)
                })?;
                let mut references = Vec::new();
                references.try_reserve_exact(length).map_err(|_| {
                    self.memory.record_out_of_memory();
                    BootstrapRuntime::out_of_memory(1, length)
                })?;
                references.extend((0..request.length()).map(ObjectSlot::new));
                references
            }
        };
        ObjectMap::new(request.length(), references)
            .map_err(|_| RuntimeFailure::runtime_invariant())
    }
}

impl RuntimeAdapter for GenerationalRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::relocation_conformance_stage2()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            request.object_map(),
            true,
        )?;
        let reference = self.nursery.allocate_object(request)?;
        self.finish_allocation(
            reference,
            request.type_id(),
            request.allocation_class(),
            request.object_map(),
        )
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let object_map = self.array_object_map(request)?;
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            &object_map,
            true,
        )?;
        let reference = self.nursery.allocate_array(request)?;
        self.finish_allocation(
            reference,
            request.type_id(),
            request.allocation_class(),
            &object_map,
        )
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            request.object_map(),
            true,
        )?;
        let reference = self.nursery.allocate_table(request)?;
        self.finish_allocation(
            reference,
            request.type_id(),
            request.allocation_class(),
            request.object_map(),
        )
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
        let (type_id, object_map, already_pinned) = self
            .nursery
            .objects
            .get(&reference)
            .map(|object| {
                (
                    object.allocation.type_id,
                    object.allocation.object_map.clone(),
                    self.allocation
                        .placement(reference)
                        .is_some_and(|placement| {
                            placement.domain() == super::allocation::HeapDomain::Pinned
                        }),
                )
            })
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if !already_pinned {
            self.prepare_allocation(
                type_id,
                pop_runtime_interface::AllocationClass::Pinned,
                &object_map,
                false,
            )?;
        }
        let pin = self.nursery.pin(reference)?;
        self.allocation
            .move_to_pinned(reference, type_id, &object_map)?;
        self.memory
            .observe_committed(self.allocation.committed_bytes());
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
        let identities_before: BTreeMap<_, _> = if servicing_minor {
            self.nursery
                .objects
                .iter()
                .map(|(reference, object)| (object.identity, *reference))
                .collect()
        } else {
            BTreeMap::new()
        };
        if servicing_minor {
            self.nursery.request_minor_collection();
            self.minor_requested = false;
        }
        let minor = self.nursery.safe_point(roots)?;
        if servicing_minor && minor.collection().is_some() {
            self.allocation
                .reconcile_after_minor(&identities_before, &self.nursery.objects)?;
            self.update_memory_target();
        }
        if self.major_requested && !self.major_cycle_active() {
            self.begin_major(roots)?;
        }
        if let Some(statistics) = self.advance_major()? {
            self.update_memory_target();
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
