//! Precise SATB marking and bounded mature sweeping.

use pop_runtime_interface::{
    AllocationClass, CollectionStatistics, ManagedReference, RootPublication, RuntimeFailure,
};

use crate::{heap::SlotValue, relocation::CollectorGeneration};

use super::allocation::RegionState;
use super::heap::{GenerationalRuntime, LargeObjectScanChunk, MajorCyclePhase};
use super::workers::{MarkTask, scan_slots};

#[derive(Clone, Copy)]
enum MarkWork {
    Discover(ManagedReference),
    ScanLargeObject(LargeObjectScanChunk),
}

impl GenerationalRuntime {
    pub(crate) fn begin_major(
        &mut self,
        publication: &RootPublication,
    ) -> Result<(), RuntimeFailure> {
        self.begin_major_references(publication.managed_references().collect())
    }

    pub(crate) fn validate_major_references(
        &self,
        references: &[ManagedReference],
    ) -> Result<(), RuntimeFailure> {
        if self.major_cycle_active() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        if references
            .iter()
            .chain(self.nursery.roots.values())
            .chain(self.nursery.pins.values())
            .all(|reference| self.nursery.contains(*reference))
        {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }

    pub(crate) fn begin_major_references(
        &mut self,
        mut pending: Vec<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        self.validate_major_references(&pending)?;
        pending.extend(self.nursery.roots.values().copied());
        pending.extend(self.nursery.pins.values().copied());
        self.major.reset();
        self.allocation
            .transition_shared_regions(RegionState::SharedMarking);
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
                    if let Some(work) = self.next_mark_work() {
                        if let Some(task) = self.prepare_mark_task(work)? {
                            let children = scan_slots(&task.slots);
                            self.apply_mark_result(
                                task.reference,
                                children,
                                task.large_object_scan_chunk,
                            )?;
                        }
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
                        self.allocation.remove_without_page_reclamation(reference);
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
            let Some(work) = self.next_mark_work() else {
                break;
            };
            completed_work += 1;
            if let Some(task) = self.prepare_mark_task(work)? {
                tasks.push(task);
            }
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
            self.apply_mark_result(
                result.reference,
                result.children,
                result.large_object_scan_chunk,
            )?;
        }
        Ok(completed_work)
    }

    fn next_mark_work(&mut self) -> Option<MarkWork> {
        if let Some(reference) = self.major.satb.pop() {
            return Some(MarkWork::Discover(reference));
        }
        let has_chunk = !self.major.pending_large_object_scan_chunks.is_empty();
        let has_reference = !self.major.pending.is_empty();
        if has_chunk && (!has_reference || self.major.prefer_large_object_scan_chunk) {
            self.major.prefer_large_object_scan_chunk = false;
            return self
                .major
                .pending_large_object_scan_chunks
                .pop()
                .map(MarkWork::ScanLargeObject);
        }
        if has_reference {
            self.major.prefer_large_object_scan_chunk = true;
            return self.major.pending.pop().map(MarkWork::Discover);
        }
        None
    }

    fn prepare_mark_task(&mut self, work: MarkWork) -> Result<Option<MarkTask>, RuntimeFailure> {
        match work {
            MarkWork::Discover(reference) => self.discover_mark_reference(reference),
            MarkWork::ScanLargeObject(chunk) => {
                let slots =
                    self.snapshot_reference_slots(chunk.reference, chunk.start, chunk.end)?;
                Ok(Some(MarkTask {
                    reference: chunk.reference,
                    slots,
                    large_object_scan_chunk: Some(chunk),
                }))
            }
        }
    }

    fn discover_mark_reference(
        &mut self,
        reference: ManagedReference,
    ) -> Result<Option<MarkTask>, RuntimeFailure> {
        if !self.major.seen.insert(reference) {
            return Ok(None);
        }
        let object = self
            .nursery
            .objects
            .get(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let generation = object.generation;
        let class = object.allocation.class;
        let reference_slot_count = object.allocation.object_map.reference_slots().len();
        if generation == CollectorGeneration::Mature {
            self.major.marked_mature.insert(reference);
        }
        self.major.scanned = self.major.scanned.saturating_add(1);

        if class == AllocationClass::Large && reference_slot_count == 0 {
            self.major_telemetry.record_pointer_free_large_object();
            return Ok(None);
        }
        let chunk_slots = self.config.large_object_scan_chunk_slots();
        if reference_slot_count > 0
            && (class == AllocationClass::Large || reference_slot_count > chunk_slots)
        {
            self.enqueue_first_large_object_scan_chunk(reference, reference_slot_count);
            return Ok(None);
        }
        let slots = self.snapshot_reference_slots(reference, 0, reference_slot_count)?;
        Ok(Some(MarkTask {
            reference,
            slots,
            large_object_scan_chunk: None,
        }))
    }

    fn enqueue_first_large_object_scan_chunk(
        &mut self,
        reference: ManagedReference,
        reference_slot_count: usize,
    ) {
        let chunk_slots = self.config.large_object_scan_chunk_slots();
        self.major
            .pending_large_object_scan_chunks
            .push(LargeObjectScanChunk {
                reference,
                start: 0,
                end: chunk_slots.min(reference_slot_count),
            });
        self.major_telemetry.record_large_object_scan_queue_depth(
            self.major.pending_large_object_scan_chunks.len(),
        );
    }

    fn apply_mark_result(
        &mut self,
        reference: ManagedReference,
        children: Vec<ManagedReference>,
        large_object_scan_chunk: Option<LargeObjectScanChunk>,
    ) -> Result<(), RuntimeFailure> {
        let object = self
            .nursery
            .objects
            .get(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let reference_slot_count = object.allocation.object_map.reference_slots().len();
        self.major.pending.extend(children);
        if let Some(chunk) = large_object_scan_chunk {
            self.major_telemetry
                .record_large_object_scan_chunk(chunk.slots());
            if chunk.end < reference_slot_count {
                self.major
                    .pending_large_object_scan_chunks
                    .push(LargeObjectScanChunk {
                        reference,
                        start: chunk.end,
                        end: chunk
                            .end
                            .saturating_add(self.config.large_object_scan_chunk_slots())
                            .min(reference_slot_count),
                    });
                self.major_telemetry.record_large_object_scan_queue_depth(
                    self.major.pending_large_object_scan_chunks.len(),
                );
            }
        }
        Ok(())
    }

    fn snapshot_reference_slots(
        &self,
        reference: ManagedReference,
        start: usize,
        end: usize,
    ) -> Result<Vec<SlotValue>, RuntimeFailure> {
        let object = self
            .nursery
            .objects
            .get(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let reference_slots = object
            .allocation
            .object_map
            .reference_slots()
            .get(start..end)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let mut slots = Vec::with_capacity(reference_slots.len());
        for slot in reference_slots {
            let value = object
                .allocation
                .slots
                .get(slot.raw() as usize)
                .copied()
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            slots.push(value);
        }
        Ok(slots)
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
            self.allocation.remove_without_page_reclamation(reference);
            self.nursery.dirty_cards.remove(&reference);
        }
        Ok(completed_work)
    }

    fn next_sweep_entry(&mut self) -> Option<(ManagedReference, bool)> {
        let next = self
            .nursery
            .objects
            .next_after(self.major.sweep_cursor)
            .map(|(reference, object)| {
                (
                    reference,
                    object.generation == CollectorGeneration::Mature
                        && !self.major.marked_mature.contains(&reference),
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

    fn prepare_sweep(&mut self) {
        self.allocation
            .transition_shared_regions(RegionState::SharedSweeping);
        self.major.phase = MajorCyclePhase::Sweeping;
        self.major.sweep_cursor = None;
        self.major.sweep_complete = false;
    }
}
