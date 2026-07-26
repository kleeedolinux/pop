//! Mutable page inventory, TLAB cursor, and relocation placement updates.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectMap, RuntimeFailure, RuntimeTypeId,
};

use crate::heap::{Allocation, AllocationKind, PageWords, SlotValue, zeroed_page_words};
use crate::relocation::table::ObjectTable;
use crate::relocation::{CollectorGeneration, CollectorObjectId, RelocationAllocation};
use crate::{ObjectOwnership, SchedulerId};

use super::model::{
    AllocationInfrastructureConfig, AllocationMetrics, AllocationPlacement, HeapDomain,
    PageDescriptor, PageId, RegionId,
};
use super::{
    DirectAccessState, DirectPageAccess, DirectReferenceLease, RegionKey, RegionRecord,
    RegionState, ReservedMatureIdentity, ReservedMatureObject,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct LayoutKey {
    type_id: RuntimeTypeId,
    object_map: Arc<ObjectMap>,
}

impl Ord for LayoutKey {
    fn cmp(&self, other: &Self) -> Ordering {
        self.type_id
            .cmp(&other.type_id)
            .then_with(|| {
                self.object_map
                    .slot_count()
                    .cmp(&other.object_map.slot_count())
            })
            .then_with(|| {
                self.object_map
                    .reference_slot_count()
                    .cmp(&other.object_map.reference_slot_count())
            })
            .then_with(|| {
                self.object_map
                    .iter_reference_slots()
                    .cmp(other.object_map.iter_reference_slots())
            })
    }
}

impl PartialOrd for LayoutKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct PlacementIndex {
    pub(super) page: PageId,
    offset_bytes: usize,
}

impl From<AllocationPlacement> for PlacementIndex {
    fn from(placement: AllocationPlacement) -> Self {
        Self {
            page: placement.page,
            offset_bytes: placement.offset_bytes,
        }
    }
}

#[derive(Clone)]
struct Tlab {
    page: PageId,
    layout: LayoutKey,
    cursor: usize,
    limit: usize,
}

#[derive(Clone)]
struct MaturePageCursor {
    page: PageId,
    payload: PageWords,
    cursor: usize,
    limit: usize,
}

pub(crate) struct PayloadPlacement {
    placement: AllocationPlacement,
    payload: PageWords,
    object_map: Arc<ObjectMap>,
}

impl PayloadPlacement {
    pub(crate) fn into_page_storage(
        self,
    ) -> Result<(PageWords, usize, Arc<ObjectMap>), RuntimeFailure> {
        if !self.placement.offset_bytes.is_multiple_of(8) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        Ok((
            self.payload,
            self.placement.offset_bytes / 8,
            self.object_map,
        ))
    }
}

#[derive(Clone)]
struct DirectSpanRecord {
    first_reference: ManagedReference,
    last_reference: ManagedReference,
    published_last_reference: Arc<AtomicU64>,
    first_offset_bytes: usize,
    object_bytes: usize,
    kind: AllocationKind,
    valid: bool,
    materialized: bool,
}

#[derive(Clone)]
pub(crate) struct AllocationInfrastructure {
    pub(super) config: AllocationInfrastructureConfig,
    pub(super) pages: BTreeMap<PageId, PageDescriptor>,
    page_cursors: BTreeMap<PageId, usize>,
    active_mature_page: Option<(LayoutKey, SchedulerId, MaturePageCursor)>,
    mature_pages: BTreeMap<(LayoutKey, SchedulerId), MaturePageCursor>,
    pub(super) placements: ObjectTable<PlacementIndex>,
    direct_spans: BTreeMap<PageId, Vec<DirectSpanRecord>>,
    active_direct_span: Option<(PageId, DirectSpanRecord)>,
    direct_access_state: Arc<DirectAccessState>,
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
            direct_spans: BTreeMap::new(),
            active_direct_span: None,
            direct_access_state: Arc::new(DirectAccessState::default()),
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
        self.place_shared(
            reference,
            type_id,
            class,
            Arc::new(object_map.clone()),
            scheduler,
            None,
        )
        .map(|_| ())
    }

    pub(crate) fn place_shared(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        object_map: Arc<ObjectMap>,
        scheduler: SchedulerId,
        kind: Option<AllocationKind>,
    ) -> Result<PayloadPlacement, RuntimeFailure> {
        let layout = LayoutKey {
            type_id,
            object_map,
        };
        let size = object_size(layout.object_map.slot_count())?;
        let (placement, payload) = match class {
            AllocationClass::NurseryEligible => {
                let placement = self.place_in_tlab(&layout, size, scheduler)?;
                let payload = self
                    .pages
                    .get(&placement.page)
                    .map(|page| page.payload.clone())
                    .ok_or_else(RuntimeFailure::runtime_invariant)?;
                (placement, payload)
            }
            AllocationClass::Mature => self.place_in_mature_page(&layout, size, scheduler)?,
            AllocationClass::Large | AllocationClass::Pinned => {
                let domain = domain_for_class(class);
                let placement = self.place_on_new_page(&layout, size, domain, None)?;
                let payload = self
                    .pages
                    .get(&placement.page)
                    .map(|page| page.payload.clone())
                    .ok_or_else(RuntimeFailure::runtime_invariant)?;
                (placement, payload)
            }
        };
        self.record_bytes(size);
        if kind.is_some() {
            self.placements
                .insert_fresh(reference, placement.into())
                .map_err(|_| RuntimeFailure::runtime_invariant())?;
        } else {
            self.placements.insert(reference, placement.into());
        }
        if let Some(kind) = kind {
            self.record_direct_span(reference, placement, kind);
        }
        Ok(PayloadPlacement {
            placement,
            payload,
            object_map: layout.object_map,
        })
    }

    pub(crate) fn placement_requirement_shared(
        &self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        object_map: &Arc<ObjectMap>,
        scheduler: SchedulerId,
    ) -> Result<PlacementRequirement, RuntimeFailure> {
        let object_bytes = object_size(object_map.slot_count())?;
        let reuses_capacity = (class == AllocationClass::NurseryEligible
            && self.tlabs.get(&scheduler).is_some_and(|tlab| {
                layout_matches(&tlab.layout, type_id, object_map)
                    && tlab.cursor.saturating_add(object_bytes) <= tlab.limit
            }))
            || (class == AllocationClass::Mature
                && self
                    .indexed_mature_page(type_id, object_map, object_bytes, scheduler)
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

    pub(crate) fn mature_batch_requirement_shared(
        &self,
        type_id: RuntimeTypeId,
        object_map: &Arc<ObjectMap>,
        scheduler: SchedulerId,
        count: usize,
    ) -> Result<PlacementRequirement, RuntimeFailure> {
        let object_bytes = object_size(object_map.slot_count())?;
        let total_object_bytes = object_bytes
            .checked_mul(count)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let available_objects = self
            .indexed_mature_page(type_id, object_map, object_bytes, scheduler)
            .map_or(0, |(_, cursor, limit)| {
                limit.saturating_sub(cursor) / object_bytes
            });
        let remaining = count.saturating_sub(available_objects);
        let page_bytes = self.config.page_bytes.max(object_bytes);
        let objects_per_page = page_bytes / object_bytes;
        let additional_pages = remaining.div_ceil(objects_per_page);
        let additional_committed_bytes = additional_pages
            .checked_mul(page_bytes)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        Ok(PlacementRequirement {
            object_bytes: total_object_bytes,
            additional_committed_bytes,
        })
    }

    pub(crate) fn reserve_pointer_free_mature_objects(
        &mut self,
        identities: ReservedMatureIdentity,
        type_id: RuntimeTypeId,
        object_map: Arc<ObjectMap>,
        scheduler: SchedulerId,
    ) -> Result<Vec<ReservedMatureObject>, RuntimeFailure> {
        let layout = LayoutKey {
            type_id,
            object_map,
        };
        let size = object_size(layout.object_map.slot_count())?;
        let mut reservations = Vec::new();
        reservations
            .try_reserve_exact(identities.len().div_ceil(2))
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        let total_bytes = size
            .checked_mul(identities.len())
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let mut next = 0;
        while next < identities.len() {
            let first_identity = identities
                .at(next)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            let (first_placement, payload) = self.place_in_mature_page(&layout, size, scheduler)?;
            next += 1;

            let (append, full) = {
                let Some((active_layout, active_scheduler, active)) = &mut self.active_mature_page
                else {
                    self.record_direct_span(
                        first_identity.0,
                        first_placement,
                        AllocationKind::Object,
                    );
                    reservations.push(ReservedMatureObject::new(
                        first_identity,
                        payload,
                        first_placement.offset_bytes / 8,
                        1,
                        size / 8,
                    ));
                    continue;
                };
                if !layout_matches(active_layout, layout.type_id, &layout.object_map)
                    || *active_scheduler != scheduler
                {
                    self.record_direct_span(
                        first_identity.0,
                        first_placement,
                        AllocationKind::Object,
                    );
                    reservations.push(ReservedMatureObject::new(
                        first_identity,
                        payload,
                        first_placement.offset_bytes / 8,
                        1,
                        size / 8,
                    ));
                    continue;
                }
                let available = active.limit.saturating_sub(active.cursor) / size;
                let append = available.min(identities.len() - next);
                active.cursor = active
                    .cursor
                    .checked_add(append.saturating_mul(size))
                    .ok_or_else(RuntimeFailure::runtime_invariant)?;
                (append, active.cursor == active.limit)
            };
            let last_index = next + append;
            reservations.push(ReservedMatureObject::new(
                first_identity,
                payload,
                first_placement.offset_bytes / 8,
                append + 1,
                size / 8,
            ));
            self.record_direct_span_range(
                first_identity.0,
                identities
                    .at(last_index.saturating_sub(1))
                    .ok_or_else(RuntimeFailure::runtime_invariant)?
                    .0,
                first_placement,
                AllocationKind::Object,
            );
            self.metrics.mature_page_index_hits = self
                .metrics
                .mature_page_index_hits
                .saturating_add(u64::try_from(append).unwrap_or(u64::MAX));
            if full {
                let active = self
                    .active_mature_page
                    .take()
                    .ok_or_else(RuntimeFailure::runtime_invariant)?
                    .2;
                self.page_cursors.insert(active.page, active.cursor);
            }
            next = last_index;
        }
        self.record_bytes(total_bytes);
        Ok(reservations)
    }

    fn place_in_tlab(
        &mut self,
        layout: &LayoutKey,
        size: usize,
        scheduler: SchedulerId,
    ) -> Result<AllocationPlacement, RuntimeFailure> {
        let refill = self.tlabs.get(&scheduler).is_none_or(|tlab| {
            !layout_matches(&tlab.layout, layout.type_id, &layout.object_map)
                || tlab.cursor.saturating_add(size) > tlab.limit
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
    ) -> Result<(AllocationPlacement, PageWords), RuntimeFailure> {
        if let Some(placement) = self.place_on_active_mature_page(layout, size, scheduler) {
            return Ok(placement);
        }
        let key = (layout.clone(), scheduler);
        self.activate_mature_page(&key);
        if let Some(placement) = self.place_on_active_mature_page(layout, size, scheduler) {
            return Ok(placement);
        }
        self.active_mature_page = None;

        let page = self.create_page(
            layout,
            HeapDomain::LocalMature,
            self.config.page_bytes.max(size),
            Some(scheduler),
        )?;
        let (limit, payload) = self
            .pages
            .get(&page)
            .map(|descriptor| (descriptor.capacity_bytes, descriptor.payload.clone()))
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let cursor = size.min(limit);
        if cursor < limit {
            self.active_mature_page = Some((
                layout.clone(),
                scheduler,
                MaturePageCursor {
                    page,
                    payload: payload.clone(),
                    cursor,
                    limit,
                },
            ));
        } else {
            self.page_cursors.insert(page, cursor);
        }
        Ok((
            AllocationPlacement {
                page,
                offset_bytes: 0,
                size_bytes: size,
                domain: HeapDomain::LocalMature,
            },
            payload,
        ))
    }

    fn place_on_active_mature_page(
        &mut self,
        layout: &LayoutKey,
        size: usize,
        scheduler: SchedulerId,
    ) -> Option<(AllocationPlacement, PageWords)> {
        if let Some((active_layout, active_scheduler, active)) = &mut self.active_mature_page
            && layout_matches(active_layout, layout.type_id, &layout.object_map)
            && *active_scheduler == scheduler
            && let Some(cursor) = active.cursor.checked_add(size)
            && cursor <= active.limit
        {
            let page = active.page;
            let payload = active.payload.clone();
            let offset_bytes = active.cursor;
            let limit = active.limit;
            active.cursor = cursor;
            self.metrics.mature_page_index_hits =
                self.metrics.mature_page_index_hits.saturating_add(1);
            if cursor == limit {
                self.page_cursors.insert(page, cursor);
                self.active_mature_page = None;
            }
            return Some((
                AllocationPlacement {
                    page,
                    offset_bytes,
                    size_bytes: size,
                    domain: HeapDomain::LocalMature,
                },
                payload,
            ));
        }
        None
    }

    fn indexed_mature_page(
        &self,
        type_id: RuntimeTypeId,
        object_map: &Arc<ObjectMap>,
        size: usize,
        scheduler: SchedulerId,
    ) -> Option<(PageId, usize, usize)> {
        if let Some((active_layout, active_scheduler, active)) = &self.active_mature_page
            && layout_matches(active_layout, type_id, object_map)
            && *active_scheduler == scheduler
        {
            return active
                .cursor
                .checked_add(size)
                .is_some_and(|end| end <= active.limit)
                .then_some((active.page, active.cursor, active.limit));
        }
        let active = self
            .mature_pages
            .iter()
            .find(|((layout, owner), _)| {
                *owner == scheduler && layout_matches(layout, type_id, object_map)
            })
            .map(|(_, active)| active)?;
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
                && page.object_map == layout.object_map
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
                object_map: layout.object_map.clone(),
                capacity_bytes,
                payload: zeroed_page_words(capacity_bytes / 8),
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
        let placement = self
            .placements
            .get(&reference)
            .copied()
            .or_else(|| self.direct_span_placement(reference))?;
        let page = self.pages.get(&placement.page)?;
        Some(AllocationPlacement {
            page: placement.page,
            offset_bytes: placement.offset_bytes,
            size_bytes: object_size(page.object_map.slot_count()).ok()?,
            domain: page.domain,
        })
    }

    pub(crate) fn contains_placement_range(
        &self,
        first: ManagedReference,
        last: ManagedReference,
    ) -> bool {
        self.placements.contains_range(first, last)
            || (self.placement(first).is_some() && self.placement(last).is_some())
    }

    fn direct_span_placement(&self, reference: ManagedReference) -> Option<PlacementIndex> {
        let placement = |page: PageId, span: &DirectSpanRecord| {
            if !span.valid
                || !(span.first_reference.raw()..=span.last_reference.raw())
                    .contains(&reference.raw())
            {
                return None;
            }
            let relative =
                usize::try_from(reference.raw().checked_sub(span.first_reference.raw())?).ok()?;
            Some(PlacementIndex {
                page,
                offset_bytes: span
                    .first_offset_bytes
                    .checked_add(relative.checked_mul(span.object_bytes)?)?,
            })
        };
        self.active_direct_span
            .as_ref()
            .and_then(|(page, span)| placement(*page, span))
            .or_else(|| {
                self.direct_spans
                    .iter()
                    .find_map(|(page, spans)| spans.iter().find_map(|span| placement(*page, span)))
            })
    }

    pub(crate) fn region(&self, reference: ManagedReference) -> Option<RegionId> {
        self.placement(reference)
            .and_then(|placement| self.pages.get(&placement.page))
            .map(|page| page.region)
    }

    pub(crate) fn page(&self, page: PageId) -> Option<&PageDescriptor> {
        self.pages.get(&page)
    }

    pub(crate) fn direct_page_access(
        &self,
        reference: ManagedReference,
        kind: Option<AllocationKind>,
    ) -> Option<DirectPageAccess> {
        let placement = self.placement(reference)?;
        let matches = |span: &&DirectSpanRecord| {
            if !span.valid
                || kind.is_some_and(|kind| span.kind != kind)
                || !(span.first_reference.raw()..=span.last_reference.raw())
                    .contains(&reference.raw())
            {
                return false;
            }
            reference
                .raw()
                .checked_sub(span.first_reference.raw())
                .and_then(|value| usize::try_from(value).ok())
                .and_then(|relative| relative.checked_mul(span.object_bytes))
                .and_then(|relative| span.first_offset_bytes.checked_add(relative))
                .is_some_and(|offset| offset == placement.offset_bytes)
        };
        let active_span = self
            .active_direct_span
            .as_ref()
            .filter(|(page, _)| *page == placement.page)
            .map(|(_, span)| span);
        let span = active_span
            .filter(matches)
            .or_else(|| self.direct_spans.get(&placement.page)?.iter().find(matches))?;
        let page = self.pages.get(&placement.page)?;
        DirectPageAccess::new(
            span.first_reference,
            span.published_last_reference.clone(),
            span.first_offset_bytes / 8,
            page.object_map.slot_count() as usize,
            page.payload.clone(),
            page.object_map.clone(),
            self.direct_access_state.clone(),
        )
    }

    pub(crate) fn direct_reference_lease(
        &self,
        first_reference: ManagedReference,
        last_reference: ManagedReference,
        scheduler: SchedulerId,
    ) -> Option<DirectReferenceLease> {
        self.placement(first_reference)?;
        self.placement(last_reference)?;
        DirectReferenceLease::new(
            first_reference,
            last_reference,
            self.direct_access_state.clone(),
            scheduler,
        )
    }

    fn record_direct_span(
        &mut self,
        reference: ManagedReference,
        placement: AllocationPlacement,
        kind: AllocationKind,
    ) {
        let extends_active = self
            .active_direct_span
            .as_ref()
            .is_some_and(|(page, span)| {
                *page == placement.page
                    && span.valid
                    && span.kind == kind
                    && span.object_bytes == placement.size_bytes
                    && span
                        .last_reference
                        .raw()
                        .checked_add(1)
                        .is_some_and(|next| next == reference.raw())
                    && span
                        .first_offset_bytes
                        .checked_add(
                            usize::try_from(reference.raw() - span.first_reference.raw())
                                .unwrap_or(usize::MAX)
                                .saturating_mul(span.object_bytes),
                        )
                        .is_some_and(|offset| offset == placement.offset_bytes)
            });
        if extends_active {
            self.active_direct_span
                .as_mut()
                .expect("matching active direct span")
                .1
                .last_reference = reference;
        } else {
            if let Some((page, span)) = self.active_direct_span.take() {
                self.direct_spans.entry(page).or_default().push(span);
            }
            self.active_direct_span = Some((
                placement.page,
                DirectSpanRecord {
                    first_reference: reference,
                    last_reference: reference,
                    published_last_reference: Arc::new(AtomicU64::new(
                        reference.raw().saturating_sub(1),
                    )),
                    first_offset_bytes: placement.offset_bytes,
                    object_bytes: placement.size_bytes,
                    kind,
                    valid: true,
                    materialized: false,
                },
            ));
        }
    }

    fn record_direct_span_range(
        &mut self,
        first_reference: ManagedReference,
        last_reference: ManagedReference,
        first_placement: AllocationPlacement,
        kind: AllocationKind,
    ) {
        self.record_direct_span(first_reference, first_placement, kind);
        if first_reference == last_reference {
            return;
        }
        let Some((page, span)) = &mut self.active_direct_span else {
            return;
        };
        let relative = last_reference.raw() - span.first_reference.raw();
        let expected_offset = span.first_offset_bytes.checked_add(
            usize::try_from(relative)
                .unwrap_or(usize::MAX)
                .saturating_mul(span.object_bytes),
        );
        if *page == first_placement.page
            && span.valid
            && span.kind == kind
            && expected_offset.is_some_and(|offset| {
                offset
                    == first_placement.offset_bytes
                        + usize::try_from(last_reference.raw() - first_reference.raw())
                            .unwrap_or(usize::MAX)
                            .saturating_mul(first_placement.size_bytes)
            })
        {
            span.last_reference = last_reference;
        } else {
            span.valid = false;
        }
    }

    pub(crate) fn publish_direct_reference(&mut self, reference: ManagedReference) {
        let Some(placement) = self.placement(reference) else {
            return;
        };
        let contains_reference = |span: &&mut DirectSpanRecord| {
            span.valid
                && (span.first_reference.raw()..=span.last_reference.raw())
                    .contains(&reference.raw())
        };
        let active_span = self
            .active_direct_span
            .as_mut()
            .filter(|(page, _)| *page == placement.page)
            .map(|(_, span)| span);
        let Some(span) = active_span.filter(contains_reference).or_else(|| {
            self.direct_spans
                .get_mut(&placement.page)?
                .iter_mut()
                .find(contains_reference)
        }) else {
            return;
        };
        span.published_last_reference
            .store(reference.raw(), AtomicOrdering::Release);
        span.materialized = true;
    }

    pub(crate) fn publish_direct_range(
        &self,
        first_reference: ManagedReference,
        last_reference: ManagedReference,
    ) {
        let publish = |span: &DirectSpanRecord| {
            if span.valid
                && span.last_reference.raw() >= first_reference.raw()
                && span.first_reference.raw() <= last_reference.raw()
            {
                span.published_last_reference.store(
                    span.last_reference.raw().min(last_reference.raw()),
                    AtomicOrdering::Release,
                );
            }
        };
        for span in self.direct_spans.values().flatten() {
            publish(span);
        }
        if let Some((_, span)) = &self.active_direct_span {
            publish(span);
        }
    }

    pub(crate) fn materialize_direct_placements(
        &mut self,
        references: &[ManagedReference],
    ) -> Result<(), RuntimeFailure> {
        for reference in references {
            if self.placements.contains_key(reference) {
                continue;
            }
            let placement = self
                .direct_span_placement(*reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            self.placements
                .insert_reserved(*reference, placement)
                .map_err(|_| RuntimeFailure::runtime_invariant())?;
        }
        let Some(first) = references.first().copied() else {
            return Ok(());
        };
        let last = references[references.len() - 1];
        let mark = |span: &mut DirectSpanRecord| {
            if span.last_reference.raw() >= first.raw() && span.first_reference.raw() <= last.raw()
            {
                span.materialized = true;
            }
        };
        for span in self.direct_spans.values_mut().flatten() {
            mark(span);
        }
        if let Some((_, span)) = &mut self.active_direct_span {
            mark(span);
        }
        Ok(())
    }

    pub(crate) fn invalidate_direct_access(&mut self, reference: ManagedReference) {
        let Some(placement) = self.placement(reference) else {
            return;
        };
        self.direct_access_state.invalidate();
        if let Some(spans) = self.direct_spans.get_mut(&placement.page) {
            for span in spans {
                span.valid = false;
            }
        }
        if let Some((page, span)) = &mut self.active_direct_span
            && *page == placement.page
        {
            span.valid = false;
        }
    }

    pub(crate) fn invalidate_all_direct_accesses(&mut self) {
        self.direct_access_state.invalidate();
        self.direct_spans.clear();
        self.active_direct_span = None;
    }

    pub(crate) fn bind_object(
        &self,
        reference: ManagedReference,
        allocation: &mut Allocation,
    ) -> Result<(), RuntimeFailure> {
        let placement = self
            .placement(reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let page = self
            .pages
            .get(&placement.page)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        Self::bind_object_at(
            PayloadPlacement {
                placement,
                payload: page.payload.clone(),
                object_map: page.object_map.clone(),
            },
            allocation,
        )
    }

    pub(crate) fn bind_object_at(
        placed: PayloadPlacement,
        allocation: &mut Allocation,
    ) -> Result<(), RuntimeFailure> {
        if !placed.placement.offset_bytes.is_multiple_of(8) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        allocation
            .slots
            .bind_to_page(placed.payload, placed.placement.offset_bytes / 8)
            .map_err(|()| RuntimeFailure::runtime_invariant())?;
        if !Arc::ptr_eq(&allocation.object_map, &placed.object_map) {
            allocation.object_map = placed.object_map;
        }
        Ok(())
    }

    pub(crate) fn bind_all_payloads(
        &self,
        objects: &mut ObjectTable<RelocationAllocation>,
    ) -> Result<(), RuntimeFailure> {
        for (reference, object) in objects.iter_mut() {
            self.bind_object(reference, &mut object.allocation)?;
        }
        Ok(())
    }

    pub(crate) fn page_payload_word(&self, page: PageId, index: usize) -> Option<SlotValue> {
        self.pages
            .get(&page)?
            .payload
            .get(index)
            .map(|word| SlotValue::scalar(word.load(std::sync::atomic::Ordering::Relaxed)))
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
        self.invalidate_direct_access(reference);
        self.placements.remove(&reference);
    }

    pub(crate) fn cancel_reserved_tail(&mut self, first: ManagedReference, last: ManagedReference) {
        let truncate = |span: &mut DirectSpanRecord| {
            if span.last_reference.raw() < first.raw() || span.first_reference.raw() > last.raw() {
                return;
            }
            if span.first_reference.raw() >= first.raw() {
                span.valid = false;
            } else {
                span.last_reference = ManagedReference::new(first.raw() - 1);
            }
        };
        for span in self.direct_spans.values_mut().flatten() {
            truncate(span);
        }
        if let Some((_, span)) = &mut self.active_direct_span {
            truncate(span);
        }
        self.reclaim_empty_pages();
    }

    pub(crate) fn reclaim_empty_pages_after_sweep(&mut self) {
        self.reclaim_empty_pages();
    }

    pub(crate) fn live_bytes(&self) -> usize {
        self.placements.values().fold(0, |total, placement| {
            let size = self
                .pages
                .get(&placement.page)
                .and_then(|page| object_size(page.object_map.slot_count()).ok())
                .unwrap_or(0);
            total.saturating_add(size)
        })
    }

    pub(crate) fn committed_bytes(&self) -> usize {
        self.committed_bytes
    }

    pub(crate) fn bytes_in_domains(&self, domains: &[HeapDomain]) -> usize {
        self.placements.values().fold(0, |total, placement| {
            let Some(page) = self.pages.get(&placement.page) else {
                return total;
            };
            if domains.contains(&page.domain) {
                total.saturating_add(object_size(page.object_map.slot_count()).unwrap_or(0))
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
        let Some(previous) = self.placement(reference) else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if previous.domain == HeapDomain::Pinned {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.object_map.slot_count())?;
        let placement = self.place_on_new_page(&layout, size, HeapDomain::Pinned, None)?;
        self.invalidate_direct_access(reference);
        self.placements.insert(reference, placement.into());
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn move_to_shared(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        object_map: &ObjectMap,
    ) -> Result<(), RuntimeFailure> {
        let Some(previous) = self.placement(reference) else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if matches!(
            previous.domain,
            HeapDomain::Shared | HeapDomain::LargeObject | HeapDomain::Pinned
        ) {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.object_map.slot_count())?;
        let placement = self.place_on_new_page(&layout, size, HeapDomain::Shared, None)?;
        self.invalidate_direct_access(reference);
        self.placements.insert(reference, placement.into());
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn move_to_isolated(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        object_map: &ObjectMap,
    ) -> Result<(), RuntimeFailure> {
        let Some(previous) = self.placement(reference) else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if matches!(
            previous.domain,
            HeapDomain::Isolated | HeapDomain::LargeObject | HeapDomain::Pinned
        ) {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.object_map.slot_count())?;
        let placement = self.place_on_new_page(&layout, size, HeapDomain::Isolated, None)?;
        self.invalidate_direct_access(reference);
        self.placements.insert(reference, placement.into());
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
        let Some(previous) = self.placement(reference) else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if matches!(
            previous.domain,
            HeapDomain::LocalMature | HeapDomain::LargeObject | HeapDomain::Pinned
        ) {
            return Ok(());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.object_map.slot_count())?;
        let placement =
            self.place_on_new_page(&layout, size, HeapDomain::LocalMature, Some(scheduler))?;
        self.invalidate_direct_access(reference);
        self.placements.insert(reference, placement.into());
        self.reclaim_empty_pages();
        Ok(())
    }

    pub(crate) fn reconcile_after_minor(
        &mut self,
        previous_identities: &BTreeMap<CollectorObjectId, ManagedReference>,
        objects: &ObjectTable<RelocationAllocation>,
        scheduler: SchedulerId,
    ) -> Result<(), RuntimeFailure> {
        self.invalidate_all_direct_accesses();
        let mut previous = std::mem::take(&mut self.placements);
        let mut next = ObjectTable::new();
        for (reference, object) in objects.iter() {
            if let Some(placement) = previous.remove(&reference) {
                next.insert(reference, placement);
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
            let size = object_size(layout.object_map.slot_count())?;
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
            next.insert(reference, placement.into());
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
        self.invalidate_all_direct_accesses();
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
            let size = object_size(layout.object_map.slot_count())?;
            let placement = self.place_compacted_shared(&layout, size)?;
            self.placements.insert(*new, placement.into());
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
        let live_pages: BTreeSet<_> =
            self.placements
                .values()
                .map(|placement| placement.page)
                .chain(self.direct_spans.iter().filter_map(|(page, spans)| {
                    spans.iter().any(|span| span.valid).then_some(*page)
                }))
                .chain(
                    self.active_direct_span
                        .as_ref()
                        .filter(|(_, span)| span.valid)
                        .map(|(page, _)| *page),
                )
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
        self.direct_spans
            .retain(|page, _| self.pages.contains_key(page));
        if self
            .active_direct_span
            .as_ref()
            .is_some_and(|(page, _)| !self.pages.contains_key(page))
        {
            self.active_direct_span = None;
        }
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
        object_map: Arc::new(object_map.clone()),
    }
}

fn layout_matches(layout: &LayoutKey, type_id: RuntimeTypeId, object_map: &Arc<ObjectMap>) -> bool {
    layout.type_id == type_id
        && (Arc::ptr_eq(&layout.object_map, object_map) || layout.object_map == *object_map)
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

    fn assert_segmented_placement_storage(_: &ObjectTable<PlacementIndex>) {}

    #[test]
    fn placement_metadata_uses_token_segment_storage() {
        let allocation = AllocationInfrastructure::new(AllocationInfrastructureConfig::default());

        assert_segmented_placement_storage(&allocation.placements);
    }

    #[test]
    fn token_placement_side_metadata_keeps_only_page_and_offset() {
        assert_eq!(
            std::mem::size_of::<PlacementIndex>(),
            std::mem::size_of::<(PageId, usize)>()
        );
        assert!(std::mem::size_of::<PlacementIndex>() < std::mem::size_of::<AllocationPlacement>());
    }
}
