//! Mutable page inventory, TLAB cursor, and relocation placement updates.

use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectMap, ObjectSlot, RuntimeFailure, RuntimeTypeId,
};

use crate::relocation::table::ObjectTable;
use crate::relocation::{CollectorGeneration, CollectorObjectId, RelocationAllocation};
use crate::{ObjectOwnership, SchedulerId};

use super::model::{
    AllocationInfrastructureConfig, AllocationMetrics, AllocationPlacement, HeapDomain,
    PageDescriptor, PageId, RegionId,
};
use super::{RegionKey, RegionRecord, RegionState};

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct LayoutKey {
    type_id: RuntimeTypeId,
    slot_count: u32,
    reference_slots: Vec<ObjectSlot>,
}

#[derive(Clone)]
struct Tlab {
    page: PageId,
    layout: LayoutKey,
    cursor: usize,
    limit: usize,
}

#[derive(Clone, Copy)]
struct MaturePageCursor {
    page: PageId,
    cursor: usize,
    limit: usize,
}

#[derive(Clone)]
pub(crate) struct AllocationInfrastructure {
    pub(super) config: AllocationInfrastructureConfig,
    pub(super) pages: BTreeMap<PageId, PageDescriptor>,
    page_cursors: BTreeMap<PageId, usize>,
    active_mature_page: Option<(LayoutKey, SchedulerId, MaturePageCursor)>,
    mature_pages: BTreeMap<(LayoutKey, SchedulerId), MaturePageCursor>,
    pub(super) placements: ObjectTable<AllocationPlacement>,
    tlabs: BTreeMap<SchedulerId, Tlab>,
    pub(super) regions: BTreeMap<RegionId, RegionRecord>,
    pub(super) active_regions: BTreeMap<RegionKey, BTreeSet<RegionId>>,
    committed_bytes: usize,
    next_page: u64,
    pub(super) next_region: u64,
    pub(super) shared_region_state: RegionState,
    metrics: AllocationMetrics,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PlacementRequirement {
    pub(crate) object_bytes: usize,
    pub(crate) additional_committed_bytes: usize,
}

impl AllocationInfrastructure {
    pub(crate) fn new(config: AllocationInfrastructureConfig) -> Self {
        Self {
            config,
            pages: BTreeMap::new(),
            page_cursors: BTreeMap::new(),
            active_mature_page: None,
            mature_pages: BTreeMap::new(),
            placements: ObjectTable::new(),
            tlabs: BTreeMap::new(),
            regions: BTreeMap::new(),
            active_regions: BTreeMap::new(),
            committed_bytes: 0,
            next_page: 1,
            next_region: 1,
            shared_region_state: RegionState::SharedAllocating,
            metrics: AllocationMetrics::default(),
        }
    }

    pub(crate) fn place(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        object_map: &ObjectMap,
        scheduler: SchedulerId,
    ) -> Result<(), RuntimeFailure> {
        let layout = layout(type_id, object_map);
        let size = object_size(object_map.slot_count())?;
        let placement = match class {
            AllocationClass::NurseryEligible => self.place_in_tlab(&layout, size, scheduler)?,
            AllocationClass::Mature => self.place_in_mature_page(&layout, size, scheduler)?,
            AllocationClass::Large | AllocationClass::Pinned => {
                let domain = domain_for_class(class);
                self.place_on_new_page(&layout, size, domain, None)?
            }
        };
        self.record_bytes(size);
        self.placements.insert(reference, placement);
        Ok(())
    }

    pub(crate) fn placement_requirement(
        &self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        object_map: &ObjectMap,
        scheduler: SchedulerId,
    ) -> Result<PlacementRequirement, RuntimeFailure> {
        let layout = layout(type_id, object_map);
        let object_bytes = object_size(object_map.slot_count())?;
        let reuses_capacity = (class == AllocationClass::NurseryEligible
            && self.tlabs.get(&scheduler).is_some_and(|tlab| {
                tlab.layout == layout && tlab.cursor.saturating_add(object_bytes) <= tlab.limit
            }))
            || (class == AllocationClass::Mature
                && self
                    .indexed_mature_page(&layout, object_bytes, scheduler)
                    .is_some());
        let additional_committed_bytes = if reuses_capacity {
            0
        } else {
            self.config.page_bytes.max(object_bytes)
        };
        Ok(PlacementRequirement {
            object_bytes,
            additional_committed_bytes,
        })
    }

