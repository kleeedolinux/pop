//! Typed heap state and logical access for relocation conformance.

use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::{ManagedReference, ObjectSlot, PinHandle, RootHandle, RuntimeFailure};

use crate::heap::{Allocation, CollectorMetrics, SlotValue};
use crate::ownership::ObjectOwnership;

use super::table::ObjectTable;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectorGeneration {
    Nursery { age: u8 },
    Mature,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CollectorObjectId(pub(super) u64);

#[derive(Clone, Debug)]
pub(crate) struct RelocationAllocation {
    pub(crate) identity: CollectorObjectId,
    pub(crate) generation: CollectorGeneration,
    pub(crate) allocation: Allocation,
    pub(crate) ownership: ObjectOwnership,
}

pub struct RelocationRuntime {
    pub(crate) objects: ObjectTable<RelocationAllocation>,
    pub(crate) roots: BTreeMap<RootHandle, ManagedReference>,
    pub(crate) pins: BTreeMap<PinHandle, ManagedReference>,
    pub(crate) dirty_cards: BTreeSet<ManagedReference>,
    pub(crate) refined_cards: Option<BTreeMap<ManagedReference, Vec<ManagedReference>>>,
    pub(crate) next_reference: u64,
    pub(super) next_identity: u64,
    pub(super) next_root: u64,
    pub(super) next_pin: u64,
    pub(super) collection_requested: Option<crate::SchedulerId>,
    pub(crate) metrics: CollectorMetrics,
}

impl RelocationRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            objects: ObjectTable::new(),
            roots: BTreeMap::new(),
            pins: BTreeMap::new(),
            dirty_cards: BTreeSet::new(),
            refined_cards: None,
            next_reference: 1,
            next_identity: 1,
            next_root: 1,
            next_pin: 1,
            collection_requested: None,
            metrics: CollectorMetrics::default(),
        }
    }

    pub fn request_minor_collection(&mut self) {
        self.collection_requested = Some(crate::SchedulerId::new(1));
    }

    pub(crate) fn request_minor_collection_for(&mut self, scheduler: crate::SchedulerId) {
        self.collection_requested = Some(scheduler);
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        self.objects.contains_key(&reference)
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.objects.len()
    }

    #[must_use]
    pub fn dirty_card_count(&self) -> usize {
        self.dirty_cards.len()
    }

    pub(crate) fn install_refined_cards(
        &mut self,
        refined: BTreeMap<ManagedReference, Vec<ManagedReference>>,
    ) -> Result<(), RuntimeFailure> {
        if refined
            .keys()
            .any(|owner| !self.dirty_cards.contains(owner))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        for (owner, children) in &refined {
            if self.generation(*owner) != Some(CollectorGeneration::Mature)
                || children.iter().any(|child| {
                    !matches!(
                        self.generation(*child),
                        Some(CollectorGeneration::Nursery { .. })
                    )
                })
            {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        self.refined_cards = Some(refined);
        Ok(())
    }

    #[must_use]
    pub fn generation(&self, reference: ManagedReference) -> Option<CollectorGeneration> {
        self.objects.get(&reference).map(|object| object.generation)
    }

    #[must_use]
    pub fn object_identity(&self, reference: ManagedReference) -> Option<CollectorObjectId> {
        self.objects.get(&reference).map(|object| object.identity)
    }

    #[must_use]
    pub const fn metrics(&self) -> CollectorMetrics {
        self.metrics
    }

    /// Stores a precise managed edge and applies the generational card barrier.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid owner, slot, or value.
    pub fn store_reference(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        self.validate_reference(owner)?;
        if let Some(reference) = value {
            self.validate_reference(reference)?;
        }
        let previous = self.load_reference(owner, slot)?;
        self.apply_reference_barrier(owner, slot, previous, value)?;
        let object = self
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        object.allocation.slots[slot.raw() as usize] = SlotValue::Reference(value);
        Ok(())
    }

    pub(crate) fn slot_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<SlotValue, RuntimeFailure> {
        self.objects
            .get(&owner)
            .and_then(|object| object.allocation.slots.get(slot.raw() as usize))
            .copied()
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    pub(crate) fn store_validated_reference(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        previous: Option<ManagedReference>,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        let current = self
            .objects
            .get_mut(&owner)
            .and_then(|object| object.allocation.slots.get_mut(slot.raw() as usize))
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        match current {
            SlotValue::Reference(found) if *found == previous => {
                *found = value;
                Ok(())
            }
            SlotValue::Reference(_) | SlotValue::Scalar(_) => {
                Err(RuntimeFailure::runtime_invariant())
            }
        }
    }

    /// Loads a precise managed-reference slot.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid owner or non-reference slot.
    pub fn load_reference(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<Option<ManagedReference>, RuntimeFailure> {
        let object = self
            .objects
            .get(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        match object.allocation.slots.get(slot.raw() as usize) {
            Some(SlotValue::Reference(value)) => Ok(*value),
            Some(SlotValue::Scalar(_)) | None => Err(RuntimeFailure::runtime_invariant()),
        }
    }

    /// Stores a scalar without dirtying a generational card.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid owner or reference slot.
    pub fn store_scalar(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let object = self
            .objects
            .get_mut(&owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        match object.allocation.slots.get_mut(slot.raw() as usize) {
            Some(current @ SlotValue::Scalar(_)) => {
                *current = SlotValue::Scalar(value);
                Ok(())
            }
            Some(SlotValue::Reference(_)) | None => Err(RuntimeFailure::runtime_invariant()),
        }
    }

    pub(crate) fn discard_unpublished(
        &mut self,
        reference: ManagedReference,
    ) -> Result<(), RuntimeFailure> {
        if self.roots.values().any(|target| *target == reference)
            || self.pins.values().any(|target| *target == reference)
            || self.objects.values().any(|object| {
                object.allocation.slots.iter().any(|slot| {
                    matches!(slot, SlotValue::Reference(Some(target)) if *target == reference)
                })
            })
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.objects
            .remove(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.metrics.rollback_allocation();
        Ok(())
    }

    pub(super) fn fresh_reference(&mut self) -> Result<ManagedReference, RuntimeFailure> {
        let reference = ManagedReference::new(self.next_reference);
        self.next_reference = self
            .next_reference
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        Ok(reference)
    }

    pub(super) fn validate_reference(
        &self,
        reference: ManagedReference,
    ) -> Result<(), RuntimeFailure> {
        if self.contains(reference) {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }

    pub(super) fn apply_reference_barrier(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        previous: Option<ManagedReference>,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        if self.load_reference(owner, slot)? != previous {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let owner_is_mature = self.generation(owner) == Some(CollectorGeneration::Mature);
        let value_is_young = value.is_some_and(|reference| {
            matches!(
                self.generation(reference),
                Some(CollectorGeneration::Nursery { .. })
            )
        });
        if owner_is_mature && value_is_young {
            self.dirty_cards.insert(owner);
        }
        Ok(())
    }
}

impl Default for RelocationRuntime {
    fn default() -> Self {
        Self::new()
    }
}
