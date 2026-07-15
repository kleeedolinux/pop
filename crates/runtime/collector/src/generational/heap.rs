//! Generational collector state and public control surface.

use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::{
    CollectionStatistics, ManagedReference, RootPublication, RuntimeFailure,
};

use crate::SchedulerId;
use crate::relocation::RelocationRuntime;

use super::allocation::{
    AllocationInfrastructure, AllocationInfrastructureConfig, AllocationMetrics,
    AllocationPlacement, PageDescriptor, PageId, RegionState,
};
use super::arena::ArenaState;
use super::coordination::{
    CollectorEpoch, EpochCoordinator, MajorCollectionHandshakeError, MutatorId,
};
use super::memory::{
    GenerationalMemoryConfig, GenerationalMemoryTelemetry, MemoryController, NonHeapMemoryUsage,
};
use super::ownership::IsolationState;
use super::pinning::{PinningConfig, PinningState, PinningTelemetry};
use super::task_roots::{TaskFrameRootConfig, TaskFrameRootState};
use super::workers::{
    BackgroundWorkerConfig, BackgroundWorkerPool, BackgroundWorkerStartError,
    BackgroundWorkerTelemetry,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MajorCollectorConfig {
    work_budget: usize,
    large_object_scan_chunk_slots: usize,
}

impl MajorCollectorConfig {
    #[must_use]
    pub const fn new(work_budget: usize) -> Self {
        Self::with_large_object_scan_chunk_slots(work_budget, 256)
    }

    #[must_use]
    pub const fn with_large_object_scan_chunk_slots(
        work_budget: usize,
        large_object_scan_chunk_slots: usize,
    ) -> Self {
        Self {
            work_budget: if work_budget == 0 { 1 } else { work_budget },
            large_object_scan_chunk_slots: if large_object_scan_chunk_slots == 0 {
                1
            } else {
                large_object_scan_chunk_slots
            },
        }
    }

    #[must_use]
    pub const fn work_budget(self) -> usize {
        self.work_budget
    }

    #[must_use]
    pub const fn large_object_scan_chunk_slots(self) -> usize {
        self.large_object_scan_chunk_slots
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MajorCollectionTelemetry {
    large_object_scan_chunks_completed: u64,
    maximum_large_object_scan_chunk_slots: usize,
    maximum_pending_large_object_scan_chunks: usize,
    pointer_free_large_objects_seen: u64,
}

impl MajorCollectionTelemetry {
    #[must_use]
    pub const fn large_object_scan_chunks_completed(self) -> u64 {
        self.large_object_scan_chunks_completed
    }

    #[must_use]
    pub const fn maximum_large_object_scan_chunk_slots(self) -> usize {
        self.maximum_large_object_scan_chunk_slots
    }

    #[must_use]
    pub const fn maximum_pending_large_object_scan_chunks(self) -> usize {
        self.maximum_pending_large_object_scan_chunks
    }

    #[must_use]
    pub const fn pointer_free_large_objects_seen(self) -> u64 {
        self.pointer_free_large_objects_seen
    }

    pub(crate) fn record_large_object_scan_chunk(&mut self, slots: usize) {
        self.large_object_scan_chunks_completed =
            self.large_object_scan_chunks_completed.saturating_add(1);
        self.maximum_large_object_scan_chunk_slots =
            self.maximum_large_object_scan_chunk_slots.max(slots);
    }

    pub(crate) fn record_pointer_free_large_object(&mut self) {
        self.pointer_free_large_objects_seen =
            self.pointer_free_large_objects_seen.saturating_add(1);
    }

    pub(crate) fn record_large_object_scan_queue_depth(&mut self, depth: usize) {
        self.maximum_pending_large_object_scan_chunks =
            self.maximum_pending_large_object_scan_chunks.max(depth);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LargeObjectScanChunk {
    pub(crate) reference: ManagedReference,
    pub(crate) start: usize,
    pub(crate) end: usize,
}

impl LargeObjectScanChunk {
    pub(crate) const fn slots(self) -> usize {
        self.end - self.start
    }
}

pub(crate) struct MajorCycle {
    pub(crate) phase: MajorCyclePhase,
    pub(crate) pending: Vec<ManagedReference>,
    pub(crate) pending_large_object_scan_chunks: Vec<LargeObjectScanChunk>,
    pub(crate) prefer_large_object_scan_chunk: bool,
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
            pending_large_object_scan_chunks: Vec::new(),
            prefer_large_object_scan_chunk: true,
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
    pub(crate) major_telemetry: MajorCollectionTelemetry,
    pub(crate) config: MajorCollectorConfig,
    pub(crate) memory: MemoryController,
    pub(crate) workers: Option<BackgroundWorkerPool>,
    pub(crate) isolation: IsolationState,
    pub(crate) pinning: PinningState,
    pub(crate) scheduler: SchedulerId,
    pub(crate) arenas: ArenaState,
    pub(crate) minor_requested: BTreeSet<SchedulerId>,
    pub(crate) major_requested: bool,
    pub(crate) coordination: EpochCoordinator,
    pub(crate) major_epoch: Option<CollectorEpoch>,
    pub(crate) major_root_snapshots: BTreeMap<MutatorId, RootPublication>,
    pub(crate) mutator_schedulers: BTreeMap<MutatorId, SchedulerId>,
    pub(crate) task_frame_roots: TaskFrameRootState,
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
            major_telemetry: MajorCollectionTelemetry::default(),
            config,
            memory: MemoryController::new(memory),
            workers: None,
            isolation: IsolationState::new(),
            pinning: PinningState::new(PinningConfig::default()),
            scheduler: SchedulerId::new(1),
            arenas: ArenaState::new(),
            minor_requested: BTreeSet::new(),
            major_requested: false,
            coordination: EpochCoordinator::default(),
            major_epoch: None,
            major_root_snapshots: BTreeMap::new(),
            mutator_schedulers: BTreeMap::new(),
            task_frame_roots: TaskFrameRootState::new(TaskFrameRootConfig::default()),
        }
    }

    #[must_use]
    pub fn with_task_frame_root_config(config: TaskFrameRootConfig) -> Self {
        let mut runtime = Self::new();
        runtime.task_frame_roots = TaskFrameRootState::new(config);
        runtime
    }

    pub fn select_scheduler(&mut self, scheduler: SchedulerId) {
        self.scheduler = scheduler;
    }

    #[must_use]
    pub fn with_pinning_config(config: PinningConfig) -> Self {
        let mut runtime = Self::new();
        runtime.pinning = PinningState::new(config);
        runtime
    }

    #[must_use]
    pub const fn active_scheduler(&self) -> SchedulerId {
        self.scheduler
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
        runtime.start_background_workers(workers)?;
        Ok(runtime)
    }

    /// Attaches persistent bounded host workers to this configured runtime.
    ///
    /// # Errors
    ///
    /// Rejects a second worker pool or returns a typed thread-start failure
    /// without replacing the runtime's current collector state.
    pub fn start_background_workers(
        &mut self,
        workers: BackgroundWorkerConfig,
    ) -> Result<(), BackgroundWorkerStartError> {
        if self.workers.is_some() {
            return Err(BackgroundWorkerStartError::AlreadyStarted);
        }
        self.workers = Some(BackgroundWorkerPool::new(workers)?);
        Ok(())
    }

    pub fn request_minor_collection(&mut self) {
        if self.minor_requested.insert(self.scheduler) {
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
    pub const fn collection_requested(&self) -> bool {
        self.major_requested || self.major_cycle_active() || self.major_epoch.is_some()
    }

    #[must_use]
    pub const fn major_sweep_entries_examined(&self) -> u64 {
        self.major.sweep_entries_examined
    }

    #[must_use]
    pub const fn major_collection_telemetry(&self) -> MajorCollectionTelemetry {
        self.major_telemetry
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

    #[must_use]
    pub fn pinning_telemetry(&self) -> PinningTelemetry {
        self.pinning.telemetry()
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
        if self.coordination.registered_mutators() != 0 || self.major_epoch.is_some() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.begin_major(publication)
    }

    pub(crate) fn handshake_failure(_error: MajorCollectionHandshakeError) -> RuntimeFailure {
        RuntimeFailure::runtime_invariant()
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
        self.allocation.reclaim_empty_pages_after_sweep();
        self.allocation
            .transition_shared_regions(RegionState::SharedAllocating);
        self.major.reset();
        statistics
    }
}

impl Default for GenerationalRuntime {
    fn default() -> Self {
        Self::new()
    }
}
