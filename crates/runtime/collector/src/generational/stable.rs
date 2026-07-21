//! Stable-token native composition over the generational mature collector.

use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, FfiBytesBorrow, FfiBytesBorrowId,
    GarbageCollectorContract, ManagedReference, ObjectAllocationRequest, ObjectSlot, PinHandle,
    RootHandle, RootPublication, RuntimeAdapter, RuntimeFailure, RuntimeTypeId, SafePointOutcome,
    SchedulerId, StackMap, TableAllocationRequest, TaskFrameRootId, WriteBarrier,
};

use super::heap::GenerationalRuntime;
use super::task_roots::{TaskFrameRootConfig, TaskFrameRootError, TaskFrameRootTelemetry};
use super::{
    EpochCoordinatorError, EpochCoordinatorTelemetry, EpochProgress, MajorCollectionHandshakeError,
    MutatorExecutionState, MutatorId,
};

pub struct StableGenerationalRuntime {
    inner: GenerationalRuntime,
}

impl StableGenerationalRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self {
            inner: GenerationalRuntime::new(),
        }
    }

    #[must_use]
    pub fn with_task_frame_root_config(config: TaskFrameRootConfig) -> Self {
        Self {
            inner: GenerationalRuntime::with_task_frame_root_config(config),
        }
    }

    /// Selects the logical scheduler for the next serialized native operation.
    pub fn select_scheduler(&mut self, scheduler: SchedulerId) {
        self.inner.select_scheduler(scheduler);
    }

    /// Registers one native worker mutator for an exact logical scheduler.
    ///
    /// # Errors
    ///
    /// Rejects coordinator capacity or typed identity exhaustion.
    pub fn register_scheduler_mutator(
        &mut self,
        scheduler: SchedulerId,
        state: MutatorExecutionState,
    ) -> Result<MutatorId, EpochCoordinatorError> {
        self.inner.select_scheduler(scheduler);
        self.inner.register_mutator(state)
    }

    /// Changes one registered native worker's collector execution state.
    ///
    /// # Errors
    ///
    /// Rejects stale registrations and handshake completion failures.
    pub fn transition_scheduler_mutator(
        &mut self,
        mutator: MutatorId,
        scheduler: SchedulerId,
        state: MutatorExecutionState,
    ) -> Result<EpochProgress, MajorCollectionHandshakeError> {
        if self.inner.mutator_scheduler(mutator) != Some(scheduler) {
            return Err(pop_runtime_interface::RuntimeFailure::runtime_invariant().into());
        }
        self.inner.select_scheduler(scheduler);
        self.inner.transition_mutator(mutator, state)
    }

    /// Removes one native worker mutator after its managed binding is clear.
    ///
    /// # Errors
    ///
    /// Rejects stale registrations and handshake completion failures.
    pub fn unregister_scheduler_mutator(
        &mut self,
        mutator: MutatorId,
        scheduler: SchedulerId,
    ) -> Result<(), MajorCollectionHandshakeError> {
        if self.inner.mutator_scheduler(mutator) != Some(scheduler) {
            return Err(pop_runtime_interface::RuntimeFailure::runtime_invariant().into());
        }
        self.inner.select_scheduler(scheduler);
        self.inner.unregister_mutator(mutator)
    }

    /// Runs one exact managed safe point and acknowledges its active epoch at
    /// most once while scheduler selection remains serialized.
    ///
    /// # Errors
    ///
    /// Rejects stale registrations, scheduler mismatches, invalid roots, and
    /// collector handshake failures.
    pub fn scheduler_mutator_safe_point(
        &mut self,
        mutator: MutatorId,
        scheduler: SchedulerId,
        roots: &mut RootPublication,
    ) -> Result<(SafePointOutcome, bool), MajorCollectionHandshakeError> {
        if self.inner.mutator_scheduler(mutator) != Some(scheduler) {
            return Err(pop_runtime_interface::RuntimeFailure::runtime_invariant().into());
        }
        self.inner.select_scheduler(scheduler);
        let outcome = self.inner.safe_point(roots)?;
        let Some(epoch) = self.inner.active_major_collection_epoch() else {
            return Ok((outcome, false));
        };
        let acknowledged = match self
            .inner
            .acknowledge_major_collection_handshake(mutator, epoch, roots)
        {
            Ok(_) => true,
            Err(MajorCollectionHandshakeError::Coordination(
                EpochCoordinatorError::AlreadyAcknowledged(found),
            )) if found == mutator => false,
            Err(error) => return Err(error),
        };
        Ok((outcome, acknowledged))
    }

    #[must_use]
    pub const fn epoch_coordinator_telemetry(&self) -> EpochCoordinatorTelemetry {
        self.inner.epoch_coordinator_telemetry()
    }

    #[must_use]
    pub fn allocation_scheduler(&self, reference: ManagedReference) -> Option<SchedulerId> {
        let page = self.inner.placement(reference)?.page();
        self.inner.page_descriptor(page)?.scheduler()
    }

    /// Retains the exact roots of one native ready or suspended task frame.
    ///
    /// # Errors
    ///
    /// Forwards bounded collector admission and root-validation failures.
    pub fn retain_task_frame_roots(
        &mut self,
        scheduler: SchedulerId,
        publication: &RootPublication,
    ) -> Result<TaskFrameRootId, TaskFrameRootError> {
        self.inner.retain_task_frame_roots(scheduler, publication)
    }

    /// Restores possibly relocated roots into an exact native frame shape.
    ///
    /// # Errors
    ///
    /// Forwards identity, owner, shape, and private-root failures without
    /// discarding the retained container.
    pub fn restore_task_frame_roots(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
        expected: &StackMap,
    ) -> Result<RootPublication, TaskFrameRootError> {
        self.inner
            .restore_task_frame_roots(identity, scheduler, expected)
    }

    /// Prepares relocated roots without releasing their last valid container.
    ///
    /// # Errors
    ///
    /// Forwards identity, owner, shape, and private-root failures.
    pub fn prepare_task_frame_root_restore(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
        expected: &StackMap,
    ) -> Result<RootPublication, TaskFrameRootError> {
        self.inner
            .prepare_task_frame_root_restore(identity, scheduler, expected)
    }

    /// Commits a successful native frame installation.
    ///
    /// # Errors
    ///
    /// Forwards stale identity, owner, and private-root failures.
    pub fn complete_task_frame_root_restore(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
    ) -> Result<(), TaskFrameRootError> {
        self.inner
            .complete_task_frame_root_restore(identity, scheduler)
    }

    /// Releases roots for a terminal or abandoned native task frame.
    ///
    /// # Errors
    ///
    /// Forwards stale identity and scheduler-owner failures.
    pub fn release_task_frame_roots(
        &mut self,
        identity: TaskFrameRootId,
        scheduler: SchedulerId,
    ) -> Result<(), TaskFrameRootError> {
        self.inner.release_task_frame_roots(identity, scheduler)
    }

    /// Transfers a retained rootless or shared-root frame between schedulers.
    ///
    /// # Errors
    ///
    /// Refuses stale source ownership and scheduler-local frame roots.
    pub fn transfer_task_frame_roots(
        &mut self,
        identity: TaskFrameRootId,
        from: SchedulerId,
        to: SchedulerId,
    ) -> Result<(), TaskFrameRootError> {
        self.inner.transfer_task_frame_roots(identity, from, to)
    }

    #[must_use]
    pub const fn task_frame_root_telemetry(&self) -> TaskFrameRootTelemetry {
        self.inner.task_frame_root_telemetry()
    }

    pub fn request_collection(&mut self) {
        self.inner.request_major_collection();
    }

    #[must_use]
    pub const fn collection_requested(&self) -> bool {
        self.inner.collection_requested()
    }

    #[must_use]
    pub fn contains(&self, reference: ManagedReference) -> bool {
        self.inner.contains(reference)
    }

    #[must_use]
    pub fn object_count(&self) -> usize {
        self.inner.object_count()
    }

    #[must_use]
    pub const fn allocation_metrics(&self) -> super::AllocationMetrics {
        self.inner.allocation_metrics()
    }

    #[must_use]
    pub fn memory_telemetry(&self) -> super::GenerationalMemoryTelemetry {
        self.inner.memory_telemetry()
    }

    #[must_use]
    pub fn allocation_type(&self, reference: ManagedReference) -> Option<RuntimeTypeId> {
        self.inner.allocation_type(reference)
    }

    #[must_use]
    pub fn allocation_class(&self, reference: ManagedReference) -> Option<AllocationClass> {
        self.inner.allocation_class(reference)
    }

    #[must_use]
    pub fn scalar_array_values(
        &self,
        reference: ManagedReference,
        expected_type: RuntimeTypeId,
    ) -> Option<impl ExactSizeIterator<Item = u64> + '_> {
        self.inner.scalar_array_values(reference, expected_type)
    }

    #[must_use]
    pub fn array_length(&self, reference: ManagedReference) -> Option<u64> {
        self.inner.array_length(reference)
    }

    /// Allocates and initializes one stable-token array.
    ///
    /// # Errors
    ///
    /// Forwards typed allocation or initialization failures.
    pub fn allocate_array_filled(
        &mut self,
        request: &ArrayAllocationRequest,
        value: u64,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let stable = ArrayAllocationRequest::new(
            request.type_id(),
            Self::stable_class(request.allocation_class()),
            request.length(),
            request.element_map(),
        );
        self.inner.allocate_array_filled(&stable, value)
    }

    /// Allocates one stable-token object with its complete typed payload.
    ///
    /// # Errors
    ///
    /// Forwards typed allocation, initializer, or memory-admission failures.
    pub fn allocate_object_initialized(
        &mut self,
        request: &ObjectAllocationRequest,
        values: &[u64],
    ) -> Result<ManagedReference, RuntimeFailure> {
        if request.allocation_class() == Self::stable_class(request.allocation_class()) {
            return self.inner.allocate_object_initialized(request, values);
        }
        self.inner.allocate_object_initialized(
            &ObjectAllocationRequest::new(
                request.type_id(),
                Self::stable_class(request.allocation_class()),
                request.object_map().clone(),
            ),
            values,
        )
    }

    /// # Errors
    ///
    /// Forwards invalid array, value, or slot-map failures.
    pub fn fill_array_value(
        &mut self,
        owner: ManagedReference,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        self.inner.fill_array_value(owner, value)
    }

    /// # Errors
    ///
    /// Forwards invalid owner, bounds, or slot-map failures.
    pub fn store_scalar(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        self.inner.store_scalar(owner, slot, value)
    }

    /// # Errors
    ///
    /// Forwards invalid array, bounds, or managed-value failures.
    pub fn store_array_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        self.inner.store_stable_array_value(owner, slot, value)
    }

    /// # Errors
    ///
    /// Forwards invalid owner, slot, or managed-value failures.
    pub fn store_slot_value(
        &mut self,
        owner: ManagedReference,
        slot: ObjectSlot,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        self.inner.store_stable_slot_value(owner, slot, value)
    }

    /// # Errors
    ///
    /// Forwards invalid owner, bounds, or slot-map failures.
    pub fn load_scalar(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        self.inner.load_scalar(owner, slot)
    }

    /// # Errors
    ///
    /// Forwards invalid array or bounds failures.
    pub fn load_array_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        self.inner.load_array_value(owner, slot)
    }

    /// # Errors
    ///
    /// Forwards invalid owner or slot failures.
    pub fn load_slot_value(
        &self,
        owner: ManagedReference,
        slot: ObjectSlot,
    ) -> Result<u64, RuntimeFailure> {
        self.inner.load_slot_value(owner, slot)
    }

    #[must_use]
    pub fn strings_equal(&self, left: ManagedReference, right: ManagedReference) -> bool {
        self.inner.strings_equal(left, right)
    }

    /// # Errors
    ///
    /// Forwards invalid table geometry or memory-admission failures.
    pub fn grow_table(
        &mut self,
        owner: ManagedReference,
        old_capacity: u32,
        new_capacity: u32,
        key_map: pop_runtime_interface::ArrayElementMap,
        value_map: pop_runtime_interface::ArrayElementMap,
    ) -> Result<(), RuntimeFailure> {
        self.inner
            .grow_table(owner, old_capacity, new_capacity, key_map, value_map)
    }

    const fn stable_class(class: AllocationClass) -> AllocationClass {
        match class {
            AllocationClass::NurseryEligible | AllocationClass::Mature => AllocationClass::Mature,
            AllocationClass::Large => AllocationClass::Large,
            AllocationClass::Pinned => AllocationClass::Pinned,
        }
    }
}

