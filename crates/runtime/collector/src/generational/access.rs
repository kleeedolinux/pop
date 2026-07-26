//! Typed logical access to generational object, array, and table storage.

use std::sync::Arc;

use pop_runtime_interface::{
    AllocationClass, ArrayElementMap, ManagedReference, ObjectMap, ObjectSlot, RuntimeFailure,
    RuntimeTypeId,
};

use crate::heap::{AllocationKind, SlotValue};
use crate::ownership::{ObjectMutability, ObjectOwnership};
use crate::relocation::CollectorGeneration;

use super::heap::GenerationalRuntime;
use super::{DirectPageAccess, DirectReferenceStoreAccess, DirectReferenceValidation};

impl GenerationalRuntime {
    #[must_use]
    pub fn direct_object_page_access(
        &self,
        reference: ManagedReference,
    ) -> Option<DirectPageAccess> {
        self.allocation
            .direct_page_access(reference, Some(AllocationKind::Object))
    }

    #[must_use]
    pub fn direct_array_page_access(
        &self,
        reference: ManagedReference,
    ) -> Option<DirectPageAccess> {
        let kind = self.nursery.objects.get(&reference).and_then(|object| {
            match object.allocation.kind {
                AllocationKind::Array(element_map) => Some(AllocationKind::Array(element_map)),
                AllocationKind::Object | AllocationKind::Table => None,
            }
        })?;
        self.allocation.direct_page_access(reference, Some(kind))
    }

    #[must_use]
    pub fn direct_array_reference_store_access(
        &self,
        reference: ManagedReference,
    ) -> Option<DirectReferenceStoreAccess> {
        if self.major_cycle_active() {
            return None;
        }
        self.nursery.objects.get(&reference).filter(|object| {
            object.allocation.kind == AllocationKind::Array(ArrayElementMap::ManagedReference)
                && object.generation == CollectorGeneration::Mature
                && object.mutability == ObjectMutability::Mutable
        })?;
        let scheduler = self.direct_local_mature_scheduler(reference)?;
        let access = self.allocation.direct_page_access(
            reference,
            Some(AllocationKind::Array(ArrayElementMap::ManagedReference)),
        )?;
        DirectReferenceStoreAccess::new(access, scheduler)
    }

    #[must_use]
    pub fn direct_reference_validation(
        &self,
        reference: ManagedReference,
    ) -> Option<DirectReferenceValidation> {
        if self.major_cycle_active() {
            return None;
        }
        if self
            .nursery
            .objects
            .get(&reference)
            .is_some_and(|object| object.generation != CollectorGeneration::Mature)
        {
            return None;
        }
        if !self.nursery.contains(reference)
            && !self
                .deferred_mature
                .iter()
                .any(|publication| publication.contains(reference))
        {
            return None;
        }
        let scheduler = self.direct_local_mature_scheduler(reference)?;
        let access = self.allocation.direct_page_access(reference, None)?;
        Some(DirectReferenceValidation::new(access, scheduler))
    }

    fn direct_local_mature_scheduler(
        &self,
        reference: ManagedReference,
    ) -> Option<crate::SchedulerId> {
        let scheduler = if let Some(object) = self.nursery.objects.get(&reference) {
            let ObjectOwnership::SchedulerLocal(scheduler) = object.ownership else {
                return None;
            };
            scheduler
        } else {
            self.deferred_mature
                .iter()
                .find(|publication| publication.contains(reference))
                .map(|publication| publication.scheduler)?
        };
        let placement = self.allocation.placement(reference)?;
        let page = self.allocation.page(placement.page())?;
        (placement.domain() == super::HeapDomain::LocalMature
            && page.scheduler() == Some(scheduler))
        .then_some(scheduler)
    }

    #[must_use]
    pub fn allocation_type(&self, reference: ManagedReference) -> Option<RuntimeTypeId> {
        self.nursery
            .objects
            .get(&reference)
            .map(|object| object.allocation.type_id)
            .or_else(|| {
                self.deferred_mature
                    .iter()
                    .find(|publication| publication.contains(reference))
                    .map(|publication| publication.type_id)
            })
    }

    #[must_use]
    pub fn allocation_class(&self, reference: ManagedReference) -> Option<AllocationClass> {
        self.nursery
            .objects
            .get(&reference)
            .map(|object| object.allocation.class)
            .or_else(|| {
                self.deferred_mature
                    .iter()
                    .find(|publication| publication.contains(reference))
                    .map(|_| AllocationClass::Mature)
            })
    }

