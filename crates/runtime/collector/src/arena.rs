//! Typed scoped-arena identifiers, layouts, values, and telemetry.

use std::collections::BTreeSet;

use pop_runtime_interface::{ManagedReference, ObjectSlot, RuntimeTypeId};

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArenaId(u64);

impl ArenaId {
    pub(crate) const fn new(raw: u64) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ArenaReference {
    arena: ArenaId,
    offset_bytes: usize,
}

impl ArenaReference {
    pub(crate) const fn new(arena: ArenaId, offset_bytes: usize) -> Self {
        Self {
            arena,
            offset_bytes,
        }
    }

    #[must_use]
    pub const fn arena(self) -> ArenaId {
        self.arena
    }

    #[must_use]
    pub const fn offset_bytes(self) -> usize {
        self.offset_bytes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArenaConfig {
    pub(crate) capacity_bytes: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaConfigError {
    ZeroCapacity,
}

impl ArenaConfig {
    /// Defines the fixed maximum bytes for one bump arena.
    ///
    /// # Errors
    ///
    /// Rejects an arena that can contain no allocation.
    pub const fn new(capacity_bytes: usize) -> Result<Self, ArenaConfigError> {
        if capacity_bytes == 0 {
            Err(ArenaConfigError::ZeroCapacity)
        } else {
            Ok(Self { capacity_bytes })
        }
    }

    #[must_use]
    pub const fn capacity_bytes(self) -> usize {
        self.capacity_bytes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArenaAllocationRequest {
    pub(crate) type_id: RuntimeTypeId,
    pub(crate) slot_count: u32,
    pub(crate) managed_slots: BTreeSet<ObjectSlot>,
    pub(crate) arena_slots: BTreeSet<ObjectSlot>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaLayoutError {
    SlotOutOfBounds,
    OverlappingReferenceKinds,
}

impl ArenaAllocationRequest {
    /// Defines disjoint scalar, managed-reference, and arena-reference slots.
    ///
    /// # Errors
    ///
    /// Rejects out-of-bounds slots and slots assigned both reference kinds.
    pub fn new(
        type_id: RuntimeTypeId,
        slot_count: u32,
        managed_slots: Vec<u32>,
        arena_slots: Vec<u32>,
    ) -> Result<Self, ArenaLayoutError> {
        let managed_slots: BTreeSet<_> = managed_slots.into_iter().map(ObjectSlot::new).collect();
        let arena_slots: BTreeSet<_> = arena_slots.into_iter().map(ObjectSlot::new).collect();
        if managed_slots
            .iter()
            .chain(&arena_slots)
            .any(|slot| slot.raw() >= slot_count)
        {
            return Err(ArenaLayoutError::SlotOutOfBounds);
        }
        if managed_slots.iter().any(|slot| arena_slots.contains(slot)) {
            return Err(ArenaLayoutError::OverlappingReferenceKinds);
        }
        Ok(Self {
            type_id,
            slot_count,
            managed_slots,
            arena_slots,
        })
    }

    #[must_use]
    pub const fn type_id(&self) -> RuntimeTypeId {
        self.type_id
    }

    #[must_use]
    pub const fn slot_count(&self) -> u32 {
        self.slot_count
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaSlotValue {
    Scalar(u64),
    ManagedReference(Option<ManagedReference>),
    ArenaReference(Option<ArenaReference>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArenaCloseStatistics {
    objects_reclaimed: u64,
    bytes_reclaimed: usize,
}

impl ArenaCloseStatistics {
    pub(crate) fn new(objects: usize, bytes: usize) -> Self {
        Self {
            objects_reclaimed: u64::try_from(objects).unwrap_or(u64::MAX),
            bytes_reclaimed: bytes,
        }
    }

    #[must_use]
    pub const fn objects_reclaimed(self) -> u64 {
        self.objects_reclaimed
    }

    #[must_use]
    pub const fn bytes_reclaimed(self) -> usize {
        self.bytes_reclaimed
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ArenaTelemetry {
    pub(crate) arenas_created: u64,
    pub(crate) arenas_closed: u64,
    pub(crate) objects_allocated: u64,
    pub(crate) objects_bulk_reclaimed: u64,
    pub(crate) live_bytes: usize,
    pub(crate) peak_bytes: usize,
}

impl ArenaTelemetry {
    #[must_use]
    pub const fn arenas_created(self) -> u64 {
        self.arenas_created
    }
    #[must_use]
    pub const fn arenas_closed(self) -> u64 {
        self.arenas_closed
    }
    #[must_use]
    pub const fn objects_allocated(self) -> u64 {
        self.objects_allocated
    }
    #[must_use]
    pub const fn objects_bulk_reclaimed(self) -> u64 {
        self.objects_bulk_reclaimed
    }
    #[must_use]
    pub const fn live_bytes(self) -> usize {
        self.live_bytes
    }
    #[must_use]
    pub const fn peak_bytes(self) -> usize {
        self.peak_bytes
    }
}
