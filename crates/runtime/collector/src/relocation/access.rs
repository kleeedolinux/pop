//! Exact-layout physical access helpers for relocation storage.

use std::collections::BTreeSet;

use pop_runtime_interface::{ArrayElementMap, ManagedReference, ObjectSlot, RuntimeFailure};

use crate::heap::{AllocationKind, SlotValue};
use crate::ownership::ObjectMutability;

use super::{CollectorGeneration, RelocationRuntime};

impl RelocationRuntime {
    pub(crate) fn fill_array_references_without_major_barrier(
        &mut self,
        owner: ManagedReference,
        value: Option<ManagedReference>,
        capture_previous: bool,
    ) -> Result<BTreeSet<ManagedReference>, RuntimeFailure> {
        let value_generation = match value {
            Some(reference) => Some(
                self.objects
                    .get(&reference)
                    .map(|object| object.generation)
                    .ok_or_else(RuntimeFailure::runtime_invariant)?,
            ),
            None => None,
        };
        let (owner_generation, previous) = {
            let object = self
                .objects
                .get_mut(&owner)
                .filter(|object| {
                    object.mutability == ObjectMutability::Mutable
                        && object.allocation.kind
                            == AllocationKind::Array(ArrayElementMap::ManagedReference)
                        && object.allocation.object_map.reference_slot_count()
                            == object.allocation.slots.len()
                })
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            let previous = if capture_previous {
                object
                    .allocation
                    .slots
                    .iter()
                    .filter_map(SlotValue::as_reference)
                    .collect()
            } else {
                BTreeSet::new()
            };
            object.allocation.slots.fill(SlotValue::reference(value));
            (object.generation, previous)
        };
        if owner_generation == CollectorGeneration::Mature
            && matches!(value_generation, Some(CollectorGeneration::Nursery { .. }))
        {
            self.dirty_cards.insert(owner);
        }
        Ok(previous)
    }

    pub(crate) fn store_array_reference_without_major_barrier(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        let value_generation = match value {
            Some(reference) => Some(
                self.objects
                    .get(&reference)
                    .map(|object| object.generation)
                    .ok_or_else(RuntimeFailure::runtime_invariant)?,
            ),
            None => None,
        };
        let owner_generation = {
            let object = self
                .objects
                .get_mut(&owner)
                .filter(|object| {
                    object.mutability == ObjectMutability::Mutable
                        && object.allocation.kind
                            == AllocationKind::Array(ArrayElementMap::ManagedReference)
                })
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            let index = slot.raw() as usize;
            if !object
                .allocation
                .slots
                .set(index, SlotValue::reference(value))
            {
                return Err(RuntimeFailure::runtime_invariant());
            }
            object.generation
        };
        if owner_generation == CollectorGeneration::Mature
            && matches!(value_generation, Some(CollectorGeneration::Nursery { .. }))
        {
            self.dirty_cards.insert(owner);
        }
        Ok(())
    }

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
        let index = slot.raw() as usize;
        let current = object
            .allocation
            .slots
            .get(index)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if current.as_reference() != previous {
            return Err(RuntimeFailure::runtime_invariant());
        }
        if object
            .allocation
            .slots
            .set(index, SlotValue::reference(value))
        {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }
}
