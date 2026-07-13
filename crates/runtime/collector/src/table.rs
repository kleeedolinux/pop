//! Precise growable storage mechanics for typed associative tables.

use pop_runtime_interface::{
    ArrayElementMap, ManagedReference, ObjectMap, ObjectSlot, RuntimeFailure,
};

use crate::heap::{AllocationKind, BootstrapRuntime, SlotValue};

impl BootstrapRuntime {
    /// Extends precise interleaved table storage while preserving its handle.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for a non-table allocation or inconsistent
    /// capacity, and an out-of-memory panic when the configured slot limit
    /// cannot admit the requested growth.
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
        let added = usize::try_from(new_slots - old_slots)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        if self
            .slot_count
            .checked_add(added)
            .is_none_or(|slots| slots > self.limits.maximum_slots)
        {
            return Err(Self::out_of_memory(0, added));
        }
        let allocation = self
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if allocation.kind != AllocationKind::Table || allocation.slots.len() != old_slots as usize
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
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
        allocation
            .slots
            .try_reserve_exact(added)
            .map_err(|_| Self::out_of_memory(0, added))?;
        for slot in old_slots..new_slots {
            allocation
                .slots
                .push(if object_map.is_reference_slot(ObjectSlot::new(slot)) {
                    SlotValue::Reference(None)
                } else {
                    SlotValue::Scalar(0)
                });
        }
        allocation.object_map = object_map;
        self.slot_count += added;
        Ok(())
    }
}
