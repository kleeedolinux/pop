//! Deterministic reserve-bounded mature-region selection.

use pop_runtime_interface::RuntimeFailure;

use super::allocation::{RegionId, RegionState, RegionTelemetry};
use super::heap::GenerationalRuntime;

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
}