    fn place_in_tlab(
        &mut self,
        layout: &LayoutKey,
        size: usize,
        scheduler: SchedulerId,
    ) -> Result<AllocationPlacement, RuntimeFailure> {
        let refill = self.tlabs.get(&scheduler).is_none_or(|tlab| {
            &tlab.layout != layout || tlab.cursor.saturating_add(size) > tlab.limit
        });
        if refill {
            let page = self.create_page(
                layout,
                HeapDomain::LocalEden,
                self.config.page_bytes.max(size),
                Some(scheduler),
            )?;
            self.tlabs.insert(
                scheduler,
                Tlab {
                    page,
                    layout: layout.clone(),
                    cursor: 0,
                    limit: self.config.tlab_bytes.max(size),
                },
            );
            self.metrics.tlab_refills = self.metrics.tlab_refills.saturating_add(1);
        }
        let tlab = self
            .tlabs
            .get_mut(&scheduler)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let offset = tlab.cursor;
        tlab.cursor = tlab
            .cursor
            .checked_add(size)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.page_cursors.insert(tlab.page, tlab.cursor);
        self.metrics.tlab_allocations = self.metrics.tlab_allocations.saturating_add(1);
        Ok(AllocationPlacement {
            page: tlab.page,
            offset_bytes: offset,
            size_bytes: size,
            domain: HeapDomain::LocalEden,
        })
    }

    fn place_on_new_page(
        &mut self,
        layout: &LayoutKey,
        size: usize,
        domain: HeapDomain,
        scheduler: Option<SchedulerId>,
    ) -> Result<AllocationPlacement, RuntimeFailure> {
        let page = self.create_page(layout, domain, self.config.page_bytes.max(size), scheduler)?;
        self.page_cursors.insert(page, size);
        Ok(AllocationPlacement {
            page,
            offset_bytes: 0,
            size_bytes: size,
            domain,
        })
    }

    fn place_in_mature_page(
        &mut self,
        layout: &LayoutKey,
        size: usize,
        scheduler: SchedulerId,
    ) -> Result<AllocationPlacement, RuntimeFailure> {
        let key = (layout.clone(), scheduler);
        self.activate_mature_page(&key);
        if let Some((active_layout, active_scheduler, active)) = &mut self.active_mature_page
            && active_layout == layout
            && *active_scheduler == scheduler
            && let Some(cursor) = active.cursor.checked_add(size)
            && cursor <= active.limit
        {
            let page = active.page;
            let offset_bytes = active.cursor;
            let limit = active.limit;
            active.cursor = cursor;
            self.metrics.mature_page_index_hits =
                self.metrics.mature_page_index_hits.saturating_add(1);
            if cursor == limit {
                self.page_cursors.insert(page, cursor);
                self.active_mature_page = None;
            }
            return Ok(AllocationPlacement {
                page,
                offset_bytes,
                size_bytes: size,
                domain: HeapDomain::LocalMature,
            });
        }
        self.active_mature_page = None;

        let page = self.create_page(
            layout,
            HeapDomain::LocalMature,
            self.config.page_bytes.max(size),
            Some(scheduler),
        )?;
        let limit = self
            .pages
            .get(&page)
            .map(|descriptor| descriptor.capacity_bytes)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let cursor = size.min(limit);
        if cursor < limit {
            self.active_mature_page = Some((
                layout.clone(),
                scheduler,
                MaturePageCursor {
                    page,
                    cursor,
                    limit,
                },
            ));
        } else {
            self.page_cursors.insert(page, cursor);
        }
        Ok(AllocationPlacement {
            page,
            offset_bytes: 0,
            size_bytes: size,
            domain: HeapDomain::LocalMature,
        })
    }

    fn indexed_mature_page(
        &self,
        layout: &LayoutKey,
        size: usize,
        scheduler: SchedulerId,
    ) -> Option<(PageId, usize, usize)> {
        if let Some((active_layout, active_scheduler, active)) = &self.active_mature_page
            && active_layout == layout
            && *active_scheduler == scheduler
        {
            return active
                .cursor
                .checked_add(size)
                .is_some_and(|end| end <= active.limit)
                .then_some((active.page, active.cursor, active.limit));
        }
        let active = self.mature_pages.get(&(layout.clone(), scheduler))?;
        active
            .cursor
            .checked_add(size)
            .is_some_and(|end| end <= active.limit)
            .then_some((active.page, active.cursor, active.limit))
    }

    fn activate_mature_page(&mut self, key: &(LayoutKey, SchedulerId)) {
        if self
            .active_mature_page
            .as_ref()
            .is_some_and(|(layout, scheduler, _)| layout == &key.0 && *scheduler == key.1)
        {
            return;
        }
        if let Some((layout, scheduler, active)) = self.active_mature_page.take() {
            self.page_cursors.insert(active.page, active.cursor);
            self.mature_pages.insert((layout, scheduler), active);
        }
        if let Some(active) = self.mature_pages.remove(key) {
            self.active_mature_page = Some((key.0.clone(), key.1, active));
        }
    }

