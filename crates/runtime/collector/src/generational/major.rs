//! Precise SATB marking and bounded mature sweeping.

use pop_runtime_interface::{
    CollectionStatistics, ManagedReference, RootPublication, RuntimeFailure,
};

use crate::{heap::SlotValue, relocation::CollectorGeneration};

use super::heap::{GenerationalRuntime, MajorCyclePhase};

impl GenerationalRuntime {
    pub(crate) fn begin_major(
        &mut self,
        publication: &RootPublication,
    ) -> Result<(), RuntimeFailure> {
        if self.major_cycle_active() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let mut pending: Vec<_> = publication.managed_references().collect();
        pending.extend(self.nursery.roots.values().copied());
        pending.extend(self.nursery.pins.values().copied());
        for reference in &pending {
            if !self.nursery.contains(*reference) {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        self.major.reset();
        self.major.phase = MajorCyclePhase::Marking;
        self.major.pending = pending;
        self.major_requested = false;
        Ok(())
    }

    pub(crate) fn advance_major(&mut self) -> Result<Option<CollectionStatistics>, RuntimeFailure> {
        self.advance_major_with_budget(self.config.work_budget())
            .map(|(statistics, _)| statistics)
    }

    pub(crate) fn advance_major_with_budget(
        &mut self,
        work_budget: usize,
    ) -> Result<(Option<CollectionStatistics>, usize), RuntimeFailure> {
        let mut remaining = work_budget;
        let mut completed_work = 0;
        while remaining > 0 {
            match self.major.phase {
                MajorCyclePhase::Idle => return Ok((None, completed_work)),
                MajorCyclePhase::Marking => {
                    if let Some(reference) =
                        self.major.satb.pop().or_else(|| self.major.pending.pop())
                    {
                        self.scan_snapshot_reference(reference)?;
                        remaining -= 1;
                        completed_work += 1;
                    } else {
                        self.prepare_sweep();
                    }
                }
                MajorCyclePhase::Sweeping => {
                    let Some(reference) = self.major.sweep.pop() else {
                        return Ok((Some(self.finish_major()), completed_work));
                    };
                    if self.nursery.objects.remove(&reference).is_some() {
                        self.major.reclaimed = self.major.reclaimed.saturating_add(1);
                    }
                    self.allocation.remove(reference);
                    self.nursery.dirty_cards.remove(&reference);
                    remaining -= 1;
                    completed_work += 1;
                }
            }
        }
        if self.major.phase == MajorCyclePhase::Sweeping && self.major.sweep.is_empty() {
            return Ok((Some(self.finish_major()), completed_work));
        }
        Ok((None, completed_work))
    }

    fn scan_snapshot_reference(
        &mut self,
        reference: ManagedReference,
    ) -> Result<(), RuntimeFailure> {
        if !self.major.seen.insert(reference) {
            return Ok(());
        }
        let object = self
            .nursery
            .objects
            .get(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if object.generation == CollectorGeneration::Mature {
            self.major.marked_mature.insert(reference);
        }
        for slot in object.allocation.object_map.reference_slots() {
            match object.allocation.slots.get(slot.raw() as usize) {
                Some(SlotValue::Reference(Some(child))) => self.major.pending.push(*child),
                Some(SlotValue::Reference(None)) => {}
                Some(SlotValue::Scalar(_)) | None => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            }
        }
        self.major.scanned = self.major.scanned.saturating_add(1);
        Ok(())
    }

    fn prepare_sweep(&mut self) {
        self.major.sweep = self
            .nursery
            .objects
            .iter()
            .filter_map(|(reference, object)| {
                (object.generation == CollectorGeneration::Mature
                    && !self.major.marked_mature.contains(reference))
                .then_some(*reference)
            })
            .collect();
        self.major.phase = MajorCyclePhase::Sweeping;
    }
}
