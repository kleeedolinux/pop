//! Generational collector state and public control surface.

use std::collections::BTreeSet;

use pop_runtime_interface::{
    CollectionStatistics, ManagedReference, RootPublication, RuntimeFailure,
};

use crate::relocation::RelocationRuntime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MajorCollectorConfig {
    work_budget: usize,
}

impl MajorCollectorConfig {
    #[must_use]
    pub const fn new(work_budget: usize) -> Self {
        Self {
            work_budget: if work_budget == 0 { 1 } else { work_budget },
        }
    }

    #[must_use]
    pub const fn work_budget(self) -> usize {
        self.work_budget
    }
}

impl Default for MajorCollectorConfig {
    fn default() -> Self {
        Self::new(64)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MajorCyclePhase {
    #[default]
    Idle,
    Marking,
    Sweeping,
}

pub(crate) struct MajorCycle {
    pub(crate) phase: MajorCyclePhase,
    pub(crate) pending: Vec<ManagedReference>,
    pub(crate) satb: Vec<ManagedReference>,
    pub(crate) seen: BTreeSet<ManagedReference>,
    pub(crate) marked_mature: BTreeSet<ManagedReference>,
    pub(crate) sweep: Vec<ManagedReference>,
    pub(crate) reclaimed: u64,
    pub(crate) scanned: u64,
}

impl MajorCycle {
    pub(crate) fn idle() -> Self {
        Self {
            phase: MajorCyclePhase::Idle,
            pending: Vec::new(),
            satb: Vec::new(),
            seen: BTreeSet::new(),
            marked_mature: BTreeSet::new(),
            sweep: Vec::new(),
            reclaimed: 0,
            scanned: 0,
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::idle();
    }
}

pub struct GenerationalRuntime {
    pub(crate) nursery: RelocationRuntime,
    pub(crate) major: MajorCycle,
    pub(crate) config: MajorCollectorConfig,
    pub(crate) minor_requested: bool,
    pub(crate) major_requested: bool,
}

impl GenerationalRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::with_config(MajorCollectorConfig::default())
    }

    #[must_use]
    pub fn with_config(config: MajorCollectorConfig) -> Self {
        Self {
            nursery: RelocationRuntime::new(),
            major: MajorCycle::idle(),
            config,
            minor_requested: false,
            major_requested: false,
        }
    }

    pub const fn request_minor_collection(&mut self) {
        self.minor_requested = true;
    }

    pub const fn request_major_collection(&mut self) {
        self.major_requested = true;
    }

    #[must_use]
    pub const fn major_phase(&self) -> MajorCyclePhase {
        self.major.phase
    }

    #[must_use]
    pub const fn major_cycle_active(&self) -> bool {
        !matches!(self.major.phase, MajorCyclePhase::Idle)
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        self.nursery.contains(reference)
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.nursery.object_count()
    }

    /// Establishes a precise snapshot and enables the SATB barrier.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an active cycle or stale root token.
    pub fn start_major_collection(
        &mut self,
        publication: &RootPublication,
    ) -> Result<(), RuntimeFailure> {
        self.begin_major(publication)
    }

    pub(crate) fn finish_major(&mut self) -> Result<CollectionStatistics, RuntimeFailure> {
        let statistics = CollectionStatistics::new(
            u64::try_from(self.nursery.objects.len()).unwrap_or(u64::MAX),
            self.major.reclaimed,
            self.major.scanned,
        );
        self.nursery
            .metrics
            .record_collection(statistics.reclaimed_objects(), statistics.scanned_objects());
        self.major.reset();
        Ok(statistics)
    }
}

impl Default for GenerationalRuntime {
    fn default() -> Self {
        Self::new()
    }
}
