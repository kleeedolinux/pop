//! Mutable page inventory, TLAB cursor, and relocation placement updates.

use std::collections::BTreeMap;

use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectMap, ObjectSlot, RuntimeFailure, RuntimeTypeId,
};

use crate::relocation::{CollectorGeneration, CollectorObjectId, RelocationAllocation};

use super::model::{
    AllocationInfrastructureConfig, AllocationMetrics, AllocationPlacement, HeapDomain,
    PageDescriptor, PageId, RegionId,
};

#[derive(Clone, Debug, Eq, PartialEq)]
struct LayoutKey {
    type_id: RuntimeTypeId,
    slot_count: u32,
    reference_slots: Vec<ObjectSlot>,
}

struct Tlab {
    page: PageId,
    layout: LayoutKey,
    cursor: usize,
    limit: usize,
}

pub(crate) struct AllocationInfrastructure {
    config: AllocationInfrastructureConfig,
    pages: BTreeMap<PageId, PageDescriptor>,
    placements: BTreeMap<ManagedReference, AllocationPlacement>,
    tlab: Option<Tlab>,
    next_page: u64,
    metrics: AllocationMetrics,
}

impl AllocationInfrastructure {
    pub(crate) fn new(config: AllocationInfrastructureConfig) -> Self {
        Self {
            config,
            pages: BTreeMap::new(),
            placements: BTreeMap::new(),
            tlab: None,
            next_page: 1,
            metrics: AllocationMetrics::default(),
        }
    }

    pub(crate) fn place(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        object_map: &ObjectMap,
    ) -> Result<(), RuntimeFailure> {
        let layout = layout(type_id, object_map);
        let size = object_size(object_map.slot_count())?;
        let placement = if class == AllocationClass::NurseryEligible {
            self.place_in_tlab(&layout, size)?
        } else {
            self.place_on_new_page(&layout, size, domain_for_class(class))?
        };
        self.record_bytes(size);
        self.placements.insert(reference, placement);
        Ok(())
    }

    fn place_in_tlab(
        &mut self,
        layout: &LayoutKey,
        size: usize,
    ) -> Result<AllocationPlacement, RuntimeFailure> {
        let refill = self.tlab.as_ref().is_none_or(|tlab| {
            &tlab.layout != layout || tlab.cursor.saturating_add(size) > tlab.limit
        });
        if refill {
            let page = self.create_page(layout, HeapDomain::LocalEden, self.config.page_bytes)?;
            self.tlab = Some(Tlab {
                page,
                layout: layout.clone(),
                cursor: 0,
                limit: self.config.tlab_bytes.max(size),
            });
            self.metrics.tlab_refills = self.metrics.tlab_refills.saturating_add(1);
        }
        let tlab = self
            .tlab
            .as_mut()
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let offset = tlab.cursor;
        tlab.cursor = tlab
            .cursor
            .checked_add(size)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
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
    ) -> Result<AllocationPlacement, RuntimeFailure> {
        let page = self.create_page(layout, domain, self.config.page_bytes.max(size))?;
        Ok(AllocationPlacement {
            page,
            offset_bytes: 0,
            size_bytes: size,
            domain,
        })
    }

    fn create_page(
        &mut self,
        layout: &LayoutKey,
        domain: HeapDomain,
        capacity_bytes: usize,
    ) -> Result<PageId, RuntimeFailure> {
        let id = PageId(self.next_page);
        self.next_page = self
            .next_page
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let pages_per_region = self.config.region_bytes / self.config.page_bytes;
        let index = usize::try_from(id.0.saturating_sub(1)).unwrap_or(usize::MAX);
        let region = RegionId(
            u64::try_from(index / pages_per_region)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        );
        self.pages.insert(
            id,
            PageDescriptor {
                id,
                region,
                domain,
                type_id: layout.type_id,
                slot_count: layout.slot_count,
                reference_slots: layout.reference_slots.clone(),
                capacity_bytes,
            },
        );
        self.metrics.pages_created = self.metrics.pages_created.saturating_add(1);
        Ok(id)
    }

    pub(crate) fn placement(&self, reference: ManagedReference) -> Option<AllocationPlacement> {
        self.placements.get(&reference).copied()
    }

    pub(crate) fn page(&self, page: PageId) -> Option<&PageDescriptor> {
        self.pages.get(&page)
    }

    pub(crate) const fn metrics(&self) -> AllocationMetrics {
        self.metrics
    }

    pub(crate) fn remove(&mut self, reference: ManagedReference) {
        self.placements.remove(&reference);
    }

    pub(crate) fn move_to_pinned(
        &mut self,
        reference: ManagedReference,
        type_id: RuntimeTypeId,
        object_map: &ObjectMap,
    ) -> Result<(), RuntimeFailure> {
        if !self.placements.contains_key(&reference) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let layout = layout(type_id, object_map);
        let size = object_size(layout.slot_count)?;
        let placement = self.place_on_new_page(&layout, size, HeapDomain::Pinned)?;
        self.placements.insert(reference, placement);
        Ok(())
    }

    pub(crate) fn reconcile_after_minor(
        &mut self,
        previous_identities: &BTreeMap<CollectorObjectId, ManagedReference>,
        objects: &BTreeMap<ManagedReference, RelocationAllocation>,
    ) -> Result<(), RuntimeFailure> {
        let mut previous = std::mem::take(&mut self.placements);
        let mut next = BTreeMap::new();
        for (reference, object) in objects {
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
            let placement = self.place_on_new_page(&layout, size, domain)?;
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
                | HeapDomain::Shared
                | HeapDomain::LargeObject
                | HeapDomain::Pinned => {}
            }
        }
        self.placements = next;
        self.tlab = None;
        Ok(())
    }

    fn record_bytes(&mut self, size: usize) {
        self.metrics.allocated_bytes = self
            .metrics
            .allocated_bytes
            .saturating_add(u64::try_from(size).unwrap_or(u64::MAX));
    }
}

fn layout(type_id: RuntimeTypeId, object_map: &ObjectMap) -> LayoutKey {
    LayoutKey {
        type_id,
        slot_count: object_map.slot_count(),
        reference_slots: object_map.reference_slots().to_vec(),
    }
}

fn object_size(slot_count: u32) -> Result<usize, RuntimeFailure> {
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
