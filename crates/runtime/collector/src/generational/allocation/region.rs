//! Region lifecycle and immutable fragmentation telemetry.

use std::collections::BTreeMap;

use pop_runtime_interface::RuntimeFailure;

use crate::SchedulerId;

use super::state::AllocationInfrastructure;
use super::{HeapDomain, RegionId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RegionState {
    Free,
    LocalEden,
    LocalSurvivor,
    LocalMature,
    Isolated,
    SharedAllocating,
    SharedMarking,
    SharedSweeping,
    EvacuationCandidate,
    Evacuating,
    Pinned,
    LargeObject,
    Quarantined,
}

impl RegionState {
    pub(super) const fn accepts_allocation(self) -> bool {
        matches!(
            self,
            Self::LocalEden
                | Self::LocalSurvivor
                | Self::LocalMature
                | Self::SharedAllocating
                | Self::SharedMarking
                | Self::SharedSweeping
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct RegionKey {
    pub(super) domain: HeapDomain,
    pub(super) scheduler: Option<SchedulerId>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RegionRecord {
    pub(super) id: RegionId,
    pub(super) state: RegionState,
    pub(super) key: RegionKey,
    pub(super) capacity_bytes: usize,
    pub(super) committed_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RegionTelemetry {
    pub(super) id: RegionId,
    pub(super) state: RegionState,
    pub(super) domain: HeapDomain,
    pub(super) scheduler: Option<SchedulerId>,
    pub(super) capacity_bytes: usize,
    pub(super) committed_bytes: usize,
    pub(super) live_bytes: usize,
    pub(super) fragmented_bytes: usize,
    pub(super) free_bytes: usize,
    pub(super) pinned_bytes: usize,
    pub(super) reference_slot_count: usize,
    pub(super) page_count: usize,
    pub(super) object_count: usize,
}

macro_rules! region_telemetry_accessors {
    ($($name:ident: $type:ty),* $(,)?) => {
        $(
            #[must_use]
            pub const fn $name(self) -> $type {
                self.$name
            }
        )*
    };
}

impl RegionTelemetry {
    region_telemetry_accessors! {
        id: RegionId,
        state: RegionState,
        domain: HeapDomain,
        scheduler: Option<SchedulerId>,
        capacity_bytes: usize,
        committed_bytes: usize,
        live_bytes: usize,
        fragmented_bytes: usize,
        free_bytes: usize,
        pinned_bytes: usize,
        reference_slot_count: usize,
        page_count: usize,
        object_count: usize,
    }

    #[must_use]
    pub const fn pin_density_percent(self) -> usize {
        if self.live_bytes == 0 {
            0
        } else {
            self.pinned_bytes
                .saturating_mul(100)
                .saturating_div(self.live_bytes)
        }
    }

    #[must_use]
    pub const fn fragmentation_percent(self) -> usize {
        if self.committed_bytes == 0 {
            0
        } else {
            self.fragmented_bytes
                .saturating_mul(100)
                .saturating_div(self.committed_bytes)
        }
    }
}

pub(crate) const fn initial_region_state(domain: HeapDomain, shared: RegionState) -> RegionState {
    match domain {
        HeapDomain::LocalEden => RegionState::LocalEden,
        HeapDomain::LocalSurvivor => RegionState::LocalSurvivor,
        HeapDomain::LocalMature => RegionState::LocalMature,
        HeapDomain::Isolated => RegionState::Isolated,
        HeapDomain::Shared => shared,
        HeapDomain::LargeObject => RegionState::LargeObject,
        HeapDomain::Pinned => RegionState::Pinned,
    }
}

impl AllocationInfrastructure {
    pub(super) fn acquire_region(
        &mut self,
        domain: HeapDomain,
        scheduler: Option<SchedulerId>,
        page_capacity_bytes: usize,
    ) -> Result<RegionId, RuntimeFailure> {
        let key = RegionKey { domain, scheduler };
        if shares_physical_region(domain) && page_capacity_bytes <= self.config.region_bytes {
            let candidate = self.active_regions.get(&key).and_then(|regions| {
                regions.iter().copied().find(|region| {
                    self.regions.get(region).is_some_and(|record| {
                        record.state.accepts_allocation()
                            && record
                                .committed_bytes
                                .checked_add(page_capacity_bytes)
                                .is_some_and(|committed| committed <= record.capacity_bytes)
                    })
                })
            });
            if let Some(region) = candidate {
                return Ok(region);
            }
        }

        let id = RegionId(self.next_region);
        self.next_region = self
            .next_region
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let state = initial_region_state(domain, self.shared_region_state);
        let capacity_bytes = self.config.region_bytes.max(page_capacity_bytes);
        self.regions.insert(
            id,
            RegionRecord {
                id,
                state,
                key,
                capacity_bytes,
                committed_bytes: 0,
            },
        );
        if shares_physical_region(domain)
            && page_capacity_bytes < capacity_bytes
            && state.accepts_allocation()
        {
            self.active_regions.entry(key).or_default().insert(id);
        }
        Ok(id)
    }

    pub(super) fn remove_active_region(&mut self, key: RegionKey, region: RegionId) {
        let remove_key = self.active_regions.get_mut(&key).is_some_and(|regions| {
            regions.remove(&region);
            regions.is_empty()
        });
        if remove_key {
            self.active_regions.remove(&key);
        }
    }

    pub(crate) fn region_telemetry(&self) -> Vec<RegionTelemetry> {
        let mut telemetry = self
            .regions
            .values()
            .map(|record| {
                (
                    record.id,
                    RegionTelemetry {
                        id: record.id,
                        state: record.state,
                        domain: record.key.domain,
                        scheduler: record.key.scheduler,
                        capacity_bytes: record.capacity_bytes,
                        committed_bytes: record.committed_bytes,
                        live_bytes: 0,
                        fragmented_bytes: 0,
                        free_bytes: 0,
                        pinned_bytes: 0,
                        reference_slot_count: 0,
                        page_count: 0,
                        object_count: 0,
                    },
                )
            })
            .collect::<BTreeMap<_, _>>();
        for page in self.pages.values() {
            if let Some(region) = telemetry.get_mut(&page.region) {
                region.page_count = region.page_count.saturating_add(1);
            }
        }
        for placement in self.placements.values() {
            let Some(page) = self.pages.get(&placement.page) else {
                continue;
            };
            let Some(region) = telemetry.get_mut(&page.region) else {
                continue;
            };
            region.live_bytes = region.live_bytes.saturating_add(placement.size_bytes);
            region.object_count = region.object_count.saturating_add(1);
            region.reference_slot_count = region
                .reference_slot_count
                .saturating_add(page.reference_slots.len());
            if placement.domain == HeapDomain::Pinned {
                region.pinned_bytes = region.pinned_bytes.saturating_add(placement.size_bytes);
            }
        }
        for region in telemetry.values_mut() {
            region.fragmented_bytes = region.committed_bytes.saturating_sub(region.live_bytes);
            region.free_bytes = region.capacity_bytes.saturating_sub(region.live_bytes);
        }
        telemetry.into_values().collect()
    }

    pub(crate) fn transition_shared_regions(&mut self, state: RegionState) {
        self.shared_region_state = state;
        for record in self.regions.values_mut().filter(|record| {
            record.key.domain == HeapDomain::Shared
                && !matches!(
                    record.state,
                    RegionState::EvacuationCandidate
                        | RegionState::Evacuating
                        | RegionState::Quarantined
                )
        }) {
            record.state = state;
        }
        self.rebuild_active_regions();
    }

    pub(crate) fn mark_evacuation_candidates(&mut self, regions: &[RegionId]) {
        for region in regions {
            if let Some(record) = self.regions.get_mut(region)
                && record.key.domain == HeapDomain::Shared
                && record.state == RegionState::SharedAllocating
            {
                record.state = RegionState::EvacuationCandidate;
            }
        }
        self.rebuild_active_regions();
    }

    pub(crate) fn cancel_evacuation_candidates(&mut self) -> usize {
        let mut cancelled = 0usize;
        for record in self
            .regions
            .values_mut()
            .filter(|record| record.state == RegionState::EvacuationCandidate)
        {
            record.state = self.shared_region_state;
            cancelled = cancelled.saturating_add(1);
        }
        self.rebuild_active_regions();
        cancelled
    }

    pub(super) fn rebuild_active_regions(&mut self) {
        self.active_regions.clear();
        for record in self.regions.values().filter(|record| {
            shares_physical_region(record.key.domain)
                && record.state.accepts_allocation()
                && record.committed_bytes < record.capacity_bytes
        }) {
            self.active_regions
                .entry(record.key)
                .or_default()
                .insert(record.id);
        }
    }
}

const fn shares_physical_region(domain: HeapDomain) -> bool {
    matches!(
        domain,
        HeapDomain::LocalEden
            | HeapDomain::LocalSurvivor
            | HeapDomain::LocalMature
            | HeapDomain::Shared
    )
}
