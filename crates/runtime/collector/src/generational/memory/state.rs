//! Mutable controller counters and adaptive target calculation.

use super::model::{GenerationalMemoryConfig, GenerationalMemoryTelemetry, NonHeapMemoryUsage};
use crate::generational::allocation::{AllocationInfrastructure, HeapDomain};

pub(crate) struct MemoryController {
    config: GenerationalMemoryConfig,
    current_target_bytes: usize,
    minor_collection_requests: u64,
    major_collection_requests: u64,
    mutator_assist_slices: u64,
    mutator_assist_work_units: u64,
    out_of_memory_failures: u64,
    allocation_pressure_events: u64,
    allocation_debt_bytes: usize,
    peak_committed_bytes: usize,
    non_heap: NonHeapMemoryUsage,
}

impl MemoryController {
    pub(crate) fn new(config: GenerationalMemoryConfig) -> Self {
        Self {
            current_target_bytes: config
                .minimum_headroom_bytes
                .min(config.ordinary_limit_bytes()),
            config,
            minor_collection_requests: 0,
            major_collection_requests: 0,
            mutator_assist_slices: 0,
            mutator_assist_work_units: 0,
            out_of_memory_failures: 0,
            allocation_pressure_events: 0,
            allocation_debt_bytes: 0,
            peak_committed_bytes: 0,
            non_heap: NonHeapMemoryUsage::default(),
        }
    }

    pub(crate) const fn assist_work_budget(&self) -> usize {
        self.config.assist_work_budget
    }

    pub(crate) fn pressure_for(&self, committed_after: usize) -> bool {
        committed_after > self.current_target_bytes
    }

    pub(crate) fn record_pressure(&mut self, committed_after: usize) {
        self.allocation_pressure_events = self.allocation_pressure_events.saturating_add(1);
        self.allocation_debt_bytes = self
            .allocation_debt_bytes
            .max(committed_after.saturating_sub(self.current_target_bytes));
    }

    pub(crate) fn observe_committed(&mut self, committed_bytes: usize) {
        self.peak_committed_bytes = self.peak_committed_bytes.max(committed_bytes);
        self.allocation_debt_bytes = committed_bytes.saturating_sub(self.current_target_bytes);
    }

    pub(crate) fn admits(&self, committed_after: usize) -> bool {
        committed_after <= self.ordinary_limit_bytes()
    }

    pub(crate) fn set_non_heap_usage(
        &mut self,
        usage: NonHeapMemoryUsage,
        live_bytes: usize,
        committed_bytes: usize,
    ) -> bool {
        let Some(total) = usage.total_bytes().checked_add(committed_bytes) else {
            return false;
        };
        if total > self.config.ordinary_limit_bytes() {
            return false;
        }
        self.non_heap = usage;
        self.update_target(live_bytes, committed_bytes);
        true
    }

    fn ordinary_limit_bytes(&self) -> usize {
        self.config
            .ordinary_limit_bytes()
            .saturating_sub(self.non_heap.total_bytes())
    }

    pub(crate) fn record_minor_request(&mut self) {
        self.minor_collection_requests = self.minor_collection_requests.saturating_add(1);
    }

    pub(crate) fn record_major_request(&mut self) {
        self.major_collection_requests = self.major_collection_requests.saturating_add(1);
    }

    pub(crate) fn record_assist(&mut self, work_units: usize) {
        self.mutator_assist_slices = self.mutator_assist_slices.saturating_add(1);
        self.mutator_assist_work_units = self
            .mutator_assist_work_units
            .saturating_add(u64::try_from(work_units).unwrap_or(u64::MAX));
    }

    pub(crate) fn record_out_of_memory(&mut self) {
        self.out_of_memory_failures = self.out_of_memory_failures.saturating_add(1);
    }

    pub(crate) fn update_target(&mut self, live_bytes: usize, committed_bytes: usize) {
        let proportional = live_bytes
            .saturating_mul(self.config.growth_percent)
            .saturating_div(100);
        let headroom = self.config.minimum_headroom_bytes.max(proportional);
        self.current_target_bytes = live_bytes
            .saturating_add(headroom)
            .min(self.ordinary_limit_bytes());
        self.observe_committed(committed_bytes);
    }

    pub(crate) fn telemetry(
        &self,
        allocation: &AllocationInfrastructure,
    ) -> GenerationalMemoryTelemetry {
        GenerationalMemoryTelemetry {
            hard_limit_bytes: self.config.hard_limit_bytes,
            ordinary_limit_bytes: self.ordinary_limit_bytes(),
            current_target_bytes: self.current_target_bytes,
            live_bytes: allocation.live_bytes(),
            committed_bytes: allocation.committed_bytes(),
            local_bytes: allocation.bytes_in_domains(&[
                HeapDomain::LocalEden,
                HeapDomain::LocalSurvivor,
                HeapDomain::LocalMature,
            ]),
            shared_bytes: allocation.bytes_in_domains(&[HeapDomain::Shared]),
            large_object_bytes: allocation.bytes_in_domains(&[HeapDomain::LargeObject]),
            pinned_bytes: allocation.bytes_in_domains(&[HeapDomain::Pinned]),
            non_heap_bytes: self.non_heap.total_bytes(),
            stack_bytes: self.non_heap.stack_bytes(),
            code_bytes: self.non_heap.code_bytes(),
            metadata_bytes: self.non_heap.metadata_bytes(),
            native_runtime_bytes: self.non_heap.native_runtime_bytes(),
            arena_bytes: self.non_heap.arena_bytes(),
            isolated_region_bytes: self
                .non_heap
                .isolated_region_bytes()
                .saturating_add(allocation.bytes_in_domains(&[HeapDomain::Isolated])),
            emergency_reserve_bytes: self.config.emergency_reserve_bytes,
            evacuation_reserve_bytes: self.config.evacuation_reserve_bytes,
            minor_collection_requests: self.minor_collection_requests,
            major_collection_requests: self.major_collection_requests,
            mutator_assist_slices: self.mutator_assist_slices,
            mutator_assist_work_units: self.mutator_assist_work_units,
            out_of_memory_failures: self.out_of_memory_failures,
            allocation_pressure_events: self.allocation_pressure_events,
            allocation_debt_bytes: self.allocation_debt_bytes,
            peak_committed_bytes: self.peak_committed_bytes,
            pages_returned: allocation.metrics().pages_returned(),
        }
    }
}
