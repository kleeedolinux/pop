use crate::{
    ArrayAllocationRequest, FfiAbiLayoutId, FfiBufferBorrow, FfiBufferBorrowId,
    FfiBufferOpenFailure, FfiBufferOpenRequest, FfiBytesBorrow, FfiBytesBorrowId, ForeignAddress,
    ForeignCallMode, ForeignTransitionId, GarbageCollectorContract, ManagedReference,
    ManagedThreadBindingId, ObjectAllocationRequest, PanicPayload, PinHandle, RootHandle,
    RootPublication, RuntimeFailure, SchedulerId, TableAllocationRequest, Trap, WriteBarrier,
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

    /// Allocates zero-initialized foreign storage with one exact ABI layout.
    ///
    /// # Errors
    ///
    /// Distinguishes allocation exhaustion from a runtime invariant failure.
    fn ffi_buffer_open(
        &mut self,
        request: &FfiBufferOpenRequest,
    ) -> Result<ManagedReference, FfiBufferOpenFailure> {
        let _ = request;
        Err(FfiBufferOpenFailure::Invariant(
            RuntimeFailure::runtime_invariant(),
        ))
    }

    /// Returns the element length of a live buffer with the expected layout.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for an invalid, closed, or mismatched buffer.
    fn ffi_buffer_length(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
    ) -> Result<u64, RuntimeFailure> {
        let _ = (buffer, layout);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Copies one element from a live buffer without partially changing `output`.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for invalid storage, layout, bounds, or size.
    fn ffi_buffer_read(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
        index: u64,
        output: &mut [u8],
    ) -> Result<(), RuntimeFailure> {
        let _ = (buffer, layout, index, output);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Copies one exact element into a live buffer.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for invalid storage, layout, bounds, or size.
    fn ffi_buffer_write(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
        index: u64,
        element: &[u8],
    ) -> Result<(), RuntimeFailure> {
        let _ = (buffer, layout, index, element);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Starts the buffer's single lexical foreign-address borrow.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for invalid or closed storage, a layout
    /// mismatch, or an already-active borrow.
    fn ffi_buffer_borrow(
        &mut self,
        buffer: ManagedReference,
        layout: FfiAbiLayoutId,
    ) -> Result<FfiBufferBorrow, RuntimeFailure> {
        let _ = (buffer, layout);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Ends the exact active lexical borrow.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for invalid storage or a stale, forged, or
    /// non-current borrow identity.
    fn ffi_buffer_end_borrow(
        &mut self,
        buffer: ManagedReference,
        borrow: FfiBufferBorrowId,
    ) -> Result<(), RuntimeFailure> {
        let _ = (buffer, borrow);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Deterministically closes a buffer, with repeated completed closes allowed.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for invalid storage or an active borrow.
    fn ffi_buffer_close(&mut self, buffer: ManagedReference) -> Result<(), RuntimeFailure> {
        let _ = buffer;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Allocates the trusted immutable packed-byte representation.
    ///
    /// This is a runtime/library construction boundary, not a general source
    /// allocation primitive.
    ///
    /// # Errors
    ///
    /// Returns a portable allocation or invariant failure.
    fn allocate_immutable_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<ManagedReference, RuntimeFailure> {
        let _ = bytes;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Pins one exact immutable byte payload and returns its packed address.
    ///
    /// # Errors
    ///
    /// Rejects non-Bytes owners and owners with an active payload borrow.
    fn ffi_bytes_borrow(
        &mut self,
        bytes: ManagedReference,
    ) -> Result<FfiBytesBorrow, RuntimeFailure> {
        let _ = bytes;
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Ends one exact immutable byte-payload borrow.
    ///
    /// # Errors
    ///
    /// Rejects stale, forged, duplicate, or wrong-owner borrow identities.
    fn ffi_bytes_end_borrow(
        &mut self,
        bytes: ManagedReference,
        borrow: FfiBytesBorrowId,
    ) -> Result<(), RuntimeFailure> {
        let _ = (bytes, borrow);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Reads exact ABI bytes through a verified foreign pointer.
    ///
    /// Adapters expose only unmanaged storage whose provenance they can prove.
    /// The default rejects arbitrary process addresses.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for unavailable, stale, or out-of-bounds
    /// storage.
    fn ffi_unsafe_read(
        &mut self,
        address: ForeignAddress,
        output: &mut [u8],
    ) -> Result<(), RuntimeFailure> {
        let _ = (address, output);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Writes exact ABI bytes through a verified mutable foreign pointer.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure for unavailable, stale, read-only, or
    /// out-of-bounds storage.
    fn ffi_unsafe_write(
        &mut self,
        address: ForeignAddress,
        bytes: &[u8],
    ) -> Result<(), RuntimeFailure> {
        let _ = (address, bytes);
        Err(RuntimeFailure::runtime_invariant())
    }

    /// Copies exact ABI bytes with `memmove` overlap semantics.
    ///
    /// # Errors
    ///
    /// Returns an invariant failure unless both complete ranges have proven
    /// live unmanaged provenance.
    fn ffi_unsafe_copy(
        &mut self,
        source: ForeignAddress,
        destination: ForeignAddress,
        byte_count: u64,
    ) -> Result<(), RuntimeFailure> {
        let _ = (source, destination, byte_count);
        Err(RuntimeFailure::runtime_invariant())
    }

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
