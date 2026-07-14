//! Public configuration and immutable telemetry snapshots.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GenerationalMemoryConfig {
    pub(super) hard_limit_bytes: usize,
    pub(super) emergency_reserve_bytes: usize,
    pub(super) evacuation_reserve_bytes: usize,
    pub(super) minimum_headroom_bytes: usize,
    pub(super) growth_percent: usize,
    pub(super) assist_work_budget: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GenerationalMemoryConfigError {
    ZeroHardLimit,
    ReserveOverflow,
    ReserveExhaustsLimit,
    HeadroomExceedsOrdinaryLimit,
    ZeroGrowthPercent,
    ZeroAssistBudget,
}

impl GenerationalMemoryConfig {
    /// Defines bounded memory admission and allocation-assist policy.
    ///
    /// Both reserves are included inside `hard_limit_bytes`. Ordinary
    /// allocations cannot consume them.
    ///
    /// # Errors
    ///
    /// Rejects unusable limits, overflowing reserves, zero growth, or an
    /// unbounded zero-work assist policy.
    pub const fn new(
        hard_limit_bytes: usize,
        emergency_reserve_bytes: usize,
        evacuation_reserve_bytes: usize,
        minimum_headroom_bytes: usize,
        growth_percent: usize,
        assist_work_budget: usize,
    ) -> Result<Self, GenerationalMemoryConfigError> {
        if hard_limit_bytes == 0 {
            return Err(GenerationalMemoryConfigError::ZeroHardLimit);
        }
        let Some(reserved) = emergency_reserve_bytes.checked_add(evacuation_reserve_bytes) else {
            return Err(GenerationalMemoryConfigError::ReserveOverflow);
        };
        if reserved >= hard_limit_bytes {
            return Err(GenerationalMemoryConfigError::ReserveExhaustsLimit);
        }
        let ordinary_limit = hard_limit_bytes - reserved;
        if minimum_headroom_bytes > ordinary_limit {
            return Err(GenerationalMemoryConfigError::HeadroomExceedsOrdinaryLimit);
        }
        if growth_percent == 0 {
            return Err(GenerationalMemoryConfigError::ZeroGrowthPercent);
        }
        if assist_work_budget == 0 {
            return Err(GenerationalMemoryConfigError::ZeroAssistBudget);
        }
        Ok(Self {
            hard_limit_bytes,
            emergency_reserve_bytes,
            evacuation_reserve_bytes,
            minimum_headroom_bytes,
            growth_percent,
            assist_work_budget,
        })
    }

    #[must_use]
    pub const fn hard_limit_bytes(self) -> usize {
        self.hard_limit_bytes
    }

    #[must_use]
    pub const fn emergency_reserve_bytes(self) -> usize {
        self.emergency_reserve_bytes
    }

    #[must_use]
    pub const fn evacuation_reserve_bytes(self) -> usize {
        self.evacuation_reserve_bytes
    }

    #[must_use]
    pub const fn minimum_headroom_bytes(self) -> usize {
        self.minimum_headroom_bytes
    }

    #[must_use]
    pub const fn growth_percent(self) -> usize {
        self.growth_percent
    }

    #[must_use]
    pub const fn assist_work_budget(self) -> usize {
        self.assist_work_budget
    }

    #[must_use]
    pub const fn ordinary_limit_bytes(self) -> usize {
        self.hard_limit_bytes - self.emergency_reserve_bytes - self.evacuation_reserve_bytes
    }
}