impl RuntimeAdapter for StableGenerationalRuntime {
    fn contract(&self) -> GarbageCollectorContract {
        GarbageCollectorContract::native_stable_generational()
    }

    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        if request.allocation_class() == Self::stable_class(request.allocation_class()) {
            return self.inner.allocate_object(request);
        }
        self.inner.allocate_object(&ObjectAllocationRequest::new(
            request.type_id(),
            Self::stable_class(request.allocation_class()),
            request.object_map().clone(),
        ))
    }

    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.inner.allocate_array(&ArrayAllocationRequest::new(
            request.type_id(),
            Self::stable_class(request.allocation_class()),
            request.length(),
            request.element_map(),
        ))
    }

    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let stable = TableAllocationRequest::new(
            request.type_id(),
            Self::stable_class(request.allocation_class()),
            request.entry_count(),
            request.key_map(),
            request.value_map(),
        )
        .map_err(|_| RuntimeFailure::runtime_invariant())?;
        self.inner.allocate_table(&stable)
    }

    fn allocate_immutable_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.inner
            .allocate_immutable_bytes_with_class(bytes, AllocationClass::Mature)
    }

    fn immutable_bytes_length(&self, bytes: ManagedReference) -> Result<u64, RuntimeFailure> {
        self.inner
            .immutable_bytes(bytes)
            .and_then(|payload| u64::try_from(payload.len()).ok())
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn immutable_bytes_read(
        &self,
        bytes: ManagedReference,
        offset: u64,
        target: &mut [u8],
    ) -> Result<(), RuntimeFailure> {
        let payload = self
            .inner
            .immutable_bytes(bytes)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let start = usize::try_from(offset).map_err(|_| RuntimeFailure::runtime_invariant())?;
        let end = start
            .checked_add(target.len())
            .filter(|end| *end <= payload.len())
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        target.copy_from_slice(&payload[start..end]);
        Ok(())
    }

    fn ffi_bytes_borrow(
        &mut self,
        bytes: ManagedReference,
    ) -> Result<FfiBytesBorrow, RuntimeFailure> {
        self.inner.ffi_bytes_borrow(bytes)
    }

    fn ffi_bytes_end_borrow(
        &mut self,
        bytes: ManagedReference,
        borrow: FfiBytesBorrowId,
    ) -> Result<(), RuntimeFailure> {
        self.inner.ffi_bytes_end_borrow(bytes, borrow)
    }

    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure> {
        self.inner.retain_root(reference)
    }

    fn resolve_root(&mut self, root: RootHandle) -> Result<ManagedReference, RuntimeFailure> {
        self.inner.resolve_root(root)
    }

    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure> {
        self.inner.release_root(root)
    }

    fn pin(&mut self, reference: ManagedReference) -> Result<PinHandle, RuntimeFailure> {
        self.inner.pin(reference)
    }

    fn unpin(&mut self, pin: PinHandle) -> Result<(), RuntimeFailure> {
        self.inner.unpin(pin)
    }

    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure> {
        self.inner.safe_point(roots)
    }

    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure> {
        self.inner.write_barrier(barrier)
    }
}

impl Default for StableGenerationalRuntime {
    fn default() -> Self {
        Self::new()
    }
}