    #[must_use]
    pub fn scalar_array_values(
        &self,
        reference: ManagedReference,
        expected_type: RuntimeTypeId,
    ) -> Option<impl ExactSizeIterator<Item = u64> + '_> {
        let allocation = &self.nursery.objects.get(&reference)?.allocation;
        if allocation.type_id != expected_type
            || !matches!(
                allocation.kind,
                AllocationKind::Array(ArrayElementMap::Scalar)
            )
            || allocation.object_map.has_reference_slots()
        {
            return None;
        }
        Some(allocation.slots.iter().map(SlotValue::raw))
    }

    #[must_use]
    pub fn array_length(&self, reference: ManagedReference) -> Option<u64> {
        self.nursery.objects.get(&reference).and_then(|object| {
            matches!(object.allocation.kind, AllocationKind::Array(_))
                .then(|| u64::try_from(object.allocation.slots.len()).unwrap_or(u64::MAX))
        })
    }

    /// Replaces every element of a precisely mapped array.
    ///
    /// # Errors
    ///
    /// Rejects invalid arrays, managed values, or pointer-map inconsistencies.
    pub fn fill_array_value(
        &mut self,
        owner: ManagedReference,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        self.ensure_mutable(owner)?;
        let element_map = self
            .nursery
            .objects
            .get(&owner)
            .and_then(|object| match object.allocation.kind {
                AllocationKind::Array(element_map) => Some(element_map),
                AllocationKind::Object | AllocationKind::Table => None,
            })
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if element_map == ArrayElementMap::Scalar {
            let slots = &mut self
                .nursery
                .objects
                .get_mut(&owner)
                .ok_or_else(RuntimeFailure::runtime_invariant)?
                .allocation
                .slots;
            slots.fill(SlotValue::scalar(value));
            return Ok(());
        }
        let value = (value != 0).then(|| ManagedReference::new(value));
        self.validate_ownership_edge(owner, value)?;
        let previous = self.nursery.fill_array_references_without_major_barrier(
            owner,
            value,
            self.major.phase == super::MajorCyclePhase::Marking,
        )?;
        for reference in previous {
            self.record_satb(Some(reference));
        }
        self.record_post_scan_edge(owner, value);
        self.reference_mutation_version = self.reference_mutation_version.wrapping_add(1);
        Ok(())
    }

    /// Stores a scalar in a non-reference slot.
    ///
    /// # Errors
    ///
    /// Rejects invalid owners, bounds, or reference-designated slots.
    pub fn store_scalar(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        self.ensure_mutable(owner)?;
        let allocation = self
            .nursery
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if allocation.allocation.object_map.is_reference_slot(slot) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        if allocation
            .allocation
            .slots
            .set(slot.raw() as usize, SlotValue::scalar(value))
        {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }

    /// Stores a typed physical value in an array slot.
    ///
    /// # Errors
    ///
    /// Rejects non-arrays or invalid slot values.
    pub fn store_array_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        if !self
            .nursery
            .objects
            .get(&owner)
            .is_some_and(|object| matches!(object.allocation.kind, AllocationKind::Array(_)))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.store_slot_value(owner, slot, value)
    }

    pub(crate) fn store_stable_array_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let element_map = self
            .nursery
            .objects
            .get(&owner)
            .and_then(|object| match object.allocation.kind {
                AllocationKind::Array(element_map) => Some(element_map),
                AllocationKind::Object | AllocationKind::Table => None,
            })
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if element_map == ArrayElementMap::Scalar {
            self.ensure_mutable(owner)?;
            return self.nursery.store_scalar(owner, slot, value);
        }
        let value = (value != 0).then(|| ManagedReference::new(value));
        if self.major.phase != super::MajorCyclePhase::Marking {
            return self
                .nursery
                .store_array_reference_without_major_barrier(owner, slot, value);
        }
        self.ensure_mutable(owner)?;
        let previous = self.nursery.slot_value(owner, slot)?.as_reference();
        if value.is_some_and(|reference| !self.nursery.contains(reference)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.record_satb(previous);
        self.record_post_scan_edge(owner, value);
        self.nursery
            .store_validated_array_reference(owner, slot, previous, value)
    }

    /// Stores a value according to the allocation's precise slot map.
    ///
    /// # Errors
    ///
    /// Rejects invalid allocations, slots, or managed values.
    pub fn store_slot_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let is_reference = self
            .nursery
            .objects
            .get(&owner)
            .is_some_and(|object| object.allocation.object_map.is_reference_slot(slot));
        if is_reference {
            self.store_reference(
                owner,
                slot,
                (value != 0).then(|| ManagedReference::new(value)),
            )
        } else {
            self.store_scalar(owner, slot, value)
        }
    }

    pub(crate) fn store_stable_slot_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        self.ensure_mutable(owner)?;
        let is_reference = self
            .nursery
            .objects
            .get(&owner)
            .is_some_and(|object| object.allocation.object_map.is_reference_slot(slot));
        if !is_reference {
            return self.nursery.store_scalar(owner, slot, value);
        }
        let previous = self.nursery.slot_value(owner, slot)?.as_reference();
        let value = (value != 0).then(|| ManagedReference::new(value));
        if value.is_some_and(|reference| !self.nursery.contains(reference)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.record_satb(previous);
        self.record_post_scan_edge(owner, value);
        self.nursery
            .store_validated_reference(owner, slot, previous, value)
    }

    /// Loads one scalar slot.
    ///
    /// # Errors
    ///
    /// Rejects invalid owners, bounds, or reference-designated slots.
    pub fn load_scalar(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        let object = self
            .nursery
            .objects
            .get(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if object.allocation.object_map.is_reference_slot(slot) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        object
            .allocation
            .slots
            .get(slot.raw() as usize)
            .map(SlotValue::raw)
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    /// Loads a typed physical value from an array.
    ///
    /// # Errors
    ///
    /// Rejects non-arrays or invalid slots.
    pub fn load_array_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        self.nursery
            .objects
            .get(&owner)
            .filter(|object| matches!(object.allocation.kind, AllocationKind::Array(_)))
            .and_then(|object| object.allocation.slots.get(slot.raw() as usize))
            .map(SlotValue::raw)
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    /// Loads a value according to the allocation's precise slot map.
    ///
    /// # Errors
    ///
    /// Rejects invalid owners or slots.
    pub fn load_slot_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        self.nursery
            .objects
            .get(&owner)
            .and_then(|object| object.allocation.slots.get(slot.raw() as usize))
            .map(SlotValue::raw)
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    #[must_use]
    pub fn strings_equal(&self, left: ManagedReference, right: ManagedReference) -> bool {
        let Some(left) = self.nursery.objects.get(&left) else {
            return false;
        };
        let Some(right) = self.nursery.objects.get(&right) else {
            return false;
        };
        left.allocation.type_id == RuntimeTypeId::new(1)
            && right.allocation.type_id == RuntimeTypeId::new(1)
            && left.allocation.slots == right.allocation.slots
    }

    /// Grows one precise table while transactionally replacing its placement.
    ///
    /// # Errors
    ///
    /// Rejects invalid table geometry or memory admission failure.
    pub fn grow_table(
        &mut self,
        owner: ManagedReference,
        old_capacity: u32,
        new_capacity: u32,
        key_map: ArrayElementMap,
        value_map: ArrayElementMap,
    ) -> Result<(), RuntimeFailure> {
        if new_capacity <= old_capacity {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let old_slots = old_capacity
            .checked_mul(2)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let new_slots = new_capacity
            .checked_mul(2)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let references = (0..new_capacity).flat_map(|entry| {
            let base = entry * 2;
            [
                (key_map == ArrayElementMap::ManagedReference).then(|| ObjectSlot::new(base)),
                (value_map == ArrayElementMap::ManagedReference).then(|| ObjectSlot::new(base + 1)),
            ]
            .into_iter()
            .flatten()
        });
        let object_map = ObjectMap::new(new_slots, references.collect())
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        let object = self
            .nursery
            .objects
            .get(&owner)
            .filter(|object| {
                object.allocation.kind == AllocationKind::Table
                    && object.allocation.slots.len() == old_slots as usize
            })
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let type_id = object.allocation.type_id;
        let class = object.allocation.class;
        let mut next_object_allocation = object.allocation.clone();
        let added = usize::try_from(new_slots - old_slots)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        let mut slots = object.allocation.slots.clone();
        slots
            .try_reserve_exact(added)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        for _ in old_slots..new_slots {
            slots.push(SlotValue::scalar(0));
        }
        let mut allocation = self.allocation.clone();
        allocation.remove(owner);
        allocation.place(owner, type_id, class, &object_map, self.scheduler)?;
        if !self.memory.admits(allocation.committed_bytes()) {
            self.memory.record_out_of_memory();
            return Err(crate::BootstrapRuntime::out_of_memory(0, added));
        }
        next_object_allocation.object_map = Arc::new(object_map);
        next_object_allocation.slots = slots;
        allocation.bind_object(owner, &mut next_object_allocation)?;
        let object = self
            .nursery
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        object.allocation = next_object_allocation;
        self.allocation = allocation;
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(())
    }
}
