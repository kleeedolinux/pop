//! Deterministic allocation-site survival sampling and adaptive pretenuring.

use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::RuntimeAllocationSiteId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AdaptivePretenuringConfig {
    minimum_samples: usize,
    survival_percent: u8,
    consecutive_high_survival_cycles: u8,
}

impl AdaptivePretenuringConfig {
    #[must_use]
    pub const fn new(
        minimum_samples: usize,
        survival_percent: u8,
        consecutive_high_survival_cycles: u8,
    ) -> Option<Self> {
        if minimum_samples == 0 || survival_percent > 100 || consecutive_high_survival_cycles == 0 {
            return None;
        }
        Some(Self {
            minimum_samples,
            survival_percent,
            consecutive_high_survival_cycles,
        })
    }

    #[must_use]
    pub const fn minimum_samples(self) -> usize {
        self.minimum_samples
    }

    #[must_use]
    pub const fn survival_percent(self) -> u8 {
        self.survival_percent
    }

    #[must_use]
    pub const fn consecutive_high_survival_cycles(self) -> u8 {
        self.consecutive_high_survival_cycles
    }
}

impl Default for AdaptivePretenuringConfig {
    fn default() -> Self {
        Self::new(4, 75, 2).expect("default adaptive-pretenuring bounds are valid")
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SiteSurvival {
    consecutive_high_survival_cycles: u8,
    pretenured: bool,
}

pub(crate) struct AdaptivePretenuringState {
    config: AdaptivePretenuringConfig,
    sites: BTreeMap<RuntimeAllocationSiteId, SiteSurvival>,
}

impl AdaptivePretenuringState {
    pub(crate) fn new(config: AdaptivePretenuringConfig) -> Self {
        Self {
            config,
            sites: BTreeMap::new(),
        }
    }

    pub(crate) fn should_pretenure(&self, site: RuntimeAllocationSiteId) -> bool {
        self.sites
            .get(&site)
            .is_some_and(|survival| survival.pretenured)
    }

    pub(crate) fn observe(
        &mut self,
        site: RuntimeAllocationSiteId,
        sampled: usize,
        survived: usize,
    ) {
        if sampled < self.config.minimum_samples || survived > sampled {
            return;
        }
        let high_survival = survived.saturating_mul(100)
            >= sampled.saturating_mul(usize::from(self.config.survival_percent));
        let survival = self.sites.entry(site).or_default();
        if high_survival {
            survival.consecutive_high_survival_cycles =
                survival.consecutive_high_survival_cycles.saturating_add(1);
            if survival.consecutive_high_survival_cycles
                >= self.config.consecutive_high_survival_cycles
            {
                survival.pretenured = true;
            }
        } else {
            survival.consecutive_high_survival_cycles = 0;
            survival.pretenured = false;
        }
    }

    pub(crate) fn pretenured_sites(&self) -> BTreeSet<RuntimeAllocationSiteId> {
        self.sites
            .iter()
            .filter_map(|(site, survival)| survival.pretenured.then_some(*site))
            .collect()
    }
}
