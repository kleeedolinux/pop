//! Precise SATB marking and bounded mature sweeping.

use std::ops::Bound::{Excluded, Unbounded};

use pop_runtime_interface::{
    CollectionStatistics, ManagedReference, RootPublication, RuntimeFailure,
};

use crate::{heap::SlotValue, relocation::CollectorGeneration};

use super::heap::{GenerationalRuntime, MajorCyclePhase};
use super::workers::MarkTask;

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
                    if self.workers.is_some() {
                        let work = self.advance_background_mark(remaining)?;
                        if work == 0 {
                            self.prepare_sweep();
                        } else {
                            remaining -= work;
                            completed_work += work;
                        }
                        continue;
                    }
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
                    if self.workers.is_some() {
                        let work = self.advance_background_sweep(remaining)?;
                        if work == 0 {
                            return Ok((Some(self.finish_major()), completed_work));
                        }
                        remaining -= work;
                        completed_work += work;
                        continue;
                    }
                    let Some((reference, reclaim)) = self.next_sweep_entry() else {
                        return Ok((Some(self.finish_major()), completed_work));
                    };
                    if reclaim && self.nursery.objects.remove(&reference).is_some() {
                        self.major.reclaimed = self.major.reclaimed.saturating_add(1);
                        self.allocation.remove(reference);
                        self.nursery.dirty_cards.remove(&reference);
                    }
                    remaining -= 1;
                    completed_work += 1;
                }
            }
        }
        if self.major.phase == MajorCyclePhase::Sweeping && self.major.sweep_complete {
            return Ok((Some(self.finish_major()), completed_work));
        }
        Ok((None, completed_work))
    }

    fn advance_background_mark(&mut self, work_budget: usize) -> Result<usize, RuntimeFailure> {
        let mut tasks = Vec::new();
        let mut completed_work = 0;
        while completed_work < work_budget {
            let Some(reference) = self.major.satb.pop().or_else(|| self.major.pending.pop()) else {
                break;
            };
            completed_work += 1;
            if !self.major.seen.insert(reference) {
                continue;
            }
            let object = self
                .nursery
                .objects
                .get(&reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            tasks.push(MarkTask {
                reference,
                generation: object.generation,
                allocation: object.allocation.clone(),
            });
        }
        if tasks.is_empty() {
            return Ok(completed_work);
        }
        let results = self
            .workers
            .as_mut()
            .ok_or_else(RuntimeFailure::runtime_invariant)?
            .mark(tasks)?;
        for result in results {
            if result.mature {
                self.major.marked_mature.insert(result.reference);
            }
            self.major.pending.extend(result.children);
            self.major.scanned = self.major.scanned.saturating_add(1);
        }
        Ok(completed_work)
    }

    fn advance_background_sweep(&mut self, work_budget: usize) -> Result<usize, RuntimeFailure> {
        let mut references = Vec::new();
        let mut completed_work = 0;
        while completed_work < work_budget {
            let Some((reference, reclaim)) = self.next_sweep_entry() else {
                break;
            };
            completed_work += 1;
            if reclaim {
                references.push(reference);
            }
        }
        if references.is_empty() {
            return Ok(completed_work);
        }
        let swept = self
            .workers
            .as_mut()
            .ok_or_else(RuntimeFailure::runtime_invariant)?
            .sweep(references)?;
        for reference in swept {
            if self.nursery.objects.remove(&reference).is_some() {
                self.major.reclaimed = self.major.reclaimed.saturating_add(1);
            }
            self.allocation.remove(reference);
            self.nursery.dirty_cards.remove(&reference);
        }
        Ok(completed_work)
    }

    fn next_sweep_entry(&mut self) -> Option<(ManagedReference, bool)> {
        let next = match self.major.sweep_cursor {
            Some(cursor) => self
                .nursery
                .objects
                .range((Excluded(cursor), Unbounded))
                .next(),
            None => self.nursery.objects.iter().next(),
        }
        .map(|(reference, object)| {
            (
                *reference,
                object.generation == CollectorGeneration::Mature
                    && !self.major.marked_mature.contains(reference),
            )
        });
        let Some((reference, reclaim)) = next else {
            self.major.sweep_complete = true;
            return None;
        };
        self.major.sweep_cursor = Some(reference);
        self.major.sweep_entries_examined = self.major.sweep_entries_examined.saturating_add(1);
        Some((reference, reclaim))
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
        self.major.phase = MajorCyclePhase::Sweeping;
        self.major.sweep_cursor = None;
        self.major.sweep_complete = false;
    }
}
