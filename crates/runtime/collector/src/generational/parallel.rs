//! Independently locked scheduler-local allocation and evacuation contexts.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};

use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, RootPublication, RuntimeAdapter,
    RuntimeFailure, SafePointOutcome, SchedulerId,
};

use super::heap::GenerationalRuntime;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParallelSchedulerLocalConfigError {
    Empty,
    InvalidScheduler(SchedulerId),
    DuplicateScheduler(SchedulerId),
}

impl fmt::Display for ParallelSchedulerLocalConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(
                formatter,
                "parallel scheduler-local runtime requires a scheduler"
            ),
            Self::InvalidScheduler(scheduler) => {
                write!(formatter, "invalid scheduler {}", scheduler.raw())
            }
            Self::DuplicateScheduler(scheduler) => {
                write!(formatter, "duplicate scheduler {}", scheduler.raw())
            }
        }
    }
}

impl Error for ParallelSchedulerLocalConfigError {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ParallelSchedulerLocalTelemetry {
    maximum_parallel_operations: usize,
}

impl ParallelSchedulerLocalTelemetry {
    #[must_use]
    pub const fn maximum_parallel_operations(self) -> usize {
        self.maximum_parallel_operations
    }
}

/// Fixed scheduler inventory with one independently locked local heap per scheduler.
pub struct ParallelSchedulerLocalRuntime {
    schedulers: BTreeMap<SchedulerId, Mutex<GenerationalRuntime>>,
    active_operations: AtomicUsize,
    maximum_parallel_operations: AtomicUsize,
}

impl ParallelSchedulerLocalRuntime {
    /// Creates disjoint scheduler-local contexts and token namespaces.
    ///
    /// # Errors
    ///
    /// Rejects an empty inventory, scheduler zero, or duplicate identities.
    pub fn new(
        schedulers: impl IntoIterator<Item = SchedulerId>,
    ) -> Result<Self, ParallelSchedulerLocalConfigError> {
        let mut contexts = BTreeMap::new();
        for scheduler in schedulers {
            if scheduler.raw() == 0 {
                return Err(ParallelSchedulerLocalConfigError::InvalidScheduler(
                    scheduler,
                ));
            }
            let runtime = GenerationalRuntime::for_scheduler_context(scheduler)
                .map_err(|_| ParallelSchedulerLocalConfigError::InvalidScheduler(scheduler))?;
            if contexts.insert(scheduler, Mutex::new(runtime)).is_some() {
                return Err(ParallelSchedulerLocalConfigError::DuplicateScheduler(
                    scheduler,
                ));
            }
        }
        if contexts.is_empty() {
            return Err(ParallelSchedulerLocalConfigError::Empty);
        }
        Ok(Self {
            schedulers: contexts,
            active_operations: AtomicUsize::new(0),
            maximum_parallel_operations: AtomicUsize::new(0),
        })
    }

    /// Runs local allocation/evacuation work under one explicit scheduler identity.
    ///
    /// # Errors
    ///
    /// Rejects an unknown scheduler, poisoned host lock, or collector failure.
    pub fn with_scheduler<ResultValue>(
        &self,
        scheduler: SchedulerId,
        operation: impl FnOnce(&mut SchedulerLocalContext<'_>) -> Result<ResultValue, RuntimeFailure>,
    ) -> Result<ResultValue, RuntimeFailure> {
        let mut runtime = self.lock_scheduler(scheduler)?;
        let active = self.active_operations.fetch_add(1, Ordering::AcqRel) + 1;
        self.maximum_parallel_operations
            .fetch_max(active, Ordering::AcqRel);
        let _activity = ActiveOperation(&self.active_operations);
        let mut context = SchedulerLocalContext {
            scheduler,
            runtime: &mut runtime,
        };
        operation(&mut context)
    }

    #[must_use]
    pub fn telemetry(&self) -> ParallelSchedulerLocalTelemetry {
        ParallelSchedulerLocalTelemetry {
            maximum_parallel_operations: self.maximum_parallel_operations.load(Ordering::Acquire),
        }
    }

    #[must_use]
    pub fn scheduler_contains(&self, scheduler: SchedulerId, reference: ManagedReference) -> bool {
        self.schedulers
            .get(&scheduler)
            .and_then(|runtime| runtime.lock().ok())
            .is_some_and(|runtime| runtime.contains(reference))
    }

    #[must_use]
    pub fn scheduler_tlab_refills(&self, scheduler: SchedulerId) -> Option<u64> {
        self.schedulers
            .get(&scheduler)?
            .lock()
            .ok()
            .map(|runtime| runtime.allocation_metrics().tlab_refills())
    }

    fn lock_scheduler(
        &self,
        scheduler: SchedulerId,
    ) -> Result<MutexGuard<'_, GenerationalRuntime>, RuntimeFailure> {
        self.schedulers
            .get(&scheduler)
            .ok_or_else(RuntimeFailure::runtime_invariant)?
            .lock()
            .map_err(|_| RuntimeFailure::runtime_invariant())
    }
}

struct ActiveOperation<'a>(&'a AtomicUsize);

impl Drop for ActiveOperation<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Narrow local-heap API that cannot change another scheduler's selected identity.
pub struct SchedulerLocalContext<'a> {
    scheduler: SchedulerId,
    runtime: &'a mut GenerationalRuntime,
}

impl SchedulerLocalContext<'_> {
    /// Allocates into this context's scheduler-local Eden or mature pages.
    ///
    /// # Errors
    ///
    /// Rejects non-local allocation classes and collector failures.
    pub fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        if !matches!(
            request.allocation_class(),
            AllocationClass::NurseryEligible | AllocationClass::Mature
        ) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.runtime.allocate_object(request)
    }

    pub fn request_minor_collection(&mut self) {
        self.runtime.request_minor_collection();
    }

    /// Evacuates only this scheduler's requested nursery.
    ///
    /// # Errors
    ///
    /// Forwards precise-root and collector failures without touching another
    /// scheduler context.
    pub fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        debug_assert_eq!(self.runtime.active_scheduler(), self.scheduler);
        self.runtime.safe_point(roots)
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        self.runtime.contains(reference)
    }
}
