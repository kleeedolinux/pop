use crate::{ManagedReference, ObjectSlot, RootSlot, SafePointId};

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

    /// Iterates over canonical root slots and their current managed values.
    pub fn root_values(&self) -> impl Iterator<Item = (RootSlot, Option<ManagedReference>)> + '_ {
        self.stack_map
            .root_slots
            .iter()
            .copied()
            .zip(self.values.iter().copied())
    }

    /// Iterates over canonical root slots and mutable managed values.
    ///
    /// The stack map and publication length remain immutable so a collector
    /// can replace relocated tokens without changing root identity.
    pub fn root_values_mut(
        &mut self,
    ) -> impl Iterator<Item = (RootSlot, &mut Option<ManagedReference>)> + '_ {
        self.stack_map
            .root_slots
            .iter()
            .copied()
            .zip(self.values.iter_mut())
    }

    pub fn managed_references(&self) -> impl Iterator<Item = ManagedReference> + '_ {
        self.values.iter().flatten().copied()
    }
}
