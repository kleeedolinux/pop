//! Typed logical access to bootstrap object and array storage.

use pop_runtime_interface::{
    ArrayElementMap, ManagedReference, ObjectSlot, RuntimeAdapter, RuntimeFailure, RuntimeTypeId,
    WriteBarrier,
};

use crate::heap::{AllocationKind, BootstrapRuntime, SlotValue};

impl BootstrapRuntime {
    /// Borrows the logical values of a scalar array with the expected runtime
    /// type through one collector lookup.
    #[must_use]
    pub fn scalar_array_values(
        &self,
        reference: ManagedReference,
        expected_type: RuntimeTypeId,
    ) -> Option<impl ExactSizeIterator<Item = u64> + '_> {
        let allocation = self.objects.get(&reference)?;
        if allocation.type_id != expected_type
            || !matches!(
                allocation.kind,
                AllocationKind::Array(ArrayElementMap::Scalar)
            )
            || !allocation
                .slots
                .iter()
                .all(|slot| matches!(slot, SlotValue::Scalar(_)))
        {
            return None;
        }
        Some(allocation.slots.iter().map(|slot| match slot {
            SlotValue::Scalar(value) => *value,
            SlotValue::Reference(_) => unreachable!("scalar array was validated"),
        }))
    }

    #[must_use]
    pub fn array_length(&self, reference: ManagedReference) -> Option<u64> {
        self.objects.get(&reference).and_then(|allocation| {
            matches!(allocation.kind, AllocationKind::Array(_))
                .then(|| u64::try_from(allocation.slots.len()).unwrap_or(u64::MAX))
        })
    }

    /// Replaces every element in a typed array before or after publication.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for a non-array owner, an invalid managed
    /// value, or an inconsistent precise element map.
    pub fn fill_array_value(
        &mut self,
        owner: ManagedReference,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let (length, element_map) = self
            .objects
            .get(&owner)
            .and_then(|allocation| match allocation.kind {
                AllocationKind::Array(element_map) => Some((allocation.slots.len(), element_map)),
                AllocationKind::Object | AllocationKind::Table => None,
            })
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if element_map == ArrayElementMap::ManagedReference {
            let reference = (value != 0).then(|| ManagedReference::new(value));
            if reference.is_some_and(|reference| !self.contains(reference)) {
                return Err(RuntimeFailure::runtime_invariant());
            }
            for index in 0..length {
                self.store_reference(
                    owner,
                    ObjectSlot::new(u32::try_from(index).unwrap_or(u32::MAX)),
                    reference,
                )?;
            }
        } else {
            let allocation = self
                .objects
                .get_mut(&owner)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            for slot in &mut allocation.slots {
                *slot = SlotValue::Scalar(value);
            }
        }
        Ok(())
    }

    /// Stores a managed reference into a slot identified as a reference by the
    /// allocation's precise object map.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for invalid objects, slots, or
    /// references.
    pub fn store_reference(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        if value.is_some_and(|reference| !self.contains(reference)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let previous = self
            .objects
            .get(&owner)
            .and_then(|allocation| allocation.slots.get(slot.raw() as usize))
            .copied();
        let Some(SlotValue::Reference(previous)) = previous else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        self.write_barrier(WriteBarrier::new(
            pop_runtime_interface::BarrierKind::CombinedSatbGenerational,
            owner,
            slot,
            previous,
            value,
        ))?;
        let allocation = self
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        allocation.slots[slot.raw() as usize] = SlotValue::Reference(value);
        Ok(())
    }

    /// Stores a scalar into a slot that is absent from the precise pointer map.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for invalid objects, slots, or a
    /// reference-designated slot.
    pub fn store_scalar(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let allocation = self
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let Some(current) = allocation.slots.get_mut(slot.raw() as usize) else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if !matches!(current, SlotValue::Scalar(_)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        *current = SlotValue::Scalar(value);
        Ok(())
    }

    /// Loads a scalar from a precise non-reference slot.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for invalid objects, slots, or
    /// reference-designated slots.
    pub fn load_scalar(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        let allocation = self
            .objects
            .get(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        match allocation.slots.get(slot.raw() as usize) {
            Some(SlotValue::Scalar(value)) => Ok(*value),
            Some(SlotValue::Reference(_)) | None => Err(RuntimeFailure::runtime_invariant()),
        }
    }

    /// Stores either a scalar or a managed handle according to the slot's
    /// precise allocation map.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for an invalid allocation, slot, or
    /// managed handle.
    pub fn store_array_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        if !self
            .objects
            .get(&owner)
            .is_some_and(|allocation| matches!(allocation.kind, AllocationKind::Array(_)))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.store_slot_value(owner, slot, value)
    }

    /// Stores a physical bootstrap value according to the slot's precise map.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid owner, slot, or managed
    /// reference.
    pub fn store_slot_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let is_reference = self
            .objects
            .get(&owner)
            .and_then(|allocation| allocation.slots.get(slot.raw() as usize))
            .is_some_and(|slot| matches!(slot, SlotValue::Reference(_)));
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

    /// Loads either a scalar or a managed handle according to the slot's
    /// precise allocation map. Empty references are returned as zero.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic for an invalid allocation or slot.
    pub fn load_array_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        if !self
            .objects
            .get(&owner)
            .is_some_and(|allocation| matches!(allocation.kind, AllocationKind::Array(_)))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.load_slot_value(owner, slot)
    }

    /// Loads a physical bootstrap value according to the slot's precise map.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid owner or slot.
    pub fn load_slot_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        let allocation = self
            .objects
            .get(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        match allocation.slots.get(slot.raw() as usize) {
            Some(SlotValue::Scalar(value)) => Ok(*value),
            Some(SlotValue::Reference(value)) => Ok(value.map_or(0, ManagedReference::raw)),
            None => Err(RuntimeFailure::runtime_invariant()),
        }
    }

    #[must_use]
    pub fn strings_equal(&self, left: ManagedReference, right: ManagedReference) -> bool {
        let Some(left) = self.objects.get(&left) else {
            return false;
        };
        let Some(right) = self.objects.get(&right) else {
            return false;
        };
        left.type_id == RuntimeTypeId::new(1)
            && right.type_id == RuntimeTypeId::new(1)
            && left.slots == right.slots
    }
}
