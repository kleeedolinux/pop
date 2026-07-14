//! Public epoch, mutator-state, publication, error, and telemetry vocabulary.

use pop_runtime_interface::{RootPublication, SafePointId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MutatorId(pub(super) u32);

impl MutatorId {
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CollectorEpoch(pub(super) u64);

impl CollectorEpoch {
    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CollectorPhase {
    Marking,
    MarkCompletion,
    Sweeping,
    Evacuation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MutatorExecutionState {
    Managed,
    Detached,
    HandlesOnly,
    BoundedForeign,
}

impl MutatorExecutionState {
    pub(super) const fn acknowledges_without_poll(self) -> bool {
        matches!(self, Self::Detached | Self::HandlesOnly)
    }

    pub(super) const fn can_publish(self) -> bool {
        matches!(self, Self::Managed)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MutatorPublication {
    safe_point: SafePointId,
    root_slots: usize,
    managed_roots: usize,
    tlab_top_bytes: usize,
    satb_entries: usize,
    dirty_cards: usize,
}

impl MutatorPublication {
    #[must_use]
    pub fn new(
        roots: &RootPublication,
        tlab_top_bytes: usize,
        satb_entries: usize,
        dirty_cards: usize,
    ) -> Self {
        Self {
            safe_point: roots.stack_map().safe_point(),
            root_slots: roots.stack_map().root_slots().len(),
            managed_roots: roots.managed_references().count(),
            tlab_top_bytes,
            satb_entries,
            dirty_cards,
        }
    }

    #[must_use]
    pub const fn safe_point(self) -> SafePointId {
        self.safe_point
    }

    #[must_use]
    pub const fn root_slots(self) -> usize {
        self.root_slots
    }

    #[must_use]
    pub const fn managed_roots(self) -> usize {
        self.managed_roots
    }

    #[must_use]
    pub const fn tlab_top_bytes(self) -> usize {
        self.tlab_top_bytes
    }

    #[must_use]
    pub const fn satb_entries(self) -> usize {
        self.satb_entries
    }

    #[must_use]
    pub const fn dirty_cards(self) -> usize {
        self.dirty_cards
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpochCoordinatorConfig {
    pub(super) maximum_mutators: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpochCoordinatorConfigError {
    ZeroMutators,
}

impl EpochCoordinatorConfig {
    /// Defines the fixed mutator-registration capacity.
    ///
    /// # Errors
    ///
    /// Rejects a coordinator that could register no mutator.
    pub const fn new(maximum_mutators: usize) -> Result<Self, EpochCoordinatorConfigError> {
        if maximum_mutators == 0 {
            Err(EpochCoordinatorConfigError::ZeroMutators)
        } else {
            Ok(Self { maximum_mutators })
        }
    }

    #[must_use]
    pub const fn maximum_mutators(self) -> usize {
        self.maximum_mutators
    }
}

impl Default for EpochCoordinatorConfig {
    fn default() -> Self {
        Self::new(1024).expect("default epoch coordinator capacity is nonzero")
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EpochCoordinatorError {
    MutatorCapacity,
    UnknownMutator(MutatorId),
    EpochAlreadyActive(CollectorEpoch),
    NoActiveEpoch,
    StaleEpoch {
        expected: CollectorEpoch,
        found: CollectorEpoch,
    },
    AlreadyAcknowledged(MutatorId),
    MutatorCannotAcknowledge(MutatorId),
    AcknowledgementsPending(usize),
    EpochOverflow,
    MutatorIdentityOverflow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EpochProgress {
    pub(super) pending: usize,
}

impl EpochProgress {
    #[must_use]
    pub const fn pending(self) -> usize {
        self.pending
    }

    #[must_use]
    pub const fn complete(self) -> bool {
        self.pending == 0
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EpochCoordinatorTelemetry {
    pub(super) epochs_requested: u64,
    pub(super) epochs_completed: u64,
    pub(super) acknowledgements: u64,
    pub(super) automatic_acknowledgements: u64,
    pub(super) maximum_pending_acknowledgements: usize,
    pub(super) stale_epoch_polls: u64,
    pub(super) blocked_foreign_polls: u64,
    pub(super) mutators_registered: u64,
    pub(super) mutators_unregistered: u64,
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

impl EpochCoordinatorTelemetry {
    telemetry_accessors! {
        epochs_requested: u64,
        epochs_completed: u64,
        acknowledgements: u64,
        automatic_acknowledgements: u64,
        maximum_pending_acknowledgements: usize,
        stale_epoch_polls: u64,
        blocked_foreign_polls: u64,
        mutators_registered: u64,
        mutators_unregistered: u64,
    }
}
