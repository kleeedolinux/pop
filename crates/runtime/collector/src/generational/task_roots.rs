//! Bounded collector-owned precise roots for queued and suspended task frames.

use std::collections::BTreeMap;

use pop_runtime_interface::{
    RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, SchedulerId, StackMap,
    TaskFrameRootId,
};

use super::heap::GenerationalRuntime;

const DEFAULT_MAXIMUM_CONTAINERS: usize = 1_000_000;
const DEFAULT_MAXIMUM_SLOTS: usize = 16_000_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskFrameRootConfigError {
    ZeroMaximumContainers,
    ZeroMaximumSlots,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TaskFrameRootConfig {
    maximum_containers: usize,
    maximum_slots: usize,
}

impl TaskFrameRootConfig {
    /// Creates explicit bounds for retained task frames and their exact slots.
    ///
    /// # Errors
    ///
    /// Rejects zero bounds because they cannot admit the scheduler's explicit
    /// empty-frame publication contract.
    pub const fn new(
        maximum_containers: usize,
        maximum_slots: usize,
    ) -> Result<Self, TaskFrameRootConfigError> {
        if maximum_containers == 0 {
            return Err(TaskFrameRootConfigError::ZeroMaximumContainers);
        }
        if maximum_slots == 0 {
            return Err(TaskFrameRootConfigError::ZeroMaximumSlots);
        }
        Ok(Self {
            maximum_containers,
            maximum_slots,
        })
    }

    #[must_use]
    pub const fn maximum_containers(self) -> usize {
        self.maximum_containers
    }

    #[must_use]
    pub const fn maximum_slots(self) -> usize {
        self.maximum_slots
    }
}

impl Default for TaskFrameRootConfig {
    fn default() -> Self {
        Self {
            maximum_containers: DEFAULT_MAXIMUM_CONTAINERS,
            maximum_slots: DEFAULT_MAXIMUM_SLOTS,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TaskFrameRootError {
    ContainerCapacityExceeded {
        maximum: usize,
    },
    SlotCapacityExceeded {
        maximum: usize,
    },
    IdentityExhausted,
    UnknownContainer(TaskFrameRootId),
    SchedulerMismatch {
        expected: SchedulerId,
        found: SchedulerId,
    },
    StackMapMismatch,
    MissingRootHandle(RootHandle),
    Runtime(RuntimeFailure),
}

impl From<RuntimeFailure> for TaskFrameRootError {
    fn from(error: RuntimeFailure) -> Self {
        Self::Runtime(error)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TaskFrameRootTelemetry {
    current_containers: usize,
    current_slots: usize,
    maximum_containers: usize,
    maximum_slots: usize,
    containers_retained: u64,
    containers_restored: u64,
    containers_released: u64,
    transfers_completed: u64,
    transfers_refused: u64,
    admission_failures: u64,
    restoration_failures: u64,
    cleanup_failures: u64,
}

impl TaskFrameRootTelemetry {
    #[must_use]
    pub const fn current_containers(self) -> usize {
        self.current_containers
    }

    #[must_use]
    pub const fn current_slots(self) -> usize {
        self.current_slots
    }

    #[must_use]
    pub const fn maximum_containers(self) -> usize {
        self.maximum_containers
    }

    #[must_use]
    pub const fn maximum_slots(self) -> usize {
        self.maximum_slots
    }

    #[must_use]
    pub const fn containers_retained(self) -> u64 {
        self.containers_retained
    }

    #[must_use]
    pub const fn containers_restored(self) -> u64 {
        self.containers_restored
    }

    #[must_use]
    pub const fn containers_released(self) -> u64 {
        self.containers_released
    }

    #[must_use]
    pub const fn transfers_completed(self) -> u64 {
        self.transfers_completed
    }

    #[must_use]
    pub const fn transfers_refused(self) -> u64 {
        self.transfers_refused
    }

    #[must_use]
    pub const fn admission_failures(self) -> u64 {
        self.admission_failures
    }

    #[must_use]
    pub const fn restoration_failures(self) -> u64 {
        self.restoration_failures
    }

    #[must_use]
    pub const fn cleanup_failures(self) -> u64 {
        self.cleanup_failures
    }
}

#[derive(Clone)]
struct TaskFrameRootRecord {
    scheduler: SchedulerId,
    stack_map: StackMap,
    handles: Vec<Option<RootHandle>>,
}

pub(crate) struct TaskFrameRootState {
    config: TaskFrameRootConfig,
    records: BTreeMap<TaskFrameRootId, TaskFrameRootRecord>,
    next_identity: u64,
    current_slots: usize,
    telemetry: TaskFrameRootTelemetry,
}

impl TaskFrameRootState {
    pub(crate) fn new(config: TaskFrameRootConfig) -> Self {
        Self {
            config,
            records: BTreeMap::new(),
            next_identity: 1,
            current_slots: 0,
            telemetry: TaskFrameRootTelemetry::default(),
        }
    }
}

impl GenerationalRuntime {
    /// Retains the exact roots of one ready or suspended task frame.
    ///
    /// # Errors
    ///
    /// Fails before publishing a container when its bounds, identity, root
    /// tokens, or collector ownership are invalid.
    pub fn retain_task_frame_roots(
        &mut self,
        scheduler: SchedulerId,
        publication: &RootPublication,
    ) -> Result<TaskFrameRootId, TaskFrameRootError> {
        let slot_count = publication.stack_map().root_slots().len();
        let next_slots = self
            .task_frame_roots
            .current_slots
            .checked_add(slot_count)
            .filter(|slots| *slots <= self.task_frame_roots.config.maximum_slots());
        let Some(next_slots) = next_slots else {
            self.task_frame_roots.telemetry.admission_failures = self
                .task_frame_roots
                .telemetry
                .admission_failures
                .saturating_add(1);
            return Err(TaskFrameRootError::SlotCapacityExceeded {
                maximum: self.task_frame_roots.config.maximum_slots(),
            });
        };
        if self.task_frame_roots.records.len() >= self.task_frame_roots.config.maximum_containers()
        {
            self.task_frame_roots.telemetry.admission_failures = self
                .task_frame_roots
                .telemetry
                .admission_failures
                .saturating_add(1);
            return Err(TaskFrameRootError::ContainerCapacityExceeded {
                maximum: self.task_frame_roots.config.maximum_containers(),
            });
        }
        let identity = TaskFrameRootId::new(self.task_frame_roots.next_identity);
        let Some(next_identity) = self.task_frame_roots.next_identity.checked_add(1) else {
            self.task_frame_roots.telemetry.admission_failures = self
                .task_frame_roots
                .telemetry
                .admission_failures
                .saturating_add(1);
            return Err(TaskFrameRootError::IdentityExhausted);
        };

        for reference in publication.managed_references() {
            let ownership_is_valid = match self.ownership(reference) {
                Some(crate::ObjectOwnership::Shared) => true,
                Some(crate::ObjectOwnership::SchedulerLocal(owner)) => owner == scheduler,
                Some(crate::ObjectOwnership::Isolated(_)) | None => false,
            };
            if !self.nursery.contains(reference) || !ownership_is_valid {
                self.task_frame_roots.telemetry.admission_failures = self
                    .task_frame_roots
                    .telemetry
                    .admission_failures
                    .saturating_add(1);
                return Err(TaskFrameRootError::Runtime(
                    RuntimeFailure::runtime_invariant(),
                ));
            }
        }

        let mut handles = Vec::with_capacity(slot_count);
        for (_, reference) in publication.root_values() {
            let handle = match reference {
                Some(reference) => match RuntimeAdapter::retain_root(self, reference) {
                    Ok(handle) => Some(handle),
                    Err(error) => {
                        for handle in handles.iter().flatten().copied() {
                            if RuntimeAdapter::release_root(self, handle).is_err() {
                                self.task_frame_roots.telemetry.cleanup_failures = self
                                    .task_frame_roots
                                    .telemetry
                                    .cleanup_failures
                                    .saturating_add(1);
                            }
                        }
                        self.task_frame_roots.telemetry.admission_failures = self
                            .task_frame_roots
                            .telemetry
                            .admission_failures
                            .saturating_add(1);
                        return Err(TaskFrameRootError::Runtime(error));
                    }
                },
                None => None,
            };
            handles.push(handle);
        }

        self.task_frame_roots.next_identity = next_identity;
        self.task_frame_roots.current_slots = next_slots;
        self.task_frame_roots.records.insert(
            identity,
            TaskFrameRootRecord {
                scheduler,
                stack_map: publication.stack_map().clone(),
                handles,
            },
        );
        let telemetry = &mut self.task_frame_roots.telemetry;
        telemetry.current_containers = self.task_frame_roots.records.len();
        telemetry.current_slots = self.task_frame_roots.current_slots;
        telemetry.maximum_containers = telemetry
            .maximum_containers
            .max(telemetry.current_containers);
        telemetry.maximum_slots = telemetry.maximum_slots.max(telemetry.current_slots);
        telemetry.containers_retained = telemetry.containers_retained.saturating_add(1);
        Ok(identity)
    }

    /// Removes one retained container and returns relocated values for the
    /// exact compiler-known frame shape.
    ///
    /// # Errors
    ///
    /// Wrong ownership, shape, identity, or missing private root handles leave
    /// the last valid container retained.
    pub fn restore_task_frame_roots(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
        expected: &StackMap,
    ) -> Result<RootPublication, TaskFrameRootError> {
        let publication = self.prepare_task_frame_root_restore(identity, scheduler, expected)?;
        self.complete_task_frame_root_restore(identity, scheduler)?;
        Ok(publication)
    }

    /// Reads the current relocated values while keeping their container live.
    ///
    /// A scheduler uses this phase before asking the compiler-generated frame
    /// adapter to install values. Failure or rejection leaves the same
    /// container available for a retry or explicit cleanup.
    ///
    /// # Errors
    ///
    /// Wrong ownership, shape, identity, or missing private root handles leave
    /// the last valid container retained.
    pub fn prepare_task_frame_root_restore(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
        expected: &StackMap,
    ) -> Result<RootPublication, TaskFrameRootError> {
        let Some(record) = self.task_frame_roots.records.get(&identity) else {
            self.record_restoration_failure();
            return Err(TaskFrameRootError::UnknownContainer(identity));
        };
        if record.scheduler != scheduler {
            let expected = record.scheduler;
            self.record_restoration_failure();
            return Err(TaskFrameRootError::SchedulerMismatch {
                expected,
                found: scheduler,
            });
        }
        if record.stack_map != *expected {
            self.record_restoration_failure();
            return Err(TaskFrameRootError::StackMapMismatch);
        }
        let stack_map = record.stack_map.clone();
        let handles = record.handles.clone();
        let mut values = Vec::with_capacity(handles.len());
        for handle in &handles {
            let value = match handle {
                Some(handle) => Some(
                    if let Some(reference) = self.nursery.roots.get(handle).copied() {
                        reference
                    } else {
                        self.record_restoration_failure();
                        return Err(TaskFrameRootError::MissingRootHandle(*handle));
                    },
                ),
                None => None,
            };
            values.push(value);
        }
        let publication = RootPublication::new(stack_map, values)
            .map_err(|_| TaskFrameRootError::StackMapMismatch)?;
        Ok(publication)
    }

    /// Commits successful installation by releasing the retained container.
    ///
    /// # Errors
    ///
    /// Unknown identities, foreign scheduler owners, or missing private roots
    /// fail without releasing a partial container.
    pub fn complete_task_frame_root_restore(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
    ) -> Result<(), TaskFrameRootError> {
        let Some(record) = self.task_frame_roots.records.get(&identity) else {
            self.record_restoration_failure();
            return Err(TaskFrameRootError::UnknownContainer(identity));
        };
        if record.scheduler != scheduler {
            let expected = record.scheduler;
            self.record_restoration_failure();
            return Err(TaskFrameRootError::SchedulerMismatch {
                expected,
                found: scheduler,
            });
        }
        let handles = record.handles.clone();
        self.remove_task_frame_root_record(identity, &handles)?;
        self.task_frame_roots.telemetry.containers_restored = self
            .task_frame_roots
            .telemetry
            .containers_restored
            .saturating_add(1);
        Ok(())
    }

    /// Releases an abandoned or terminal task frame without restoring it.
    ///
    /// # Errors
    ///
    /// Unknown or foreign containers fail without releasing another task's
    /// roots.
    pub fn release_task_frame_roots(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
    ) -> Result<(), TaskFrameRootError> {
        let Some(record) = self.task_frame_roots.records.get(&identity) else {
            self.task_frame_roots.telemetry.cleanup_failures = self
                .task_frame_roots
                .telemetry
                .cleanup_failures
                .saturating_add(1);
            return Err(TaskFrameRootError::UnknownContainer(identity));
        };
        if record.scheduler != scheduler {
            let expected = record.scheduler;
            self.task_frame_roots.telemetry.cleanup_failures = self
                .task_frame_roots
                .telemetry
                .cleanup_failures
                .saturating_add(1);
            return Err(TaskFrameRootError::SchedulerMismatch {
                expected,
                found: scheduler,
            });
        }
        let handles = record.handles.clone();
        self.remove_task_frame_root_record(identity, &handles)?;
        self.task_frame_roots.telemetry.containers_released = self
            .task_frame_roots
            .telemetry
            .containers_released
            .saturating_add(1);
        Ok(())
    }

    /// Transfers only an exact rootless container or one whose roots are
    /// already shared.
    ///
    /// # Errors
    ///
    /// Refuses stale source ownership and scheduler-local roots without
    /// changing the retained container.
    pub fn transfer_task_frame_roots(
        &mut self,
        identity: TaskFrameRootId,
        from: SchedulerId,
        to: SchedulerId,
    ) -> Result<(), TaskFrameRootError> {
        let Some(record) = self.task_frame_roots.records.get(&identity) else {
            self.record_transfer_refusal();
            return Err(TaskFrameRootError::UnknownContainer(identity));
        };
        if record.scheduler != from {
            let expected = record.scheduler;
            self.record_transfer_refusal();
            return Err(TaskFrameRootError::SchedulerMismatch {
                expected,
                found: from,
            });
        }
        let handles = record.handles.clone();
        for handle in handles.iter().flatten() {
            let Some(reference) = self.nursery.roots.get(handle).copied() else {
                self.record_transfer_refusal();
                return Err(TaskFrameRootError::MissingRootHandle(*handle));
            };
            if !matches!(
                self.ownership(reference),
                Some(crate::ObjectOwnership::Shared)
            ) {
                self.record_transfer_refusal();
                return Err(TaskFrameRootError::Runtime(
                    RuntimeFailure::runtime_invariant(),
                ));
            }
        }
        let record = self
            .task_frame_roots
            .records
            .get_mut(&identity)
            .ok_or(TaskFrameRootError::UnknownContainer(identity))?;
        record.scheduler = to;
        self.task_frame_roots.telemetry.transfers_completed = self
            .task_frame_roots
            .telemetry
            .transfers_completed
            .saturating_add(1);
        Ok(())
    }

    #[must_use]
    pub const fn task_frame_root_telemetry(&self) -> TaskFrameRootTelemetry {
        self.task_frame_roots.telemetry
    }

    fn remove_task_frame_root_record(
        &mut self,
        identity: TaskFrameRootId,
        handles: &[Option<RootHandle>],
    ) -> Result<(), TaskFrameRootError> {
        let Some(record) = self.task_frame_roots.records.get(&identity) else {
            self.task_frame_roots.telemetry.cleanup_failures = self
                .task_frame_roots
                .telemetry
                .cleanup_failures
                .saturating_add(1);
            return Err(TaskFrameRootError::UnknownContainer(identity));
        };
        let Some(next_slots) = self
            .task_frame_roots
            .current_slots
            .checked_sub(record.stack_map.root_slots().len())
        else {
            self.task_frame_roots.telemetry.cleanup_failures = self
                .task_frame_roots
                .telemetry
                .cleanup_failures
                .saturating_add(1);
            return Err(TaskFrameRootError::Runtime(
                RuntimeFailure::runtime_invariant(),
            ));
        };
        for handle in handles.iter().flatten() {
            if !self.nursery.roots.contains_key(handle) {
                self.task_frame_roots.telemetry.cleanup_failures = self
                    .task_frame_roots
                    .telemetry
                    .cleanup_failures
                    .saturating_add(1);
                return Err(TaskFrameRootError::MissingRootHandle(*handle));
            }
        }
        for handle in handles.iter().flatten() {
            self.nursery.roots.remove(handle);
        }
        self.task_frame_roots.records.remove(&identity);
        self.task_frame_roots.current_slots = next_slots;
        self.task_frame_roots.telemetry.current_containers = self.task_frame_roots.records.len();
        self.task_frame_roots.telemetry.current_slots = self.task_frame_roots.current_slots;
        Ok(())
    }

    fn record_restoration_failure(&mut self) {
        self.task_frame_roots.telemetry.restoration_failures = self
            .task_frame_roots
            .telemetry
            .restoration_failures
            .saturating_add(1);
    }

    fn record_transfer_refusal(&mut self) {
        self.task_frame_roots.telemetry.transfers_refused = self
            .task_frame_roots
            .telemetry
            .transfers_refused
            .saturating_add(1);
    }
}
