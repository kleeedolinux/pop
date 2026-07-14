//! Exact-layout physical access helpers for relocation storage.

use pop_runtime_interface::{ArrayElementMap, ManagedReference, ObjectSlot, RuntimeFailure};

use crate::heap::{AllocationKind, SlotValue};

use super::RelocationRuntime;

impl RelocationRuntime {
    pub(crate) fn store_validated_array_reference(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        previous: Option<ManagedReference>,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        let object = self
            .objects
            .get_mut(&owner)
            .filter(|object| {
                object.allocation.kind == AllocationKind::Array(ArrayElementMap::ManagedReference)
            })
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let current = object
            .allocation
            .slots
            .get_mut(slot.raw() as usize)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if current.as_reference() != previous {
            return Err(RuntimeFailure::runtime_invariant());
        }
        *current = SlotValue::reference(value);
        Ok(())
    }
}
