use pop_runtime_collector::BootstrapRuntime;
use pop_runtime_interface::{
    ManagedReference, RootPublication, RuntimeAdapter, RuntimeFailure, SafePointId, StackMap,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkloadConfiguration {
    pub batches: u32,
    pub items_per_batch: u32,
    pub slots_per_object: u32,
    pub pressure_limit: u32,
}

impl WorkloadConfiguration {
    pub(crate) fn validate(self) -> Result<Self, RuntimeFailure> {
        if self.batches == 0 || self.items_per_batch == 0 || self.pressure_limit == 0 {
            Err(RuntimeFailure::runtime_invariant())
        } else {
            Ok(self)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkloadCounters {
    pub workload: &'static str,
    pub graph_shape: &'static str,
    pub roots: u64,
    pub operations: u64,
    pub allocations: u64,
    pub reference_stores: u64,
    pub root_transitions: u64,
    pub pin_transitions: u64,
    pub collections: u64,
    pub reclaimed_objects: u64,
    pub scanned_objects: u64,
    pub logical_peak_objects: u64,
    pub logical_peak_slots: u64,
    pub final_live_objects: u64,
    pub final_live_slots: u64,
}

#[derive(Default)]
pub(crate) struct WorkloadState {
    pub(crate) reference_stores: u64,
    pub(crate) root_transitions: u64,
    pub(crate) pin_transitions: u64,
    pub(crate) logical_peak_objects: u64,
    pub(crate) logical_peak_slots: u64,
}

impl WorkloadState {
    pub(crate) fn observe(&mut self, runtime: &BootstrapRuntime) {
        self.logical_peak_objects = self
            .logical_peak_objects
            .max(u64::try_from(runtime.object_count()).unwrap_or(u64::MAX));
        self.logical_peak_slots = self
            .logical_peak_slots
            .max(u64::try_from(runtime.slot_count()).unwrap_or(u64::MAX));
    }

    pub(crate) fn finish(
        self,
        runtime: &BootstrapRuntime,
        workload: &'static str,
        graph_shape: &'static str,
        roots: u64,
    ) -> WorkloadCounters {
        let metrics = runtime.metrics();
        let operations = metrics
            .allocations()
            .saturating_add(self.reference_stores)
            .saturating_add(self.root_transitions)
            .saturating_add(self.pin_transitions);
        WorkloadCounters {
            workload,
            graph_shape,
            roots,
            operations,
            allocations: metrics.allocations(),
            reference_stores: self.reference_stores,
            root_transitions: self.root_transitions,
            pin_transitions: self.pin_transitions,
            collections: metrics.collections(),
            reclaimed_objects: metrics.reclaimed_objects(),
            scanned_objects: metrics.scanned_objects(),
            logical_peak_objects: self.logical_peak_objects,
            logical_peak_slots: self.logical_peak_slots,
            final_live_objects: u64::try_from(runtime.object_count()).unwrap_or(u64::MAX),
            final_live_slots: u64::try_from(runtime.slot_count()).unwrap_or(u64::MAX),
        }
    }
}

pub(crate) fn empty_roots() -> Result<RootPublication, RuntimeFailure> {
    let map = StackMap::new(SafePointId::new(1), Vec::new())
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
    RootPublication::new(map, Vec::<Option<ManagedReference>>::new())
        .map_err(|_| RuntimeFailure::runtime_invariant())
}

pub(crate) fn collect(
    runtime: &mut BootstrapRuntime,
    roots: &mut RootPublication,
) -> Result<(), RuntimeFailure> {
    runtime.request_collection();
    runtime
        .safe_point(roots)?
        .collection()
        .ok_or_else(RuntimeFailure::runtime_invariant)?;
    Ok(())
}
