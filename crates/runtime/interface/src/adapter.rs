use crate::{
    ArrayAllocationRequest, GarbageCollectorContract, ManagedReference, ObjectAllocationRequest,
    PanicPayload, PinHandle, RootHandle, RootPublication, RuntimeFailure, TableAllocationRequest,
    Trap, WriteBarrier,
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
