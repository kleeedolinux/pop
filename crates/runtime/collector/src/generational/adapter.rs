//! PLRI adapter for incremental generational conformance.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, FfiBytesBorrow, FfiBytesBorrowId,
    GarbageCollectorContract, ManagedReference, ObjectAllocationRequest, ObjectMap, PinHandle,
    RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, SafePointOutcome,
    TableAllocationRequest, WriteBarrier,
};

use crate::heap::{AllocationKind, BootstrapRuntime};
use crate::relocation::CollectorGeneration;

use super::heap::GenerationalRuntime;
use super::workers::CardRefinementTask;
use super::{PendingMatureObject, ReservedMatureLease, ReservedMatureObject};

impl GenerationalRuntime {
    fn selected_object_request(
        &self,
        request: &ObjectAllocationRequest,
    ) -> ObjectAllocationRequest {
        if request.allocation_class() == AllocationClass::NurseryEligible
            && request
                .allocation_site()
                .is_some_and(|site| self.pretenuring.should_pretenure(site))
        {
            request.with_allocation_class(AllocationClass::Mature)
        } else {
            request.clone()
        }
    }

    /// Reserves a bounded stable-token TLAB slice for one pointer-free site.
    ///
    /// # Errors
    ///
    /// Rejects a non-mature or reference-bearing layout, zero capacity, token
    /// exhaustion, or memory admission failure without publishing an object.
    pub fn reserve_pointer_free_mature_objects(
        &mut self,
        request: &ObjectAllocationRequest,
        count: usize,
    ) -> Result<ReservedMatureLease, RuntimeFailure> {
        if request.allocation_class() != AllocationClass::Mature
            || request.object_map().slot_count() == 0
            || request.object_map().has_reference_slots()
            || count == 0
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let object_map = request.shared_object_map();
        self.prepare_mature_batch(request.type_id(), &object_map, count)?;
        let identities = self.nursery.reserve_mature_identities(count)?;
        let reservations = self.allocation.reserve_pointer_free_mature_objects(
            identities,
            request.type_id(),
            object_map.clone(),
            self.scheduler,
        )?;
        let first_reference = reservations
            .first()
            .map(ReservedMatureObject::reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let last_reference = reservations
            .last()
            .and_then(ReservedMatureObject::last_reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let Some(validation) =
            self.allocation
                .direct_reference_lease(first_reference, last_reference, self.scheduler)
        else {
            self.allocation
                .cancel_reserved_tail(first_reference, last_reference);
            return Err(RuntimeFailure::runtime_invariant());
        };
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(ReservedMatureLease::new(
            request.type_id(),
            self.scheduler,
            object_map,
            reservations,
            validation,
        ))
    }

    fn prepare_mature_batch(
        &mut self,
        type_id: pop_runtime_interface::RuntimeTypeId,
        object_map: &Arc<ObjectMap>,
        count: usize,
    ) -> Result<(), RuntimeFailure> {
        let requested_slots = usize::try_from(object_map.slot_count())
            .map_err(|_| BootstrapRuntime::out_of_memory(count, usize::MAX))?;
        let mut requirement = self.allocation.mature_batch_requirement_shared(
            type_id,
            object_map,
            self.scheduler,
            count,
        )?;
        let mut committed_after = self
            .allocation
            .committed_bytes()
            .saturating_add(requirement.additional_committed_bytes);
        if self.memory.pressure_for(committed_after) {
            self.memory.record_pressure(committed_after);
            self.request_major_collection();
            if self.major_cycle_active() {
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
            requirement = self.allocation.mature_batch_requirement_shared(
                type_id,
                object_map,
                self.scheduler,
                count,
            )?;
            committed_after = self
                .allocation
                .committed_bytes()
                .saturating_add(requirement.additional_committed_bytes);
        }
        if !self.memory.admits(committed_after) {
            self.memory.record_out_of_memory();
            return Err(BootstrapRuntime::out_of_memory(count, requested_slots));
        }
        let _ = requirement.object_bytes;
        Ok(())
    }

    /// Publishes complete objects consumed from the caller's bound TLAB.
    ///
    /// # Errors
    ///
    /// Rejects scheduler mismatch, stale reservations, or duplicate tokens.
    pub fn publish_reserved_mature_objects(
        &mut self,
        pending: Vec<PendingMatureObject>,
    ) -> Result<(), RuntimeFailure> {
        for pending in &pending {
            if pending.scheduler != self.scheduler
                || self.nursery.contains(pending.reference)
                || self.allocation.placement(pending.reference).is_none()
            {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        for pending in pending {
            let reference = self.nursery.publish_reserved_mature(pending)?;
            self.allocation.publish_direct_reference(reference);
            self.mark_new_allocation(reference);
        }
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(())
    }

    /// Publishes every completed lease entry and cancels all unused capacity.
    ///
    /// # Errors
    ///
    /// Rejects malformed page ranges, scheduler mismatch, stale reservations,
    /// duplicate tokens, or partial publication state.
    pub fn publish_reserved_mature_lease(
        &mut self,
        lease: ReservedMatureLease,
    ) -> Result<(), RuntimeFailure> {
        let mut publication = lease.into_publication()?;
        let Some((first_reserved, last_reserved)) = publication.bounds() else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if publication.scheduler != self.scheduler
            || !self
                .allocation
                .contains_placement_range(first_reserved, last_reserved)
            || !publication.initialized_page_ranges_are_valid()
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let initialized_bounds = publication.initialized_bounds();
        let unused = publication.cancel_unused_tail();
        if let Some((first, last)) = initialized_bounds {
            self.allocation.publish_direct_range(first, last);
            self.mark_new_allocation_range(first, last);
        }
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        if let Some((first, last)) = unused {
            if self.nursery.contains(first) || self.nursery.contains(last) {
                return Err(RuntimeFailure::runtime_invariant());
            }
            self.allocation.cancel_reserved_tail(first, last);
        }
        if publication.initialized_count() != 0 {
            self.deferred_mature.push(publication);
        }
        Ok(())
    }

    pub fn cancel_reserved_objects(&mut self, references: Vec<ManagedReference>) {
        if references.is_empty() {
            return;
        }
        for reference in references {
            if !self.nursery.contains(reference) {
                self.allocation.remove_without_page_reclamation(reference);
            }
        }
        self.allocation.reclaim_empty_pages_after_sweep();
    }

    pub(crate) fn materialize_deferred_mature(&mut self) -> Result<(), RuntimeFailure> {
        if self.deferred_mature.is_empty() {
            return Ok(());
        }
        let deferred = std::mem::take(&mut self.deferred_mature);
        for publication in deferred {
            let pending = publication.into_pending()?;
            let references = pending
                .iter()
                .map(PendingMatureObject::reference)
                .collect::<Vec<_>>();
            self.allocation.materialize_direct_placements(&references)?;
            self.nursery
                .publish_reserved_mature_batch(pending.into_iter())?;
        }
        Ok(())
    }

    /// Allocates one object with its complete typed payload before publication.
    ///
    /// # Errors
    ///
    /// Rejects invalid initializers, managed tokens, or memory admission.
    pub fn allocate_object_initialized(
        &mut self,
        request: &ObjectAllocationRequest,
        values: &[u64],
    ) -> Result<ManagedReference, RuntimeFailure> {
        let request = self.selected_object_request(request);
        let object_map = request.shared_object_map();
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            &object_map,
            true,
        )?;
        let reference = self.nursery.fresh_reference()?;
        let placement = self.allocation.place_shared(
            reference,
            request.type_id(),
            request.allocation_class(),
            object_map,
            self.scheduler,
            Some(AllocationKind::Object),
        )?;
        let (payload, start, page_object_map) = match placement.into_page_storage() {
            Ok(storage) => storage,
            Err(error) => {
                self.allocation.remove(reference);
                return Err(error);
            }
        };
        if let Err(error) = self.nursery.publish_object_initialized_on_page(
            reference,
            &request,
            values,
            page_object_map,
            payload,
            start,
        ) {
            self.allocation.remove(reference);
            return Err(error);
        }
        self.complete_allocation(reference)
    }

    /// Allocates one atomically published object whose selected reference
    /// slots point at the new object itself.
    ///
    /// # Errors
    ///
    /// Rejects non-reference, duplicate, out-of-bounds, or nonzero bootstrap
    /// self slots before exposing the new token.
    pub fn allocate_object_initialized_self_referential(
        &mut self,
        request: &ObjectAllocationRequest,
        values: &[u64],
        self_slots: &[pop_runtime_interface::ObjectSlot],
    ) -> Result<ManagedReference, RuntimeFailure> {
        if self_slots.windows(2).any(|pair| pair[0] >= pair[1])
            || self_slots.iter().any(|slot| {
                !request.object_map().is_reference_slot(*slot)
                    || values.get(slot.raw() as usize).copied() != Some(0)
            })
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let request = self.selected_object_request(request);
        let object_map = request.shared_object_map();
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            &object_map,
            true,
        )?;
        let reference = self.nursery.fresh_reference()?;
        let placement = self.allocation.place_shared(
            reference,
            request.type_id(),
            request.allocation_class(),
            object_map,
            self.scheduler,
            Some(AllocationKind::Object),
        )?;
        let (payload, start, page_object_map) = match placement.into_page_storage() {
            Ok(storage) => storage,
            Err(error) => {
                self.allocation.remove(reference);
                return Err(error);
            }
        };
        if let Err(error) = self.nursery.publish_object_initialized_on_page(
            reference,
            &request,
            values,
            page_object_map,
            payload,
            start,
        ) {
            self.allocation.remove(reference);
            return Err(error);
        }
        for slot in self_slots {
            if let Err(error) = self
                .nursery
                .store_reference(reference, *slot, Some(reference))
            {
                self.allocation.remove(reference);
                self.nursery.discard_unpublished(reference)?;
                return Err(error);
            }
        }
        self.complete_allocation(reference)
    }

    /// Allocates one array with its final scalar payload in a single pass.
    ///
    /// Managed-reference arrays retain the ordinary checked fill path.
    ///
    /// # Errors
    ///
    /// Forwards typed allocation, memory-admission, or initialization failures.
    pub fn allocate_array_filled(
        &mut self,
        request: &ArrayAllocationRequest,
        value: u64,
    ) -> Result<ManagedReference, RuntimeFailure> {
        if value != 0
            && self
                .deferred_mature
                .iter()
                .any(|publication| publication.contains(ManagedReference::new(value)))
        {
            self.materialize_deferred_mature()?;
        }
        let object_map = Self::array_object_map(request);
        let object_map = Arc::new(object_map);
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            &object_map,
            true,
        )?;
        let reference = self.nursery.fresh_reference()?;
        let placement = self.allocation.place_shared(
            reference,
            request.type_id(),
            request.allocation_class(),
            object_map,
            self.scheduler,
            Some(AllocationKind::Array(request.element_map())),
        )?;
        let (payload, start, page_object_map) = match placement.into_page_storage() {
            Ok(storage) => storage,
            Err(error) => {
                self.allocation.remove(reference);
                return Err(error);
            }
        };
        if let Err(error) = self.nursery.publish_array_filled_on_page(
            reference,
            request,
            page_object_map,
            payload,
            start,
            value,
        ) {
            self.allocation.remove(reference);
            return Err(error);
        }
        self.complete_allocation(reference)
    }

    fn prepare_allocation(
        &mut self,
        type_id: pop_runtime_interface::RuntimeTypeId,
        class: pop_runtime_interface::AllocationClass,
        object_map: &Arc<ObjectMap>,
        allow_assist: bool,
    ) -> Result<(), RuntimeFailure> {
        let requested_slots = usize::try_from(object_map.slot_count())
            .map_err(|_| BootstrapRuntime::out_of_memory(1, usize::MAX))?;
        let mut requirement = self.allocation.placement_requirement_shared(
            type_id,
            class,
            object_map,
            self.scheduler,
        )?;
        let mut committed_after = self
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
            requirement = self.allocation.placement_requirement_shared(
                type_id,
                class,
                object_map,
                self.scheduler,
            )?;
            committed_after = self
                .allocation
                .committed_bytes()
                .saturating_add(requirement.additional_committed_bytes);
        }
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
        object_map: Arc<ObjectMap>,
        kind: AllocationKind,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let placement = match self.allocation.place_shared(
            reference,
            type_id,
            class,
            object_map,
            self.scheduler,
            Some(kind),
        ) {
            Ok(placement) => placement,
            Err(error) => {
                self.nursery.discard_unpublished(reference)?;
                return Err(error);
            }
        };
        let object = self
            .nursery
            .objects
            .get_mut(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if let Err(error) = super::allocation::AllocationInfrastructure::bind_object_at(
            placement,
            &mut object.allocation,
        ) {
            self.allocation.remove(reference);
            self.nursery.discard_unpublished(reference)?;
            return Err(error);
        }
        self.complete_allocation(reference)
    }

    fn complete_allocation(
        &mut self,
        reference: ManagedReference,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let object = self
            .nursery
            .objects
            .get_mut(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        object.ownership = crate::ownership::ObjectOwnership::SchedulerLocal(self.scheduler);
        self.allocation.publish_direct_reference(reference);
        self.mark_new_allocation(reference);
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(reference)
    }

    fn array_object_map(request: &ArrayAllocationRequest) -> ObjectMap {
        match request.element_map() {
            ArrayElementMap::Scalar => ObjectMap::scalar(request.length()),
            ArrayElementMap::ManagedReference => {
                ObjectMap::homogeneous_references(request.length())
            }
        }
    }

    fn refine_cards_for_minor(&mut self) -> Result<bool, RuntimeFailure> {
        if self.workers.is_none() || self.nursery.dirty_cards.is_empty() {
            self.pending_card_refinement = None;
            return Ok(true);
        }
        if self.production_concurrent
            && let Some(pending) = self.pending_card_refinement.take()
        {
            let refined = self
                .workers
                .as_mut()
                .ok_or_else(RuntimeFailure::runtime_invariant)?
                .complete_card_refinement(pending.count)?;
            if pending.mutation_version == self.reference_mutation_version {
                self.nursery.install_refined_cards(refined)?;
                return Ok(true);
            }
        }
        let young = Arc::new(
            self.nursery
                .objects
                .iter()
                .filter_map(|(reference, object)| {
                    matches!(object.generation, CollectorGeneration::Nursery { .. })
                        .then_some(reference)
                        .filter(|_| {
                            object.ownership
                                == crate::ObjectOwnership::SchedulerLocal(self.scheduler)
                        })
                })
                .collect::<BTreeSet<_>>(),
        );
        let tasks = self
            .nursery
            .dirty_cards
            .iter()
            .filter(|owner| {
                self.nursery.objects.get(owner).is_some_and(|object| {
                    object.ownership == crate::ObjectOwnership::SchedulerLocal(self.scheduler)
                })
            })
            .map(|owner| {
                let object = self
                    .nursery
                    .objects
                    .get(owner)
                    .filter(|object| object.generation == CollectorGeneration::Mature)
                    .ok_or_else(RuntimeFailure::runtime_invariant)?;
                Ok(CardRefinementTask {
                    owner: *owner,
                    allocation: object.allocation.clone(),
                })
            })
            .collect::<Result<Vec<_>, RuntimeFailure>>()?;
        let workers = self
            .workers
            .as_mut()
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if self.production_concurrent {
            let count = workers.submit_card_refinement(tasks, &young)?;
            self.pending_card_refinement = Some(super::heap::PendingCardRefinement {
                count,
                mutation_version: self.reference_mutation_version,
            });
            Ok(false)
        } else {
            let refined = workers.refine_cards(tasks, &young)?;
            self.nursery.install_refined_cards(refined)?;
            Ok(true)
        }
    }
}

impl RuntimeAdapter for GenerationalRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        if self.production_concurrent {
            GarbageCollectorContract::pop_v1()
        } else {
            GarbageCollectorContract::relocation_conformance_stage2()
        }
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let request = self.selected_object_request(request);
        let object_map = request.shared_object_map();
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            &object_map,
            true,
        )?;
        let reference = self.nursery.allocate_object(&request)?;
        self.finish_allocation(
            reference,
            request.type_id(),
            request.allocation_class(),
            object_map,
            AllocationKind::Object,
        )
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let object_map = Self::array_object_map(request);
        let object_map = Arc::new(object_map);
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
            object_map,
            AllocationKind::Array(request.element_map()),
        )
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let object_map = Arc::new(request.object_map().clone());
        self.prepare_allocation(
            request.type_id(),
            request.allocation_class(),
            &object_map,
            true,
        )?;
        let reference = self.nursery.allocate_table(request)?;
        self.finish_allocation(
            reference,
            request.type_id(),
            request.allocation_class(),
            object_map,
            AllocationKind::Table,
        )
    }

    fn allocate_immutable_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocate_immutable_bytes_with_class(bytes, AllocationClass::NurseryEligible)
    }

    fn immutable_bytes_length(&self, bytes: ManagedReference) -> Result<u64, RuntimeFailure> {
        self.immutable_bytes(bytes)
            .and_then(|payload| u64::try_from(payload.len()).ok())
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn immutable_bytes_read(
        &self,
        bytes: ManagedReference,
        offset: u64,
        target: &mut [u8],
    ) -> Result<(), RuntimeFailure> {
        let payload = self
            .immutable_bytes(bytes)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let start = usize::try_from(offset).map_err(|_| RuntimeFailure::runtime_invariant())?;
        let end = start
            .checked_add(target.len())
            .filter(|end| *end <= payload.len())
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        target.copy_from_slice(&payload[start..end]);
        Ok(())
    }

    fn ffi_bytes_borrow(
        &mut self,
        bytes: ManagedReference,
    ) -> Result<FfiBytesBorrow, RuntimeFailure> {
        self.borrow_immutable_bytes(bytes)
    }

    fn ffi_bytes_end_borrow(
        &mut self,
        bytes: ManagedReference,
        borrow: FfiBytesBorrowId,
    ) -> Result<(), RuntimeFailure> {
        self.end_immutable_bytes_borrow(bytes, borrow)
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        if self
            .deferred_mature
            .iter()
            .any(|publication| publication.contains(reference))
        {
            self.materialize_deferred_mature()?;
        }
        if matches!(
            self.ownership(reference),
            Some(crate::ownership::ObjectOwnership::Isolated(_))
        ) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let root = self.nursery.retain_root(reference)?;
        self.shade_new_root(reference);
        Ok(root)
    }

    fn resolve_root(&mut self, root: RootHandle) -> Result<ManagedReference, RuntimeFailure> {
        self.nursery.resolve_root(root)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        if self.isolation.owns_handle(root) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.nursery.release_root(root)
    }

    fn pin(&mut self, reference: ManagedReference) -> Result<PinHandle, RuntimeFailure> {
        if self
            .deferred_mature
            .iter()
            .any(|publication| publication.contains(reference))
        {
            self.materialize_deferred_mature()?;
        }
        if matches!(
            self.ownership(reference),
            Some(crate::ownership::ObjectOwnership::Isolated(_))
        ) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.nursery.validate_pin_transition(reference)?;
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
        if !already_pinned {
            self.allocation
                .move_to_pinned(reference, type_id, &object_map)?;
            let object = self
                .nursery
                .objects
                .get_mut(&reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            self.allocation
                .bind_object(reference, &mut object.allocation)?;
        }
        let pin = self.nursery.pin(reference)?;
        if let Err(error) = self.pinning.register(pin, reference) {
            self.nursery.unpin(pin)?;
            return Err(error);
        }
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        self.mark_new_allocation(reference);
        Ok(pin)
    }

    fn unpin(&mut self, pin: PinHandle) -> Result<(), RuntimeFailure> {
        let record = self
            .pinning
            .record(pin)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.nursery.unpin(pin)?;
        self.pinning.complete_unpin(pin, record)
    }

    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        self.pinning.advance_safe_point();
        let servicing_minor = self.minor_requested.contains(&self.scheduler)
            && !self.major_cycle_active()
            && self.active_major_collection_epoch().is_none();
        let deferred_root = roots.managed_references().any(|reference| {
            self.deferred_mature
                .iter()
                .any(|publication| publication.contains(reference))
        });
        if servicing_minor || self.major_requested || self.major_cycle_active() || deferred_root {
            self.materialize_deferred_mature()?;
        }
        let identities_before: BTreeMap<_, _> = if servicing_minor {
            self.nursery
                .objects
                .iter()
                .map(|(reference, object)| (object.identity, reference))
                .collect()
        } else {
            BTreeMap::new()
        };
        let sampled_sites_before: BTreeMap<_, _> = if servicing_minor {
            self.nursery
                .objects
                .values()
                .filter(|object| {
                    matches!(object.generation, CollectorGeneration::Nursery { .. })
                        && object.ownership
                            == crate::ObjectOwnership::SchedulerLocal(self.scheduler)
                })
                .filter_map(|object| object.allocation.site.map(|site| (object.identity, site)))
                .collect()
        } else {
            BTreeMap::new()
        };
        if servicing_minor {
            if !self.refine_cards_for_minor()? {
                return Ok(SafePointOutcome::no_collection());
            }
            self.allocation.invalidate_all_direct_accesses();
            self.nursery.request_minor_collection_for(self.scheduler);
            self.minor_requested.remove(&self.scheduler);
        }
        let minor = self.nursery.safe_point(roots)?;
        if servicing_minor && minor.collection().is_some() {
            self.allocation.reconcile_after_minor(
                &identities_before,
                &self.nursery.objects,
                self.scheduler,
            )?;
            self.allocation
                .bind_all_payloads(&mut self.nursery.objects)?;
            let mut sampled = BTreeMap::<_, usize>::new();
            for site in sampled_sites_before.values().copied() {
                *sampled.entry(site).or_default() += 1;
            }
            let mut survived = BTreeMap::<_, usize>::new();
            for object in self.nursery.objects.values() {
                if let Some(site) = sampled_sites_before.get(&object.identity) {
                    *survived.entry(*site).or_default() += 1;
                }
            }
            for (site, count) in sampled {
                self.pretenuring
                    .observe(site, count, survived.get(&site).copied().unwrap_or(0));
            }
            self.update_memory_target();
        }
        if self.major_requested && !self.major_cycle_active() {
            if self.has_registered_mutators() {
                if self.active_major_collection_epoch().is_none() {
                    self.begin_major_collection_handshake()
                        .map_err(Self::handshake_failure)?;
                }
            } else {
                self.begin_major(roots)?;
            }
        }
        if let Some(statistics) = self.advance_major()? {
            self.update_memory_target();
            return Ok(SafePointOutcome::collected(statistics));
        }
        Ok(minor)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        if self.deferred_mature.iter().any(|publication| {
            publication.contains(barrier.owner())
                || barrier
                    .value()
                    .is_some_and(|reference| publication.contains(reference))
        }) {
            self.materialize_deferred_mature()?;
        }
        self.ensure_mutable(barrier.owner())?;
        self.validate_ownership_edge(barrier.owner(), barrier.value())?;
        self.nursery.write_barrier(barrier)?;
        self.record_satb(barrier.previous());
        self.record_post_scan_edge(barrier.owner(), barrier.value());
        Ok(())
    }
}