    fn reusable_page(
        &self,
        layout: &LayoutKey,
        size: usize,
        domain: HeapDomain,
        scheduler: Option<SchedulerId>,
    ) -> Option<(PageId, usize)> {
        self.pages.values().find_map(|page| {
            let region = self.regions.get(&page.region)?;
            let cursor = self.page_cursors.get(&page.id).copied().unwrap_or(0);
            (page.domain == domain
                && page.scheduler == scheduler
                && region.state.accepts_allocation()
                && page.type_id == layout.type_id
                && page.slot_count == layout.slot_count
                && page.reference_slots == layout.reference_slots
                && cursor
                    .checked_add(size)
                    .is_some_and(|end| end <= page.capacity_bytes))
            .then_some((page.id, cursor))
        })
    }

    fn create_page(
        &mut self,
        layout: &LayoutKey,
        domain: HeapDomain,
        capacity_bytes: usize,
        scheduler: Option<SchedulerId>,
    ) -> Result<PageId, RuntimeFailure> {
        let committed_bytes = self
            .committed_bytes
            .checked_add(capacity_bytes)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let id = PageId(self.next_page);
        self.next_page = self
            .next_page
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let region = self.acquire_region(domain, scheduler, capacity_bytes)?;
        self.pages.insert(
            id,
            PageDescriptor {
                id,
                region,
                domain,
                scheduler,
                type_id: layout.type_id,
                slot_count: layout.slot_count,
                reference_slots: layout.reference_slots.clone(),
                capacity_bytes,
            },
        );
        self.page_cursors.insert(id, 0);
        let (key, full) = {
            let record = self
                .regions
                .get_mut(&region)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            record.committed_bytes = record
                .committed_bytes
                .checked_add(capacity_bytes)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            (record.key, record.committed_bytes >= record.capacity_bytes)
        };
        if full {
            self.remove_active_region(key, region);
        }
        self.committed_bytes = committed_bytes;
        self.metrics.pages_created = self.metrics.pages_created.saturating_add(1);
        Ok(id)
    }

    pub(crate) fn placement(&self, reference: ManagedReference) -> Option<AllocationPlacement> {
        self.placements.get(&reference).copied()
    }

    pub(crate) fn region(&self, reference: ManagedReference) -> Option<RegionId> {
        self.placement(reference)
            .and_then(|placement| self.pages.get(&placement.page))
            .map(|page| page.region)
    }

    pub(crate) fn page(&self, page: PageId) -> Option<&PageDescriptor> {
        self.pages.get(&page)
    }

    pub(crate) const fn metrics(&self) -> AllocationMetrics {
        self.metrics
    }

    pub(crate) fn tlab_top_bytes(&self, scheduler: SchedulerId) -> usize {
        self.tlabs.get(&scheduler).map_or(0, |tlab| tlab.cursor)
    }

    pub(crate) fn remove(&mut self, reference: ManagedReference) {
        self.remove_without_page_reclamation(reference);
        self.reclaim_empty_pages();
    }

    pub(crate) fn remove_without_page_reclamation(&mut self, reference: ManagedReference) {
        self.placements.remove(&reference);
    }

    pub(crate) fn reclaim_empty_pages_after_sweep(&mut self) {
        self.reclaim_empty_pages();
    }

    pub(crate) fn live_bytes(&self) -> usize {
        self.placements.values().fold(0, |total, placement| {
            total.saturating_add(placement.size_bytes)
        })
    }

    pub(crate) fn committed_bytes(&self) -> usize {
        self.committed_bytes
    }

    pub(crate) fn bytes_in_domains(&self, domains: &[HeapDomain]) -> usize {
        self.placements.values().fold(0, |total, placement| {
            if domains.contains(&placement.domain) {
                total.saturating_add(placement.size_bytes)
            } else {
                total
            }
        })
    }

