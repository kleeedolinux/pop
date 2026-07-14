//! SATB and generational managed-reference store barriers.

use pop_runtime_interface::{ManagedReference, ObjectSlot, RuntimeFailure};

use crate::relocation::CollectorGeneration;

use super::heap::{GenerationalRuntime, MajorCyclePhase};

impl GenerationalRuntime {
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
        let previous = self.nursery.load_reference(owner, slot)?;
        if value.is_some_and(|reference| !self.nursery.contains(reference)) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.record_satb(previous);
        self.record_post_scan_edge(owner, value);
        self.nursery.store_reference(owner, slot, value)
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
        if self.nursery.generation(reference) == Some(CollectorGeneration::Mature) {
            match self.major.phase {
                MajorCyclePhase::Marking => {
                    self.major.marked_mature.insert(reference);
                    self.major.pending.push(reference);
                }
                MajorCyclePhase::Sweeping => {
                    self.major.marked_mature.insert(reference);
                }
                MajorCyclePhase::Idle => {}
            }
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
