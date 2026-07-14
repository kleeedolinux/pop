//! Typed logical access to generational object, array, and table storage.

use pop_runtime_interface::{
    AllocationClass, ArrayElementMap, ManagedReference, ObjectMap, ObjectSlot, RuntimeFailure,
    RuntimeTypeId,
};

use crate::heap::{AllocationKind, SlotValue};

use super::heap::GenerationalRuntime;

impl GenerationalRuntime {
    #[must_use]
    pub fn allocation_type(&self, reference: ManagedReference) -> Option<RuntimeTypeId> {
        self.nursery
            .objects
            .get(&reference)
            .map(|object| object.allocation.type_id)
    }

    #[must_use]
    pub fn allocation_class(&self, reference: ManagedReference) -> Option<AllocationClass> {
        self.nursery
            .objects
            .get(&reference)
            .map(|object| object.allocation.class)
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
            || !allocation.object_map.reference_slots().is_empty()
        {
            return None;
        }
        Some(allocation.slots.iter().map(|slot| slot.raw()))
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
        let (length, element_map) = self
            .nursery
            .objects
            .get(&owner)
            .and_then(|object| match object.allocation.kind {
                AllocationKind::Array(element_map) => {
                    Some((object.allocation.slots.len(), element_map))
                }
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
        for index in 0..length {
            let slot = ObjectSlot::new(
                u32::try_from(index).map_err(|_| RuntimeFailure::runtime_invariant())?,
            );
            self.store_reference(
                owner,
                slot,
                (value != 0).then(|| ManagedReference::new(value)),
            )?;
        }
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
        let allocation = self
            .nursery
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if allocation.allocation.object_map.is_reference_slot(slot) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let current = allocation
            .allocation
            .slots
            .get_mut(slot.raw() as usize)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        *current = SlotValue::scalar(value);
        Ok(())
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
            .copied()
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
        if !self
            .nursery
            .objects
            .get(&owner)
            .is_some_and(|object| matches!(object.allocation.kind, AllocationKind::Array(_)))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.load_slot_value(owner, slot)
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
            .copied()
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
        let object = self
            .nursery
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        object.allocation.object_map = object_map;
        object.allocation.slots = slots;
        self.allocation = allocation;
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(())
    }
}
