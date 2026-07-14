//! Generational collector state and public control surface.

use std::collections::BTreeSet;

use pop_runtime_interface::{
    CollectionStatistics, ManagedReference, RootPublication, RuntimeFailure,
};

use crate::relocation::RelocationRuntime;

use super::allocation::{
    AllocationInfrastructure, AllocationInfrastructureConfig, AllocationMetrics,
    AllocationPlacement, PageDescriptor, PageId,
};
use super::memory::{
    GenerationalMemoryConfig, GenerationalMemoryTelemetry, MemoryController, NonHeapMemoryUsage,
};
use super::ownership::IsolationState;
use super::workers::{
    BackgroundWorkerConfig, BackgroundWorkerPool, BackgroundWorkerStartError,
    BackgroundWorkerTelemetry,
};

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
    pub(crate) sweep_cursor: Option<ManagedReference>,
    pub(crate) sweep_complete: bool,
    pub(crate) sweep_entries_examined: u64,
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
            sweep_cursor: None,
            sweep_complete: false,
            sweep_entries_examined: 0,
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
    pub(crate) allocation: AllocationInfrastructure,
    pub(crate) major: MajorCycle,
    pub(crate) config: MajorCollectorConfig,
    pub(crate) memory: MemoryController,
    pub(crate) workers: Option<BackgroundWorkerPool>,
    pub(crate) isolation: IsolationState,
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
        Self::with_allocation_config(config, AllocationInfrastructureConfig::default())
    }

    #[must_use]
    pub fn with_allocation_config(
        config: MajorCollectorConfig,
        allocation: AllocationInfrastructureConfig,
    ) -> Self {
        Self::with_memory_config(config, allocation, GenerationalMemoryConfig::default())
    }

    #[must_use]
    pub fn with_memory_config(
        config: MajorCollectorConfig,
        allocation: AllocationInfrastructureConfig,
        memory: GenerationalMemoryConfig,
    ) -> Self {
        Self {
            nursery: RelocationRuntime::new(),
            allocation: AllocationInfrastructure::new(allocation),
            major: MajorCycle::idle(),
            config,
            memory: MemoryController::new(memory),
            workers: None,
            isolation: IsolationState::new(),
            minor_requested: false,
            major_requested: false,
        }
    }

    /// Creates a collector with persistent bounded host-worker queues.
    ///
    /// # Errors
    ///
    /// Returns a typed startup failure when a host worker cannot be created.
    pub fn with_background_workers(
        workers: BackgroundWorkerConfig,
    ) -> Result<Self, BackgroundWorkerStartError> {
        let mut runtime = Self::new();
        runtime.workers = Some(BackgroundWorkerPool::new(workers)?);
        Ok(runtime)
    }

    pub fn request_minor_collection(&mut self) {
        if !self.minor_requested {
            self.minor_requested = true;
            self.memory.record_minor_request();
        }
    }

    pub fn request_major_collection(&mut self) {
        if !self.major_requested && !self.major_cycle_active() {
            self.major_requested = true;
            self.memory.record_major_request();
        }
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
    pub const fn major_sweep_entries_examined(&self) -> u64 {
        self.major.sweep_entries_examined
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        self.nursery.contains(reference)
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.nursery.object_count()
    }

    #[must_use]
    pub fn placement(&self, reference: ManagedReference) -> Option<AllocationPlacement> {
        self.allocation.placement(reference)
    }

    #[must_use]
    pub fn page_descriptor(&self, page: PageId) -> Option<&PageDescriptor> {
        self.allocation.page(page)
    }

    #[must_use]
    pub const fn allocation_metrics(&self) -> AllocationMetrics {
        self.allocation.metrics()
    }

    #[must_use]
    pub fn memory_telemetry(&self) -> GenerationalMemoryTelemetry {
        self.memory.telemetry(&self.allocation)
    }

    #[must_use]
    pub fn background_worker_telemetry(&self) -> Option<BackgroundWorkerTelemetry> {
        self.workers.as_ref().map(BackgroundWorkerPool::telemetry)
    }

    /// Replaces the complete stack/code/metadata/native/arena/isolated usage
    /// snapshot accounted by this collector's hard limit.
    ///
    /// # Errors
    ///
    /// Returns deterministic out-of-memory without changing the previous
    /// snapshot when the new total would consume protected reserves.
    pub fn set_non_heap_memory_usage(
        &mut self,
        usage: NonHeapMemoryUsage,
    ) -> Result<(), RuntimeFailure> {
        if self.memory.set_non_heap_usage(
            usage,
            self.allocation.live_bytes(),
            self.allocation.committed_bytes(),
        ) {
            Ok(())
        } else {
            self.memory.record_out_of_memory();
            Err(crate::heap::BootstrapRuntime::out_of_memory(0, 0))
        }
    }

    pub(crate) fn update_memory_target(&mut self) {
        self.memory.update_target(
            self.allocation.live_bytes(),
            self.allocation.committed_bytes(),
        );
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

    pub(crate) fn finish_major(&mut self) -> CollectionStatistics {
        let statistics = CollectionStatistics::new(
            u64::try_from(self.nursery.objects.len()).unwrap_or(u64::MAX),
            self.major.reclaimed,
            self.major.scanned,
        );
        self.nursery
            .metrics
            .record_collection(statistics.reclaimed_objects(), statistics.scanned_objects());
        self.major.reset();
        statistics
    }
}

impl Default for GenerationalRuntime {
    fn default() -> Self {
        Self::new()
    }
}
