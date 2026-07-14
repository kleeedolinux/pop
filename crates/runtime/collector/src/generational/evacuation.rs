//! Deterministic reserve-bounded mature-region selection and evacuation.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use pop_runtime_interface::{ManagedReference, RootPublication, RuntimeFailure};

use super::allocation::{RegionId, RegionState, RegionTelemetry};
use super::heap::GenerationalRuntime;
use super::workers::EvacuationRewriteTask;
use crate::heap::{BootstrapRuntime, SlotValue};
use crate::relocation::CollectorGeneration;

type RefinedCards = BTreeMap<ManagedReference, Vec<ManagedReference>>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvacuationSelectionConfig {
    maximum_regions: usize,
    minimum_fragmentation_percent: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvacuationSelectionConfigError {
    ZeroRegionLimit,
    InvalidFragmentationPercent,
}

impl EvacuationSelectionConfig {
    /// Defines the bounded selective-evacuation candidate policy.
    ///
    /// # Errors
    ///
    /// Rejects a zero region bound or a fragmentation threshold outside
    /// `1..=100`.
    pub const fn new(
        maximum_regions: usize,
        minimum_fragmentation_percent: usize,
    ) -> Result<Self, EvacuationSelectionConfigError> {
        if maximum_regions == 0 {
            return Err(EvacuationSelectionConfigError::ZeroRegionLimit);
        }
        if minimum_fragmentation_percent == 0 || minimum_fragmentation_percent > 100 {
            return Err(EvacuationSelectionConfigError::InvalidFragmentationPercent);
        }
        Ok(Self {
            maximum_regions,
            minimum_fragmentation_percent,
        })
    }

    #[must_use]
    pub const fn maximum_regions(self) -> usize {
        self.maximum_regions
    }

    #[must_use]
    pub const fn minimum_fragmentation_percent(self) -> usize {
        self.minimum_fragmentation_percent
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvacuationCandidate {
    region: RegionId,
    live_bytes: usize,
    reclaimable_bytes: usize,
    copy_cost_bytes: usize,
    reference_update_cost_bytes: usize,
    estimated_benefit_bytes: usize,
    object_count: usize,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EvacuationStatistics {
    regions_evacuated: usize,
    objects_evacuated: usize,
    bytes_copied: usize,
    object_fields_updated: usize,
    stack_roots_updated: usize,
    strong_handles_updated: usize,
    pin_handles_updated: usize,
    peak_committed_bytes: usize,
    committed_bytes_reclaimed: usize,
    worker_objects_processed: usize,
}

impl EvacuationStatistics {
    #[must_use]
    pub const fn regions_evacuated(self) -> usize {
        self.regions_evacuated
    }

    #[must_use]
    pub const fn objects_evacuated(self) -> usize {
        self.objects_evacuated
    }

    #[must_use]
    pub const fn bytes_copied(self) -> usize {
        self.bytes_copied
    }

    #[must_use]
    pub const fn object_fields_updated(self) -> usize {
        self.object_fields_updated
    }

    #[must_use]
    pub const fn stack_roots_updated(self) -> usize {
        self.stack_roots_updated
    }

    #[must_use]
    pub const fn strong_handles_updated(self) -> usize {
        self.strong_handles_updated
    }

    #[must_use]
    pub const fn pin_handles_updated(self) -> usize {
        self.pin_handles_updated
    }

    #[must_use]
    pub const fn peak_committed_bytes(self) -> usize {
        self.peak_committed_bytes
    }

    #[must_use]
    pub const fn committed_bytes_reclaimed(self) -> usize {
        self.committed_bytes_reclaimed
    }

    #[must_use]
    pub const fn worker_objects_processed(self) -> usize {
        self.worker_objects_processed
    }
}

macro_rules! evacuation_candidate_accessors {
    ($($name:ident: $type:ty),* $(,)?) => {
        $(
            #[must_use]
            pub const fn $name(self) -> $type {
                self.$name
            }
        )*
    };
}

impl EvacuationCandidate {
    evacuation_candidate_accessors! {
        region: RegionId,
        live_bytes: usize,
        reclaimable_bytes: usize,
        copy_cost_bytes: usize,
        reference_update_cost_bytes: usize,
        estimated_benefit_bytes: usize,
        object_count: usize,
    }

    fn from_region(region: RegionTelemetry) -> Option<Self> {
        let copy_cost_bytes = region.live_bytes();
        let reference_update_cost_bytes = region.reference_slot_count().saturating_mul(8);
        let relocation_cost = copy_cost_bytes.saturating_add(reference_update_cost_bytes);
        let estimated_benefit_bytes = region.fragmented_bytes().checked_sub(relocation_cost)?;
        if estimated_benefit_bytes == 0 {
            return None;
        }
        Some(Self {
            region: region.id(),
            live_bytes: region.live_bytes(),
            reclaimable_bytes: region.fragmented_bytes(),
            copy_cost_bytes,
            reference_update_cost_bytes,
            estimated_benefit_bytes,
            object_count: region.object_count(),
        })
    }
}

impl GenerationalRuntime {
    #[must_use]
    pub fn region_telemetry(&self) -> Vec<RegionTelemetry> {
        self.allocation.region_telemetry()
    }

    /// Selects a deterministic bounded set of profitable shared regions.
    ///
    /// Already selected regions count against both the region limit and the
    /// protected evacuation reserve. Pinned, large, active-phase, and
    /// non-profitable regions are never admitted.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure if selection is attempted during an
    /// active major mark/sweep cycle.
    pub fn select_evacuation_candidates(
        &mut self,
        config: EvacuationSelectionConfig,
    ) -> Result<Vec<EvacuationCandidate>, RuntimeFailure> {
        if self.major_cycle_active() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let regions = self.allocation.region_telemetry();
        let selected_count = regions
            .iter()
            .filter(|region| {
                matches!(
                    region.state(),
                    RegionState::EvacuationCandidate | RegionState::Evacuating
                )
            })
            .count();
        let remaining_regions = config.maximum_regions().saturating_sub(selected_count);
        if remaining_regions == 0 {
            return Ok(Vec::new());
        }
        let reserved_live_bytes = regions
            .iter()
            .filter(|region| {
                matches!(
                    region.state(),
                    RegionState::EvacuationCandidate | RegionState::Evacuating
                )
            })
            .fold(0usize, |total, region| {
                total.saturating_add(region.live_bytes())
            });
        let reserve = self
            .memory_telemetry()
            .evacuation_reserve_bytes()
            .saturating_sub(reserved_live_bytes);

        let mut eligible = regions
            .into_iter()
            .filter(|region| {
                region.state() == RegionState::SharedAllocating
                    && region.pinned_bytes() == 0
                    && region.fragmentation_percent() >= config.minimum_fragmentation_percent()
            })
            .filter_map(EvacuationCandidate::from_region)
            .collect::<Vec<_>>();
        eligible.sort_by(|left, right| {
            right
                .estimated_benefit_bytes
                .cmp(&left.estimated_benefit_bytes)
                .then_with(|| right.reclaimable_bytes.cmp(&left.reclaimable_bytes))
                .then_with(|| left.live_bytes.cmp(&right.live_bytes))
                .then_with(|| left.region.cmp(&right.region))
        });

        let mut admitted_bytes = 0usize;
        let mut selected = Vec::new();
        for candidate in eligible {
            if selected.len() == remaining_regions {
                break;
            }
            let Some(after) = admitted_bytes.checked_add(candidate.live_bytes) else {
                continue;
            };
            if after > reserve {
                continue;
            }
            admitted_bytes = after;
            selected.push(candidate);
        }
        self.allocation.mark_evacuation_candidates(
            &selected
                .iter()
                .copied()
                .map(EvacuationCandidate::region)
                .collect::<Vec<_>>(),
        );
        Ok(selected)
    }

    pub fn cancel_evacuation_candidates(&mut self) -> usize {
        self.allocation.cancel_evacuation_candidates()
    }

    /// Copies every selected shared region and atomically rewrites precise
    /// object fields, stable handles, and the stopped mutator's root slots.
    ///
    /// The private forwarding map exists only while the mutator is stopped.
    /// Old physical tokens are invalid when this method returns successfully.
    ///
    /// # Errors
    ///
    /// Rejects an active major cycle, stale or malformed precise references,
    /// an invalid selected-region state, or evacuation-reserve exhaustion
    /// without publishing a partial relocation.
    pub fn evacuate_selected_regions(
        &mut self,
        publication: &mut RootPublication,
    ) -> Result<EvacuationStatistics, RuntimeFailure> {
        if self.major_cycle_active() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let selected_telemetry = self
            .allocation
            .region_telemetry()
            .into_iter()
            .filter(|region| region.state() == RegionState::EvacuationCandidate)
            .collect::<Vec<_>>();
        let selected_regions = selected_telemetry
            .iter()
            .map(|region| region.id())
            .collect::<BTreeSet<_>>();
        if selected_regions.is_empty() {
            return Ok(EvacuationStatistics::default());
        }

        self.validate_evacuation_references(publication)?;
        let expected_objects = selected_telemetry.iter().fold(0usize, |total, region| {
            total.saturating_add(region.object_count())
        });
        let selected_objects =
            self.selected_evacuation_objects(&selected_regions, expected_objects)?;
        let (relocations, next_reference, bytes_copied) =
            self.plan_relocation_tokens(&selected_objects)?;
        let (next_objects, object_fields_updated, worker_objects_processed) =
            self.relocate_objects(&relocations)?;

        let (next_roots, strong_handles_updated) =
            relocate_handles(&self.nursery.roots, &relocations);
        let (next_pins, pin_handles_updated) = relocate_handles(&self.nursery.pins, &relocations);
        let (stack_updates, stack_roots_updated) = relocated_stack_roots(publication, &relocations);
        let (next_dirty_cards, next_refined_cards) = self.relocate_card_metadata(&relocations);

        let committed_before = self.allocation.committed_bytes();
        let mut next_allocation = self.allocation.clone();
        let peak_committed =
            next_allocation.reconcile_after_evacuation(&relocations, &next_objects)?;
        if !self.memory.admits_evacuation(peak_committed) {
            self.memory.record_out_of_memory();
            return Err(BootstrapRuntime::out_of_memory(0, bytes_copied));
        }

        let committed_after = next_allocation.committed_bytes();
        self.nursery.objects = next_objects;
        self.nursery.roots = next_roots;
        self.nursery.pins = next_pins;
        self.nursery.dirty_cards = next_dirty_cards;
        self.nursery.refined_cards = next_refined_cards;
        self.nursery.next_reference = next_reference;
        self.allocation = next_allocation;
        for ((_, value), update) in publication.root_values_mut().zip(stack_updates) {
            *value = update;
        }
        self.memory.observe_committed(peak_committed);
        self.update_memory_target();

        Ok(EvacuationStatistics {
            regions_evacuated: selected_regions.len(),
            objects_evacuated: relocations.len(),
            bytes_copied,
            object_fields_updated,
            stack_roots_updated,
            strong_handles_updated,
            pin_handles_updated,
            peak_committed_bytes: peak_committed,
            committed_bytes_reclaimed: committed_before.saturating_sub(committed_after),
            worker_objects_processed,
        })
    }

    fn selected_evacuation_objects(
        &self,
        selected_regions: &BTreeSet<RegionId>,
        expected_objects: usize,
    ) -> Result<Vec<ManagedReference>, RuntimeFailure> {
        let selected = self
            .nursery
            .objects
            .iter()
            .filter_map(|(reference, object)| {
                self.allocation
                    .region(*reference)
                    .filter(|region| selected_regions.contains(region))
                    .map(|_| (*reference, object))
            })
            .collect::<Vec<_>>();
        if selected.is_empty()
            || selected.len() != expected_objects
            || selected.iter().any(|(reference, object)| {
                object.generation != CollectorGeneration::Mature
                    || object.ownership != crate::ObjectOwnership::Shared
                    || self.nursery.pins.values().any(|target| target == reference)
            })
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        Ok(selected
            .into_iter()
            .map(|(reference, _)| reference)
            .collect())
    }

    fn plan_relocation_tokens(
        &self,
        selected_objects: &[ManagedReference],
    ) -> Result<(BTreeMap<ManagedReference, ManagedReference>, u64, usize), RuntimeFailure> {
        let mut next_reference = self.nursery.next_reference;
        let mut relocations = BTreeMap::new();
        let mut bytes_copied = 0usize;
        for old in selected_objects {
            let new = ManagedReference::new(next_reference);
            next_reference = next_reference
                .checked_add(1)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            relocations.insert(*old, new);
            bytes_copied = bytes_copied.saturating_add(
                self.allocation
                    .placement(*old)
                    .ok_or_else(RuntimeFailure::runtime_invariant)?
                    .size_bytes(),
            );
        }
        Ok((relocations, next_reference, bytes_copied))
    }

    fn relocate_objects(
        &mut self,
        relocations: &BTreeMap<ManagedReference, ManagedReference>,
    ) -> Result<
        (
            crate::relocation::table::ObjectTable<crate::relocation::RelocationAllocation>,
            usize,
            usize,
        ),
        RuntimeFailure,
    > {
        if self.workers.is_none() {
            let (objects, fields_updated) = self.relocate_objects_on_collector(relocations)?;
            return Ok((objects, fields_updated, 0));
        }

        let tasks = relocations
            .iter()
            .map(|(source, destination)| {
                Ok(EvacuationRewriteTask {
                    destination: *destination,
                    allocation: self
                        .nursery
                        .objects
                        .get(source)
                        .cloned()
                        .ok_or_else(RuntimeFailure::runtime_invariant)?,
                })
            })
            .collect::<Result<Vec<_>, RuntimeFailure>>()?;
        let relocation_snapshot = Arc::new(relocations.clone());
        let results = self
            .workers
            .as_mut()
            .ok_or_else(RuntimeFailure::runtime_invariant)?
            .evacuate(tasks, &relocation_snapshot)?;
        let mut fields_updated = 0usize;
        let mut next_objects = crate::relocation::table::ObjectTable::new();
        for (reference, object) in self.nursery.objects.iter() {
            if relocations.contains_key(reference) {
                continue;
            }
            let mut next_object = object.clone();
            fields_updated = fields_updated
                .saturating_add(rewrite_object_references(&mut next_object, relocations));
            if next_objects.insert(*reference, next_object).is_some() {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        let worker_objects_processed = results.len();
        for result in results {
            fields_updated = fields_updated.saturating_add(result.fields_updated);
            if next_objects
                .insert(result.destination, result.allocation)
                .is_some()
            {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        Ok((next_objects, fields_updated, worker_objects_processed))
    }

    fn relocate_objects_on_collector(
        &self,
        relocations: &BTreeMap<ManagedReference, ManagedReference>,
    ) -> Result<
        (
            crate::relocation::table::ObjectTable<crate::relocation::RelocationAllocation>,
            usize,
        ),
        RuntimeFailure,
    > {
        let mut fields_updated = 0usize;
        let mut next_objects = crate::relocation::table::ObjectTable::new();
        for (reference, object) in self.nursery.objects.iter() {
            let next_key = relocated(*reference, relocations);
            let mut next_object = object.clone();
            fields_updated = fields_updated
                .saturating_add(rewrite_object_references(&mut next_object, relocations));
            if next_objects.insert(next_key, next_object).is_some() {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        Ok((next_objects, fields_updated))
    }

    fn relocate_card_metadata(
        &self,
        relocations: &BTreeMap<ManagedReference, ManagedReference>,
    ) -> (BTreeSet<ManagedReference>, Option<RefinedCards>) {
        let dirty = self
            .nursery
            .dirty_cards
            .iter()
            .map(|reference| relocated(*reference, relocations))
            .collect();
        let refined = self.nursery.refined_cards.as_ref().map(|cards| {
            cards
                .iter()
                .map(|(owner, children)| {
                    (
                        relocated(*owner, relocations),
                        children
                            .iter()
                            .map(|child| relocated(*child, relocations))
                            .collect(),
                    )
                })
                .collect()
        });
        (dirty, refined)
    }

    fn validate_evacuation_references(
        &self,
        publication: &RootPublication,
    ) -> Result<(), RuntimeFailure> {
        for reference in publication
            .managed_references()
            .chain(self.nursery.roots.values().copied())
            .chain(self.nursery.pins.values().copied())
        {
            if !self.nursery.contains(reference) {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        for object in self.nursery.objects.values() {
            for slot in object.allocation.object_map.reference_slots() {
                match object.allocation.slots.get(slot.raw() as usize) {
                    Some(SlotValue::Reference(Some(reference)))
                        if self.nursery.contains(*reference) => {}
                    Some(SlotValue::Reference(None)) => {}
                    Some(SlotValue::Reference(Some(_)) | SlotValue::Scalar(_)) | None => {
                        return Err(RuntimeFailure::runtime_invariant());
                    }
                }
            }
        }
        Ok(())
    }
}

fn relocated_stack_roots(
    publication: &RootPublication,
    relocations: &BTreeMap<ManagedReference, ManagedReference>,
) -> (Vec<Option<ManagedReference>>, usize) {
    let mut updated = 0usize;
    let roots = publication
        .root_values()
        .map(|(_, value)| {
            value.map(|reference| {
                let next = relocated(reference, relocations);
                if next != reference {
                    updated = updated.saturating_add(1);
                }
                next
            })
        })
        .collect();
    (roots, updated)
}

fn rewrite_object_references(
    object: &mut crate::relocation::RelocationAllocation,
    relocations: &BTreeMap<ManagedReference, ManagedReference>,
) -> usize {
    let mut updated = 0usize;
    for slot in &mut object.allocation.slots {
        if let SlotValue::Reference(Some(reference)) = slot
            && let Some(destination) = relocations.get(reference)
        {
            *reference = *destination;
            updated = updated.saturating_add(1);
        }
    }
    updated
}

fn relocated(
    reference: ManagedReference,
    relocations: &BTreeMap<ManagedReference, ManagedReference>,
) -> ManagedReference {
    relocations.get(&reference).copied().unwrap_or(reference)
}

fn relocate_handles<Handle: Copy + Ord>(
    handles: &BTreeMap<Handle, ManagedReference>,
    relocations: &BTreeMap<ManagedReference, ManagedReference>,
) -> (BTreeMap<Handle, ManagedReference>, usize) {
    let mut updated = 0usize;
    let next = handles
        .iter()
        .map(|(handle, reference)| {
            let next = relocated(*reference, relocations);
            if next != *reference {
                updated = updated.saturating_add(1);
            }
            (*handle, next)
        })
        .collect();
    (next, updated)
}
