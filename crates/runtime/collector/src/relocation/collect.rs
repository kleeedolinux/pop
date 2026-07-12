//! Atomic single-mutator nursery evacuation and remembered-card scanning.

use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::{
    CollectionStatistics, ManagedReference, RootPublication, RuntimeFailure,
};

use crate::heap::SlotValue;

use super::heap::{CollectorGeneration, RelocationRuntime};

impl RelocationRuntime {
    pub(super) fn collect_minor(
        &mut self,
        publication: &mut RootPublication,
    ) -> Result<CollectionStatistics, RuntimeFailure> {
        let stack_roots: Vec<_> = publication.managed_references().collect();
        let handle_roots: Vec<_> = self.roots.values().copied().collect();
        let pin_roots: Vec<_> = self.pins.values().copied().collect();
        for reference in stack_roots.iter().chain(&handle_roots).chain(&pin_roots) {
            self.validate_reference(*reference)?;
        }

        let mut pending = stack_roots;
        pending.extend(handle_roots);
        pending.extend(pin_roots);
        for owner in &self.dirty_cards {
            let object = self
                .objects
                .get(owner)
                .filter(|object| object.generation == CollectorGeneration::Mature)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            for slot in object.allocation.object_map.reference_slots() {
                match object.allocation.slots.get(slot.raw() as usize) {
                    Some(SlotValue::Reference(Some(reference))) => pending.push(*reference),
                    Some(SlotValue::Reference(None)) => {}
                    Some(SlotValue::Scalar(_)) | None => {
                        return Err(RuntimeFailure::runtime_invariant());
                    }
                }
            }
        }

        let mut live_young = BTreeSet::new();
        while let Some(reference) = pending.pop() {
            let object = self
                .objects
                .get(&reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            if object.generation == CollectorGeneration::Mature || !live_young.insert(reference) {
                continue;
            }
            for slot in object.allocation.object_map.reference_slots() {
                match object.allocation.slots.get(slot.raw() as usize) {
                    Some(SlotValue::Reference(Some(child))) => pending.push(*child),
                    Some(SlotValue::Reference(None)) => {}
                    Some(SlotValue::Scalar(_)) | None => {
                        return Err(RuntimeFailure::runtime_invariant());
                    }
                }
            }
        }

        let young_before = self
            .objects
            .values()
            .filter(|object| matches!(object.generation, CollectorGeneration::Nursery { .. }))
            .count();
        let pinned: BTreeSet<_> = self.pins.values().copied().collect();
        let mut relocations = BTreeMap::new();
        for old in &live_young {
            relocations.insert(*old, self.fresh_reference()?);
        }

        let mut next_objects = BTreeMap::new();
        for (reference, object) in &self.objects {
            if object.generation == CollectorGeneration::Mature {
                next_objects.insert(*reference, object.clone());
            }
        }
        for old in &live_young {
            let mut object = self
                .objects
                .get(old)
                .cloned()
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            object.generation = match object.generation {
                CollectorGeneration::Nursery { age }
                    if pinned.contains(old) || age.saturating_add(1) >= 2 =>
                {
                    CollectorGeneration::Mature
                }
                CollectorGeneration::Nursery { age } => CollectorGeneration::Nursery {
                    age: age.saturating_add(1),
                },
                CollectorGeneration::Mature => return Err(RuntimeFailure::runtime_invariant()),
            };
            next_objects.insert(relocations[old], object);
        }

        for object in next_objects.values_mut() {
            for slot in &mut object.allocation.slots {
                if let SlotValue::Reference(Some(reference)) = slot
                    && let Some(relocated) = relocations.get(reference)
                {
                    *reference = *relocated;
                }
            }
        }

        let next_roots = relocate_handles(&self.roots, &relocations);
        let next_pins = relocate_handles(&self.pins, &relocations);
        let stack_updates: Vec<_> = publication
            .root_values()
            .map(|(_, value)| {
                value.map(|reference| relocations.get(&reference).copied().unwrap_or(reference))
            })
            .collect();
        let next_dirty_cards = remembered_cards(&next_objects);
        let reclaimed = young_before.saturating_sub(live_young.len());
        let statistics = CollectionStatistics::new(
            portable_count(next_objects.len()),
            portable_count(reclaimed),
            portable_count(live_young.len().saturating_add(self.dirty_cards.len())),
        );

        self.objects = next_objects;
        self.roots = next_roots;
        self.pins = next_pins;
        self.dirty_cards = next_dirty_cards;
        for ((_, value), update) in publication.root_values_mut().zip(stack_updates) {
            *value = update;
        }
        self.metrics
            .record_collection(statistics.reclaimed_objects(), statistics.scanned_objects());
        Ok(statistics)
    }
}

fn relocate_handles<Handle: Copy + Ord>(
    handles: &BTreeMap<Handle, ManagedReference>,
    relocations: &BTreeMap<ManagedReference, ManagedReference>,
) -> BTreeMap<Handle, ManagedReference> {
    handles
        .iter()
        .map(|(handle, reference)| {
            (
                *handle,
                relocations.get(reference).copied().unwrap_or(*reference),
            )
        })
        .collect()
}

fn remembered_cards(
    objects: &BTreeMap<ManagedReference, super::heap::RelocationAllocation>,
) -> BTreeSet<ManagedReference> {
    objects
        .iter()
        .filter_map(|(reference, object)| {
            (object.generation == CollectorGeneration::Mature
                && object.allocation.slots.iter().any(|slot| {
                    matches!(
                        slot,
                        SlotValue::Reference(Some(child))
                            if objects.get(child).is_some_and(|child| matches!(
                                child.generation,
                                CollectorGeneration::Nursery { .. }
                            ))
                    )
                }))
            .then_some(*reference)
        })
        .collect()
}

fn portable_count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
