use crate::{ManagedReference, ObjectSlot};

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
