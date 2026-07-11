//! Versioned backend-neutral Pop Lang Runtime Interface contracts.

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct PlriVersion {
    major: u16,
    minor: u16,
}

impl PlriVersion {
    #[must_use]
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    #[must_use]
    pub const fn major(self) -> u16 {
        self.major
    }

    #[must_use]
    pub const fn minor(self) -> u16 {
        self.minor
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GarbageCollectorContract {
    stage: GarbageCollectorStage,
    roots: RootPrecision,
    nursery: NurseryMobility,
    mature_heap: MatureHeapCollection,
    barriers: BarrierContract,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GarbageCollectorStage {
    BootstrapPreciseStopTheWorld,
    ProductionConcurrentGenerational,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RootPrecision {
    Precise,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum NurseryMobility {
    Absent,
    Moving,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MatureHeapCollection {
    StopTheWorldMarkSweep,
    MostlyNonMovingConcurrent,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BarrierContract {
    None,
    SatbAndGenerationalCard,
}

impl GarbageCollectorContract {
    #[must_use]
    pub const fn pop_v1() -> Self {
        Self {
            stage: GarbageCollectorStage::ProductionConcurrentGenerational,
            roots: RootPrecision::Precise,
            nursery: NurseryMobility::Moving,
            mature_heap: MatureHeapCollection::MostlyNonMovingConcurrent,
            barriers: BarrierContract::SatbAndGenerationalCard,
        }
    }

    #[must_use]
    pub const fn bootstrap_stage1() -> Self {
        Self {
            stage: GarbageCollectorStage::BootstrapPreciseStopTheWorld,
            roots: RootPrecision::Precise,
            nursery: NurseryMobility::Absent,
            mature_heap: MatureHeapCollection::StopTheWorldMarkSweep,
            barriers: BarrierContract::None,
        }
    }

    #[must_use]
    pub const fn stage(self) -> GarbageCollectorStage {
        self.stage
    }

    #[must_use]
    pub const fn precise_roots(self) -> bool {
        matches!(self.roots, RootPrecision::Precise)
    }

    #[must_use]
    pub const fn moving_nursery(self) -> bool {
        matches!(self.nursery, NurseryMobility::Moving)
    }

    #[must_use]
    pub const fn mostly_non_moving_mature_heap(self) -> bool {
        matches!(
            self.mature_heap,
            MatureHeapCollection::MostlyNonMovingConcurrent
        )
    }

    #[must_use]
    pub const fn concurrent_mature_marking(self) -> bool {
        matches!(
            self.mature_heap,
            MatureHeapCollection::MostlyNonMovingConcurrent
        )
    }

    #[must_use]
    pub const fn satb_barrier(self) -> bool {
        matches!(self.barriers, BarrierContract::SatbAndGenerationalCard)
    }

    #[must_use]
    pub const fn generational_card_barrier(self) -> bool {
        matches!(self.barriers, BarrierContract::SatbAndGenerationalCard)
    }

    #[must_use]
    pub const fn user_finalizers(self) -> bool {
        false
    }

    #[must_use]
    pub const fn weak_references(self) -> bool {
        false
    }

    #[must_use]
    pub const fn conservative_scanning(self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorContract {
    typed_results: bool,
    panics_unwind: bool,
    exceptions_are_ordinary_errors: bool,
}

impl ErrorContract {
    #[must_use]
    pub const fn pop_v1() -> Self {
        Self {
            typed_results: true,
            panics_unwind: true,
            exceptions_are_ordinary_errors: false,
        }
    }

    #[must_use]
    pub const fn uses_typed_results(self) -> bool {
        self.typed_results
    }

    #[must_use]
    pub const fn panics_unwind(self) -> bool {
        self.panics_unwind
    }

    #[must_use]
    pub const fn exceptions_are_ordinary_errors(self) -> bool {
        self.exceptions_are_ordinary_errors
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InitializationState {
    Unloaded,
    Loading,
    Loaded,
    Initializing,
    Ready,
    Failed,
}

impl InitializationState {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Unloaded, Self::Loading)
                | (Self::Loading, Self::Loaded | Self::Failed)
                | (Self::Loaded, Self::Initializing)
                | (Self::Initializing, Self::Ready | Self::Failed)
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum RuntimeOperation {
    AllocateObject,
    AllocateArray,
    AllocateTable,
    TupleMake,
    ArrayGet,
    ArraySet,
    FieldGet,
    FieldSet,
    RecordUpdate,
    UnionMake,
    CaptureLoad,
    CaptureStore,
    DispatchCall,
    RetainRoot,
    ReleaseRoot,
    PublishRoots,
    GcSafePoint,
    SatbWriteBarrier,
    GenerationalWriteBarrier,
    Trap,
    Panic,
    ContinueUnwind,
    Suspend,
    Resume,
    InitializeModule,
    InitializeBubble,
}

impl RuntimeOperation {
    /// Stable C-ABI symbol selected by a native backend. MIR carries the
    /// operation, never this spelling; other backends may dispatch directly.
    #[must_use]
    pub const fn abi_symbol(self) -> &'static str {
        match self {
            Self::AllocateObject => "pop_rt_allocate_object",
            Self::AllocateArray => "pop_rt_allocate_array",
            Self::AllocateTable => "pop_rt_allocate_table",
            Self::TupleMake => "pop_rt_tuple_make",
            Self::ArrayGet => "pop_rt_array_get",
            Self::ArraySet => "pop_rt_array_set",
            Self::FieldGet => "pop_rt_field_get",
            Self::FieldSet => "pop_rt_field_set",
            Self::RecordUpdate => "pop_rt_record_update",
            Self::UnionMake => "pop_rt_union_make",
            Self::CaptureLoad => "pop_rt_capture_load",
            Self::CaptureStore => "pop_rt_capture_store",
            Self::DispatchCall => "pop_rt_dispatch_call",
            Self::RetainRoot => "pop_rt_retain_root",
            Self::ReleaseRoot => "pop_rt_release_root",
            Self::PublishRoots => "pop_rt_publish_roots",
            Self::GcSafePoint => "pop_rt_gc_safe_point",
            Self::SatbWriteBarrier => "pop_rt_satb_write_barrier",
            Self::GenerationalWriteBarrier => "pop_rt_generational_write_barrier",
            Self::Trap => "pop_rt_trap",
            Self::Panic => "pop_rt_panic",
            Self::ContinueUnwind => "pop_rt_continue_unwind",
            Self::Suspend => "pop_rt_suspend",
            Self::Resume => "pop_rt_resume",
            Self::InitializeModule => "pop_rt_initialize_module",
            Self::InitializeBubble => "pop_rt_initialize_bubble",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeTypeId(u32);

impl RuntimeTypeId {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ManagedReference(u64);

impl ManagedReference {
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RootHandle(u64);

impl RootHandle {
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ObjectSlot(u32);

impl ObjectSlot {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RootSlot(u32);

impl RootSlot {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct SafePointId(u32);

impl SafePointId {
    #[must_use]
    pub const fn new(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectMapError {
    SlotOutOfBounds { slot: ObjectSlot, slot_count: u32 },
    DuplicateReferenceSlot(ObjectSlot),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMap {
    slot_count: u32,
    reference_slots: Vec<ObjectSlot>,
}

impl ObjectMap {
    /// Constructs a canonical logical object pointer map.
    ///
    /// # Errors
    ///
    /// Returns an error when a reference slot is duplicated or outside the
    /// declared logical slot range.
    pub fn new(
        slot_count: u32,
        mut reference_slots: Vec<ObjectSlot>,
    ) -> Result<Self, ObjectMapError> {
        reference_slots.sort_unstable();
        for pair in reference_slots.windows(2) {
            if pair[0] == pair[1] {
                return Err(ObjectMapError::DuplicateReferenceSlot(pair[0]));
            }
        }
        if let Some(slot) = reference_slots
            .iter()
            .copied()
            .find(|slot| slot.raw() >= slot_count)
        {
            return Err(ObjectMapError::SlotOutOfBounds { slot, slot_count });
        }
        Ok(Self {
            slot_count,
            reference_slots,
        })
    }

    #[must_use]
    pub const fn slot_count(&self) -> u32 {
        self.slot_count
    }

    #[must_use]
    pub fn reference_slots(&self) -> &[ObjectSlot] {
        &self.reference_slots
    }

    #[must_use]
    pub fn is_reference_slot(&self, slot: ObjectSlot) -> bool {
        self.reference_slots.binary_search(&slot).is_ok()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RootMapError {
    DuplicateRootSlot(RootSlot),
    ValueCount { expected: usize, found: usize },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StackMap {
    safe_point: SafePointId,
    root_slots: Vec<RootSlot>,
}

impl StackMap {
    /// Constructs a canonical logical stack map for one safe point.
    ///
    /// # Errors
    ///
    /// Returns an error when a logical root slot occurs more than once.
    pub fn new(
        safe_point: SafePointId,
        mut root_slots: Vec<RootSlot>,
    ) -> Result<Self, RootMapError> {
        root_slots.sort_unstable();
        for pair in root_slots.windows(2) {
            if pair[0] == pair[1] {
                return Err(RootMapError::DuplicateRootSlot(pair[0]));
            }
        }
        Ok(Self {
            safe_point,
            root_slots,
        })
    }

    #[must_use]
    pub const fn safe_point(&self) -> SafePointId {
        self.safe_point
    }

    #[must_use]
    pub fn root_slots(&self) -> &[RootSlot] {
        &self.root_slots
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RootPublication {
    stack_map: StackMap,
    values: Vec<Option<ManagedReference>>,
}

impl RootPublication {
    /// Associates the live managed values with the canonical root slots in a
    /// stack map.
    ///
    /// # Errors
    ///
    /// Returns an error when the number of published values differs from the
    /// number of logical root slots.
    pub fn new(
        stack_map: StackMap,
        values: Vec<Option<ManagedReference>>,
    ) -> Result<Self, RootMapError> {
        if stack_map.root_slots.len() != values.len() {
            return Err(RootMapError::ValueCount {
                expected: stack_map.root_slots.len(),
                found: values.len(),
            });
        }
        Ok(Self { stack_map, values })
    }

    #[must_use]
    pub const fn stack_map(&self) -> &StackMap {
        &self.stack_map
    }

    pub fn managed_references(&self) -> impl Iterator<Item = ManagedReference> + '_ {
        self.values.iter().flatten().copied()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationClass {
    NurseryEligible,
    Mature,
    Large,
    Pinned,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectAllocationRequest {
    type_id: RuntimeTypeId,
    allocation_class: AllocationClass,
    object_map: ObjectMap,
}

impl ObjectAllocationRequest {
    #[must_use]
    pub const fn new(
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        object_map: ObjectMap,
    ) -> Self {
        Self {
            type_id,
            allocation_class,
            object_map,
        }
    }

    #[must_use]
    pub const fn type_id(&self) -> RuntimeTypeId {
        self.type_id
    }

    #[must_use]
    pub const fn allocation_class(&self) -> AllocationClass {
        self.allocation_class
    }

    #[must_use]
    pub const fn object_map(&self) -> &ObjectMap {
        &self.object_map
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArrayElementMap {
    Scalar,
    ManagedReference,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArrayAllocationRequest {
    type_id: RuntimeTypeId,
    allocation_class: AllocationClass,
    length: u32,
    element_map: ArrayElementMap,
}

impl ArrayAllocationRequest {
    #[must_use]
    pub const fn new(
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        length: u32,
        element_map: ArrayElementMap,
    ) -> Self {
        Self {
            type_id,
            allocation_class,
            length,
            element_map,
        }
    }

    #[must_use]
    pub const fn type_id(&self) -> RuntimeTypeId {
        self.type_id
    }

    #[must_use]
    pub const fn allocation_class(&self) -> AllocationClass {
        self.allocation_class
    }

    #[must_use]
    pub const fn length(&self) -> u32 {
        self.length
    }

    #[must_use]
    pub const fn element_map(&self) -> ArrayElementMap {
        self.element_map
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierKind {
    Satb,
    GenerationalCard,
    CombinedSatbGenerational,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WriteBarrier {
    kind: BarrierKind,
    owner: ManagedReference,
    slot: ObjectSlot,
    previous: Option<ManagedReference>,
    value: Option<ManagedReference>,
}

impl WriteBarrier {
    #[must_use]
    pub const fn new(
        kind: BarrierKind,
        owner: ManagedReference,
        slot: ObjectSlot,
        previous: Option<ManagedReference>,
        value: Option<ManagedReference>,
    ) -> Self {
        Self {
            kind,
            owner,
            slot,
            previous,
            value,
        }
    }

    #[must_use]
    pub const fn kind(self) -> BarrierKind {
        self.kind
    }

    #[must_use]
    pub const fn owner(self) -> ManagedReference {
        self.owner
    }

    #[must_use]
    pub const fn slot(self) -> ObjectSlot {
        self.slot
    }

    #[must_use]
    pub const fn previous(self) -> Option<ManagedReference> {
        self.previous
    }

    #[must_use]
    pub const fn value(self) -> Option<ManagedReference> {
        self.value
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapKind {
    IntegerOverflow,
    DivisionByZero,
    BoundsViolation,
    ImpossibleState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Trap {
    kind: TrapKind,
}

impl Trap {
    #[must_use]
    pub const fn new(kind: TrapKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn kind(self) -> TrapKind {
        self.kind
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanicKind {
    RuntimeInvariant,
    OutOfMemory {
        requested_objects: u64,
        requested_slots: u64,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PanicPayload {
    kind: PanicKind,
}

impl PanicPayload {
    #[must_use]
    pub const fn new(kind: PanicKind) -> Self {
        Self { kind }
    }

    #[must_use]
    pub const fn out_of_memory(requested_objects: u64, requested_slots: u64) -> Self {
        Self::new(PanicKind::OutOfMemory {
            requested_objects,
            requested_slots,
        })
    }

    #[must_use]
    pub const fn kind(&self) -> PanicKind {
        self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UnwindReason {
    Panic(PanicPayload),
    Cancellation,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeFailure {
    Trap(Trap),
    Unwind(UnwindReason),
}

impl RuntimeFailure {
    #[must_use]
    pub fn from_panic(payload: PanicPayload) -> Self {
        Self::Unwind(UnwindReason::Panic(payload))
    }

    #[must_use]
    pub fn runtime_invariant() -> Self {
        Self::from_panic(PanicPayload::new(PanicKind::RuntimeInvariant))
    }
}

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

    /// Publishes precise stack roots and services a requested collection.
    ///
    /// # Errors
    ///
    /// Returns an invariant panic when a published reference is invalid.
    fn safe_point(&mut self, roots: &RootPublication) -> Result<SafePointOutcome, RuntimeFailure>;

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
