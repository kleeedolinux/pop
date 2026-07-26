//! SATB and generational managed-reference store barriers.

use pop_runtime_interface::{ManagedReference, ObjectSlot, RuntimeFailure};

use crate::relocation::CollectorGeneration;

use super::MutatorId;
use super::heap::{GenerationalRuntime, MajorCyclePhase};

impl GenerationalRuntime {
    /// Loads one precise managed edge.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid owner or non-reference slot.
    pub fn load_reference(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<Option<ManagedReference>, RuntimeFailure> {
        self.nursery.load_reference(owner, slot)
    }

    /// Stores one precise managed edge through SATB and card barriers.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid owner, slot, or target.
    pub fn store_reference(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        self.ensure_mutable(owner)?;
        let previous = self.nursery.load_reference(owner, slot)?;
        if value.is_some_and(|reference| !self.nursery.contains(reference)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.validate_ownership_edge(owner, value)?;
        self.record_satb(previous);
        self.record_post_scan_edge(owner, value);
        self.nursery.store_reference(owner, slot, value)?;
        self.reference_mutation_version = self.reference_mutation_version.wrapping_add(1);
        Ok(())
    }

    /// Stores one precise edge while buffering concurrent-mark references for
    /// the exact registered mutator.
    ///
    /// # Errors
    ///
    /// Rejects an unknown mutator, scheduler mismatch, invalid edge, or
    /// immutable owner before changing heap state.
    pub fn store_reference_for_mutator(
        &mut self,
        mutator: MutatorId,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        if self.mutator_scheduler(mutator) != Some(self.scheduler) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.ensure_mutable(owner)?;
        let previous = self.nursery.load_reference(owner, slot)?;
        if value.is_some_and(|reference| !self.nursery.contains(reference)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.validate_ownership_edge(owner, value)?;
        if self.major.phase == MajorCyclePhase::Marking {
            let mut references = Vec::with_capacity(2);
            if previous.is_some_and(|reference| {
                self.nursery.generation(reference) == Some(CollectorGeneration::Mature)
            }) {
                references.extend(previous);
            }
            if self.major.marked_mature.contains(&owner)
                && self.major.seen.contains(&owner)
                && value.is_some()
            {
                references.extend(value);
            }
            self.buffer_mutator_barrier_references(mutator, references);
        }
        self.nursery.store_reference(owner, slot, value)?;
        self.reference_mutation_version = self.reference_mutation_version.wrapping_add(1);
        Ok(())
    }

    #[must_use]
    pub fn buffered_barrier_entries(&self, mutator: MutatorId) -> usize {
        self.major.barrier_buffers.get(&mutator).map_or(0, Vec::len)
    }

    fn buffer_mutator_barrier_references(
        &mut self,
        mutator: MutatorId,
        references: Vec<ManagedReference>,
    ) {
        if references.is_empty() {
            return;
        }
        let capacity = self.config.barrier_buffer_capacity();
        let buffer = self.major.barrier_buffers.entry(mutator).or_default();
        for reference in references {
            if buffer.len() == capacity {
                self.major.satb.append(buffer);
            }
            buffer.push(reference);
        }
    }

    pub(crate) fn drain_mutator_barrier_buffers(&mut self) {
        for buffer in self.major.barrier_buffers.values_mut() {
            self.major.satb.append(buffer);
        }
    }

    pub(crate) fn record_satb(&mut self, previous: Option<ManagedReference>) {
        if self.major.phase != MajorCyclePhase::Marking {
            return;
        }
        if let Some(reference) = previous
            && self.nursery.generation(reference) == Some(CollectorGeneration::Mature)
        {
            self.major.satb.push(reference);
        }
    }

    pub(crate) fn shade_new_root(&mut self, reference: ManagedReference) {
        if self.major.phase == MajorCyclePhase::Marking {
            self.major.pending.push(reference);
        }
    }

    pub(crate) fn mark_new_allocation(&mut self, reference: ManagedReference) {
        match self.major.phase {
            MajorCyclePhase::Marking
                if self.nursery.generation(reference) == Some(CollectorGeneration::Mature) =>
            {
                self.major.marked_mature.insert(reference);
                self.major.pending.push(reference);
            }
            MajorCyclePhase::Sweeping
                if self.nursery.generation(reference) == Some(CollectorGeneration::Mature) =>
            {
                self.major.marked_mature.insert(reference);
            }
            MajorCyclePhase::Idle | MajorCyclePhase::Marking | MajorCyclePhase::Sweeping => {}
        }
    }

    pub(crate) fn mark_new_allocation_range(
        &mut self,
        first: ManagedReference,
        last: ManagedReference,
    ) {
        match self.major.phase {
            MajorCyclePhase::Marking => {
                for raw in first.raw()..=last.raw() {
                    let reference = ManagedReference::new(raw);
                    self.major.marked_mature.insert(reference);
                    self.major.pending.push(reference);
                }
            }
            MajorCyclePhase::Sweeping => {
                for raw in first.raw()..=last.raw() {
                    self.major.marked_mature.insert(ManagedReference::new(raw));
                }
            }
            MajorCyclePhase::Idle => {}
        }
    }

    pub(crate) fn record_post_scan_edge(
        &mut self,
        owner: ManagedReference,
        value: Option<ManagedReference>,
    ) {
        if self.major.phase == MajorCyclePhase::Marking
            && self.major.marked_mature.contains(&owner)
            && self.major.seen.contains(&owner)
            && let Some(reference) = value
        {
            self.major.pending.push(reference);
        }
    }
}