    pub(crate) fn move_to_pinned(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        object_map: &ObjectMap,
    ) -> Result<(), RuntimeFailure> {
        let Some(previous) = self.placements.get(&reference).copied() else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if previous.domain == HeapDomain::Pinned {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.slot_count)?;
        let placement = self.place_on_new_page(&layout, size, HeapDomain::Pinned, None)?;
        self.placements.insert(reference, placement);
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn move_to_shared(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        object_map: &ObjectMap,
    ) -> Result<(), RuntimeFailure> {
        let Some(previous) = self.placements.get(&reference).copied() else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if matches!(
            previous.domain,
            HeapDomain::Shared | HeapDomain::LargeObject | HeapDomain::Pinned
        ) {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.slot_count)?;
        let placement = self.place_on_new_page(&layout, size, HeapDomain::Shared, None)?;
        self.placements.insert(reference, placement);
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn move_to_isolated(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        object_map: &ObjectMap,
    ) -> Result<(), RuntimeFailure> {
        let Some(previous) = self.placements.get(&reference).copied() else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if matches!(
            previous.domain,
            HeapDomain::Isolated | HeapDomain::LargeObject | HeapDomain::Pinned
        ) {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.slot_count)?;
        let placement = self.place_on_new_page(&layout, size, HeapDomain::Isolated, None)?;
        self.placements.insert(reference, placement);
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn move_to_local_mature(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        object_map: &ObjectMap,
        scheduler: SchedulerId,
    ) -> Result<(), RuntimeFailure> {
        let Some(previous) = self.placements.get(&reference).copied() else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if matches!(
            previous.domain,
            HeapDomain::LocalMature | HeapDomain::LargeObject | HeapDomain::Pinned
        ) {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.slot_count)?;
        let placement =
            self.place_on_new_page(&layout, size, HeapDomain::LocalMature, Some(scheduler))?;
        self.placements.insert(reference, placement);
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn reconcile_after_minor(
        &mut self,
        previous_identities: &BTreeMap<CollectorObjectId, ManagedReference>,
        objects: &ObjectTable<RelocationAllocation>,
        scheduler: SchedulerId,
    ) -> Result<(), RuntimeFailure> {
        let mut previous = std::mem::take(&mut self.placements);
        let mut next = ObjectTable::new();
        for (reference, object) in objects.iter() {
            if let Some(placement) = previous.remove(reference) {
                next.insert(*reference, placement);
                continue;
            }
            let old_reference = previous_identities
                .get(&object.identity)
                .copied()
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            previous
                .remove(&old_reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            let domain = match object.generation {
                CollectorGeneration::Nursery { .. } => HeapDomain::LocalSurvivor,
                CollectorGeneration::Mature => HeapDomain::LocalMature,
            };
            let layout = layout(object.allocation.type_id, &object.allocation.object_map);
            let size = object_size(layout.slot_count)?;
            let object_scheduler = match object.ownership {
                ObjectOwnership::SchedulerLocal(owner) if owner == scheduler => owner,
                ObjectOwnership::SchedulerLocal(_)
                | ObjectOwnership::Isolated(_)
                | ObjectOwnership::Shared => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            };
            let placement =
                self.place_on_new_page(&layout, size, domain, Some(object_scheduler))?;
            next.insert(*reference, placement);
            self.record_bytes(size);
            match domain {
                HeapDomain::LocalSurvivor => {
                    self.metrics.survivor_copies = self.metrics.survivor_copies.saturating_add(1);
                }
                HeapDomain::LocalMature => {
                    self.metrics.promotions = self.metrics.promotions.saturating_add(1);
                }
                HeapDomain::LocalEden
                | HeapDomain::Isolated
                | HeapDomain::Shared
                | HeapDomain::LargeObject
                | HeapDomain::Pinned => {}
            }
        }
        self.placements = next;
        self.tlabs.remove(&scheduler);
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn reconcile_after_evacuation(
        &mut self,
        relocations: &BTreeMap<ManagedReference, ManagedReference>,
        objects: &ObjectTable<RelocationAllocation>,
    ) -> Result<usize, RuntimeFailure> {
        let selected_regions = relocations
            .keys()
            .map(|reference| {
                self.region(*reference)
                    .ok_or_else(RuntimeFailure::runtime_invariant)
            })
            .collect::<Result<BTreeSet<_>, _>>()?;
        if selected_regions.is_empty()
            || selected_regions.iter().any(|region| {
                !self.regions.get(region).is_some_and(|record| {
                    record.key.domain == HeapDomain::Shared
                        && record.state == RegionState::EvacuationCandidate
                })
            })
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        for region in &selected_regions {
            self.regions
                .get_mut(region)
                .ok_or_else(RuntimeFailure::runtime_invariant)?
                .state = RegionState::Evacuating;
        }
        self.rebuild_active_regions();

        for (old, new) in relocations {
            let old_placement = self
                .placement(*old)
                .filter(|placement| placement.domain == HeapDomain::Shared)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            let old_region = self
                .pages
                .get(&old_placement.page)
                .map(|page| page.region)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            if !selected_regions.contains(&old_region) {
                return Err(RuntimeFailure::runtime_invariant());
            }
            let object = objects
                .get(new)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            let layout = layout(object.allocation.type_id, &object.allocation.object_map);
            let size = object_size(layout.slot_count)?;
            let placement = self.place_compacted_shared(&layout, size)?;
            self.placements.insert(*new, placement);
        }
        let peak_committed_bytes = self.committed_bytes();

        for old in relocations.keys() {
            self.placements.remove(old);
        }
        for region in &selected_regions {
            self.regions
                .get_mut(region)
                .ok_or_else(RuntimeFailure::runtime_invariant)?
                .state = RegionState::Quarantined;
        }
        self.reclaim_empty_pages();
        Ok(peak_committed_bytes)
    }

    fn place_compacted_shared(
        &mut self,
        layout: &LayoutKey,
        size: usize,
    ) -> Result<AllocationPlacement, RuntimeFailure> {
        let reusable = self.reusable_page(layout, size, HeapDomain::Shared, None);
        let (page, offset_bytes) = if let Some(reusable) = reusable {
            reusable
        } else {
            (
                self.create_page(
                    layout,
                    HeapDomain::Shared,
                    self.config.page_bytes.max(size),
                    None,
                )?,
                0,
            )
        };
        self.page_cursors
            .insert(page, offset_bytes.saturating_add(size));
        Ok(AllocationPlacement {
            page,
            offset_bytes,
            size_bytes: size,
            domain: HeapDomain::Shared,
        })
    }

    fn record_bytes(&mut self, size: usize) {
        self.metrics.allocated_bytes = self
            .metrics
            .allocated_bytes
            .saturating_add(u64::try_from(size).unwrap_or(u64::MAX));
    }

    fn reclaim_empty_pages(&mut self) {
        self.metrics.page_reclamation_passes =
            self.metrics.page_reclamation_passes.saturating_add(1);
        let live_pages: BTreeSet<_> = self
            .placements
            .values()
            .map(|placement| placement.page)
            .collect();
        let before = self.pages.len();
        let returned_bytes = self
            .pages
            .iter()
            .fold(0_usize, |total, (page, descriptor)| {
                if live_pages.contains(page) {
                    total
                } else {
                    total.saturating_add(descriptor.capacity_bytes)
                }
            });
        self.pages.retain(|page, _| live_pages.contains(page));
        self.committed_bytes = self.committed_bytes.saturating_sub(returned_bytes);
        self.page_cursors
            .retain(|page, _| self.pages.contains_key(page));
        self.mature_pages
            .retain(|_, active| self.pages.contains_key(&active.page));
        if self
            .active_mature_page
            .as_ref()
            .is_some_and(|(_, _, active)| !self.pages.contains_key(&active.page))
        {
            self.active_mature_page = None;
        }
        let returned = before.saturating_sub(self.pages.len());
        self.metrics.pages_returned = self
            .metrics
            .pages_returned
            .saturating_add(u64::try_from(returned).unwrap_or(u64::MAX));
        self.tlabs
            .retain(|_, tlab| self.pages.contains_key(&tlab.page));
        for region in self.regions.values_mut() {
            region.committed_bytes = 0;
        }
        for page in self.pages.values() {
            if let Some(region) = self.regions.get_mut(&page.region) {
                region.committed_bytes = region.committed_bytes.saturating_add(page.capacity_bytes);
            }
        }
        self.regions.retain(|_, region| region.committed_bytes != 0);
        self.rebuild_active_regions();
    }
}

fn layout(type_id: RuntimeTypeId, object_map: &ObjectMap) -> LayoutKey {
    LayoutKey {
        type_id,
        slot_count: object_map.slot_count(),
        reference_slots: object_map.reference_slots().to_vec(),
    }
}

pub(crate) fn object_size(slot_count: u32) -> Result<usize, RuntimeFailure> {
    usize::try_from(slot_count)
        .map_err(|_| RuntimeFailure::runtime_invariant())?
        .checked_mul(8)
        .map(|size| size.max(8))
        .ok_or_else(RuntimeFailure::runtime_invariant)
}

const fn domain_for_class(class: AllocationClass) -> HeapDomain {
    match class {
        AllocationClass::NurseryEligible => HeapDomain::LocalEden,
        AllocationClass::Mature => HeapDomain::LocalMature,
        AllocationClass::Large => HeapDomain::LargeObject,
        AllocationClass::Pinned => HeapDomain::Pinned,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_segmented_placement_storage(_: &ObjectTable<AllocationPlacement>) {}

    #[test]
    fn placement_metadata_uses_token_segment_storage() {
        let allocation = AllocationInfrastructure::new(AllocationInfrastructureConfig::default());

        assert_segmented_placement_storage(&allocation.placements);
    }
}
