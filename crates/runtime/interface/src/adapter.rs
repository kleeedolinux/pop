use crate::{
    ArrayAllocationRequest, ForeignCallMode, ForeignTransitionId, GarbageCollectorContract,
    ManagedReference, ManagedThreadBindingId, ObjectAllocationRequest, PanicPayload, PinHandle,
    RootHandle, RootPublication, RuntimeFailure, SchedulerId, TableAllocationRequest, Trap,
    WriteBarrier,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CollectionStatistics {
    live: u64,
    reclaimed: u64,
    scanned: u64,
}

impl CollectionStatistics {
    #[must_use]
    pub const fn new(live_objects: u64, reclaimed_objects: u64, scanned_objects: u64) -> Self {
        Self {
            live: live_objects,
            reclaimed: reclaimed_objects,
            scanned: scanned_objects,
        }
    }

    #[must_use]
    pub const fn live_objects(self) -> u64 {
        self.live
    }

    #[must_use]
    pub const fn reclaimed_objects(self) -> u64 {
        self.reclaimed
    }

    #[must_use]
    pub const fn scanned_objects(self) -> u64 {
        self.scanned
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SafePointOutcome {
    collection: Option<CollectionStatistics>,
}

impl SafePointOutcome {
    #[must_use]
    pub const fn no_collection() -> Self {
        Self { collection: None }
    }

    #[must_use]
    pub const fn collected(statistics: CollectionStatistics) -> Self {
        Self {
            collection: Some(statistics),
        }
    }

    #[must_use]
    pub const fn collection(self) -> Option<CollectionStatistics> {
        self.collection
    }
}

/// Backend-neutral semantic runtime operations consumed by generated code and
/// the MIR reference interpreter.
pub trait RuntimeAdapter {
    fn contract(&self) -> GarbageCollectorContract;

    /// Allocates a traced object with a precise logical pointer map.
    ///
    /// # Errors
    ///
    /// Returns a portable runtime failure when allocation cannot complete.
    fn allocate_object(
        &mut self,
        request: &ObjectAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure>;

    /// Allocates a traced array with a homogeneous element pointer map.
    ///
    /// # Errors
    ///
    /// Returns a portable runtime failure when allocation cannot complete.
    fn allocate_array(
        &mut self,
        request: &ArrayAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure>;

    /// Allocates typed associative storage with homogeneous interleaved key and
    /// value pointer maps.
    ///
    /// # Errors
    ///
    /// Returns a portable runtime failure when allocation cannot complete.
    fn allocate_table(
        &mut self,
        request: &TableAllocationRequest,
    ) -> Result<ManagedReference, RuntimeFailure>;

    /// Registers a strong runtime root.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when the managed reference is invalid.
    fn retain_root(&mut self, reference: ManagedReference) -> Result<RootHandle, RuntimeFailure>;

    /// Resolves one live strong-root handle to its current managed reference.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when root resolution is unavailable or the
    /// handle is invalid, forged, stale, or already released.
    fn resolve_root(&mut self, root: RootHandle) -> Result<ManagedReference, RuntimeFailure> {
        let _ = root;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Releases a previously registered strong runtime root.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when the root handle is invalid.
    fn release_root(&mut self, root: RootHandle) -> Result<(), RuntimeFailure>;

    /// Registers a scoped strong pin for a managed reference at an unsafe
    /// foreign boundary.
    ///
    /// Adapters without a native pinning boundary reject this operation until
    /// they implement the same semantic contract.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when the reference is invalid or pinning is
    /// unavailable.
    fn pin(&mut self, reference: ManagedReference) -> Result<PinHandle, RuntimeFailure> {
        let _ = reference;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Releases a previously registered scoped pin.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when the pin handle is invalid or pinning is
    /// unavailable.
    fn unpin(&mut self, pin: PinHandle) -> Result<(), RuntimeFailure> {
        let _ = pin;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Attaches the current host thread to one logical scheduler before
    /// managed execution begins.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when attachment is unavailable or the
    /// current thread is already bound.
    fn attach_managed_thread(
        &mut self,
        scheduler: SchedulerId,
    ) -> Result<ManagedThreadBindingId, RuntimeFailure> {
        let _ = scheduler;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Detaches and consumes one exact managed-thread binding.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic for a stale, wrong-thread, or active native
    /// transition binding.
    fn detach_managed_thread(
        &mut self,
        binding: ManagedThreadBindingId,
    ) -> Result<(), RuntimeFailure> {
        let _ = binding;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Enters a balanced foreign-call transition after servicing the exact
    /// mutable root publication.
    ///
    /// Adapters without a foreign execution boundary reject this operation.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when the transition is unavailable or root
    /// publication cannot complete.
    fn enter_foreign(
        &mut self,
        roots: &mut RootPublication,
        mode: ForeignCallMode,
    ) -> Result<ForeignTransitionId, RuntimeFailure> {
        let _ = (roots, mode);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Leaves and consumes one balanced foreign-call transition, installing
    /// current managed references into the identical publication.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic for an unavailable, stale, mismatched, or
    /// out-of-order transition.
    fn leave_foreign(
        &mut self,
        transition: ForeignTransitionId,
        roots: &mut RootPublication,
    ) -> Result<(), RuntimeFailure> {
        let _ = (transition, roots);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Publishes precise stack roots and services a requested collection.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when a published reference is invalid.
    fn safe_point(
        &mut self,
        roots: &mut RootPublication,
    ) -> Result<SafePointOutcome, RuntimeFailure>;

    /// Applies the collector's semantic write barrier for a managed-reference
    /// store.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic for invalid owners, slots, or references.
    fn write_barrier(&mut self, barrier: WriteBarrier) -> Result<(), RuntimeFailure>;

    fn raise_trap(&mut self, trap: Trap) -> RuntimeFailure {
        RuntimeFailure::Trap(trap)
    }

    fn begin_panic(&mut self, payload: PanicPayload) -> RuntimeFailure {
        RuntimeFailure::from_panic(payload)
    }
}
