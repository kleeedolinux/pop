//! Public background-worker configuration, startup errors, and telemetry.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BackgroundWorkerConfig {
    pub(super) worker_count: usize,
    pub(super) queue_capacity: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundWorkerConfigError {
    ZeroWorkers,
    ZeroQueueCapacity,
}

impl BackgroundWorkerConfig {
    /// Defines a fixed worker count and bounded queue per worker.
    ///
    /// # Errors
    ///
    /// Rejects configurations with no worker or no queue capacity.
    pub const fn new(
        worker_count: usize,
        queue_capacity: usize,
    ) -> Result<Self, BackgroundWorkerConfigError> {
        if worker_count == 0 {
            Err(BackgroundWorkerConfigError::ZeroWorkers)
        } else if queue_capacity == 0 {
            Err(BackgroundWorkerConfigError::ZeroQueueCapacity)
        } else {
            Ok(Self {
                worker_count,
                queue_capacity,
            })
        }
    }

    #[must_use]
    pub const fn worker_count(self) -> usize {
        self.worker_count
    }

    #[must_use]
    pub const fn queue_capacity(self) -> usize {
        self.queue_capacity
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackgroundWorkerStartError {
    AlreadyStarted,
    ThreadSpawn,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BackgroundWorkerTelemetry {
    pub(super) workers_started: usize,
    pub(super) worker_threads_used: usize,
    pub(super) jobs_submitted: u64,
    pub(super) jobs_completed: u64,
    pub(super) mark_jobs_completed: u64,
    pub(super) card_refinement_jobs_completed: u64,
    pub(super) sweep_jobs_completed: u64,
    pub(super) evacuation_jobs_completed: u64,
    pub(super) batches_completed: u64,
    pub(super) maximum_batch_size: usize,
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

impl BackgroundWorkerTelemetry {
    telemetry_accessors! {
        workers_started: usize,
        worker_threads_used: usize,
        jobs_submitted: u64,
        jobs_completed: u64,
        mark_jobs_completed: u64,
        card_refinement_jobs_completed: u64,
        sweep_jobs_completed: u64,
        evacuation_jobs_completed: u64,
        batches_completed: u64,
        maximum_batch_size: usize,
    }
}