impl Default for GenerationalMemoryConfig {
    fn default() -> Self {
        Self::new(usize::MAX, 0, 0, 4 * 1024 * 1024, 50, 64)
            .expect("default generational memory policy is valid")
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
#[allow(clippy::struct_field_names)]
pub struct NonHeapMemoryUsage {
    stack_bytes: usize,
    code_bytes: usize,
    metadata_bytes: usize,
    native_runtime_bytes: usize,
    arena_bytes: usize,
    isolated_region_bytes: usize,
    total_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NonHeapMemoryUsageError {
    TotalOverflow,
}

impl NonHeapMemoryUsage {
    /// Creates one complete non-heap memory-accounting snapshot.
    ///
    /// # Errors
    ///
    /// Rejects a snapshot whose total cannot be represented.
    pub fn new(
        stack_bytes: usize,
        code_bytes: usize,
        metadata_bytes: usize,
        native_runtime_bytes: usize,
        arena_bytes: usize,
        isolated_region_bytes: usize,
    ) -> Result<Self, NonHeapMemoryUsageError> {
        let Some(total_bytes) = stack_bytes
            .checked_add(code_bytes)
            .and_then(|total| total.checked_add(metadata_bytes))
            .and_then(|total| total.checked_add(native_runtime_bytes))
            .and_then(|total| total.checked_add(arena_bytes))
            .and_then(|total| total.checked_add(isolated_region_bytes))
        else {
            return Err(NonHeapMemoryUsageError::TotalOverflow);
        };
        Ok(Self {
            stack_bytes,
            code_bytes,
            metadata_bytes,
            native_runtime_bytes,
            arena_bytes,
            isolated_region_bytes,
            total_bytes,
        })
    }

    #[must_use]
    pub const fn stack_bytes(self) -> usize {
        self.stack_bytes
    }

    #[must_use]
    pub const fn code_bytes(self) -> usize {
        self.code_bytes
    }

    #[must_use]
    pub const fn metadata_bytes(self) -> usize {
        self.metadata_bytes
    }

    #[must_use]
    pub const fn native_runtime_bytes(self) -> usize {
        self.native_runtime_bytes
    }

    #[must_use]
    pub const fn arena_bytes(self) -> usize {
        self.arena_bytes
    }

    #[must_use]
    pub const fn isolated_region_bytes(self) -> usize {
        self.isolated_region_bytes
    }

    #[must_use]
    pub const fn total_bytes(self) -> usize {
        self.total_bytes
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct GenerationalMemoryTelemetry {
    pub(super) hard_limit_bytes: usize,
    pub(super) ordinary_limit_bytes: usize,
    pub(super) current_target_bytes: usize,
    pub(super) live_bytes: usize,
    pub(super) committed_bytes: usize,
    pub(super) local_bytes: usize,
    pub(super) shared_bytes: usize,
    pub(super) large_object_bytes: usize,
    pub(super) pinned_bytes: usize,
    pub(super) non_heap_bytes: usize,
    pub(super) stack_bytes: usize,
    pub(super) code_bytes: usize,
    pub(super) metadata_bytes: usize,
    pub(super) native_runtime_bytes: usize,
    pub(super) arena_bytes: usize,
    pub(super) isolated_region_bytes: usize,
    pub(super) emergency_reserve_bytes: usize,
    pub(super) evacuation_reserve_bytes: usize,
    pub(super) minor_collection_requests: u64,
    pub(super) major_collection_requests: u64,
    pub(super) mutator_assist_slices: u64,
    pub(super) mutator_assist_work_units: u64,
    pub(super) out_of_memory_failures: u64,
    pub(super) allocation_pressure_events: u64,
    pub(super) allocation_debt_bytes: usize,
    pub(super) peak_committed_bytes: usize,
    pub(super) pages_returned: u64,
}

macro_rules! telemetry_accessors {
    ($($name:ident: $type:ty),* $(,)?) => {
        $(
            #[must_use]
            pub const fn $name(self) -> $type {
                self.$name
            }
        )*
    };
}

impl GenerationalMemoryTelemetry {
    telemetry_accessors! {
        hard_limit_bytes: usize,
        ordinary_limit_bytes: usize,
        current_target_bytes: usize,
        live_bytes: usize,
        committed_bytes: usize,
        local_bytes: usize,
        shared_bytes: usize,
        large_object_bytes: usize,
        pinned_bytes: usize,
        non_heap_bytes: usize,
        stack_bytes: usize,
        code_bytes: usize,
        metadata_bytes: usize,
        native_runtime_bytes: usize,
        arena_bytes: usize,
        isolated_region_bytes: usize,
        emergency_reserve_bytes: usize,
        evacuation_reserve_bytes: usize,
        minor_collection_requests: u64,
        major_collection_requests: u64,
        mutator_assist_slices: u64,
        mutator_assist_work_units: u64,
        out_of_memory_failures: u64,
        allocation_pressure_events: u64,
        allocation_debt_bytes: usize,
        peak_committed_bytes: usize,
        pages_returned: u64,
    }
}
