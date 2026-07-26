use crate::{ManagedReference, ObjectSlot, RootSlot, SafePointId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ObjectMapError {
    SlotOutOfBounds { slot: ObjectSlot, slot_count: u32 },
    DuplicateReferenceSlot(ObjectSlot),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectMap {
    slot_count: u32,
    reference_pattern: ReferencePattern,
    reference_slots: Vec<ObjectSlot>,
    reference_membership: Vec<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReferencePattern {
    Sparse,
    Homogeneous,
    Strided { first: u32, stride: u32 },
}

impl ObjectMap {
    /// Constructs the canonical map for a pointer-free logical payload.
    #[must_use]
    pub const fn scalar(slot_count: u32) -> Self {
        Self {
            slot_count,
            reference_pattern: ReferencePattern::Sparse,
            reference_slots: Vec::new(),
            reference_membership: Vec::new(),
        }
    }

    /// Constructs the canonical constant-size description for a homogeneous
    /// managed-reference payload.
    #[must_use]
    pub const fn homogeneous_references(slot_count: u32) -> Self {
        Self {
            slot_count,
            reference_pattern: ReferencePattern::Homogeneous,
            reference_slots: Vec::new(),
            reference_membership: Vec::new(),
        }
    }

    /// Constructs a constant-size map for one repeated slot position.
    #[must_use]
    pub const fn strided_references(slot_count: u32, first: u32, stride: u32) -> Option<Self> {
        if stride == 0 || first >= slot_count {
            return None;
        }
        Some(Self {
            slot_count,
            reference_pattern: ReferencePattern::Strided { first, stride },
            reference_slots: Vec::new(),
            reference_membership: Vec::new(),
        })
    }

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
        let membership_words = reference_slots
            .last()
            .map_or(0, |slot| slot.raw() as usize / u64::BITS as usize + 1);
        let mut reference_membership = vec![0; membership_words];
        for slot in &reference_slots {
            let index = slot.raw() as usize;
            reference_membership[index / u64::BITS as usize] |=
                1_u64 << (index % u64::BITS as usize);
        }
        Ok(Self {
            slot_count,
            reference_pattern: ReferencePattern::Sparse,
            reference_slots,
            reference_membership,
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
    pub fn has_reference_slots(&self) -> bool {
        !matches!(self.reference_pattern, ReferencePattern::Sparse)
            || !self.reference_slots.is_empty()
    }

    #[must_use]
    pub fn reference_slot_count(&self) -> usize {
        match self.reference_pattern {
            ReferencePattern::Sparse => self.reference_slots.len(),
            ReferencePattern::Homogeneous => self.slot_count as usize,
            ReferencePattern::Strided { first, stride } => {
                ((self.slot_count - 1 - first) / stride + 1) as usize
            }
        }
    }

    pub fn iter_reference_slots(&self) -> impl Iterator<Item = ObjectSlot> + '_ {
        let pattern = self.reference_pattern;
        (0..self.slot_count)
            .filter(move |slot| match pattern {
                ReferencePattern::Sparse => false,
                ReferencePattern::Homogeneous => true,
                ReferencePattern::Strided { first, stride } => {
                    *slot >= first && (*slot - first).is_multiple_of(stride)
                }
            })
            .map(ObjectSlot::new)
            .chain(self.reference_slots.iter().copied())
    }

    #[must_use]
    pub fn is_reference_slot(&self, slot: ObjectSlot) -> bool {
        match self.reference_pattern {
            ReferencePattern::Homogeneous => return slot.raw() < self.slot_count,
            ReferencePattern::Strided { first, stride } => {
                return slot.raw() < self.slot_count
                    && slot.raw() >= first
                    && (slot.raw() - first).is_multiple_of(stride);
            }
            ReferencePattern::Sparse => {}
        }
        let index = slot.raw() as usize;
        self.reference_membership
            .get(index / u64::BITS as usize)
            .is_some_and(|word| word & (1_u64 << (index % u64::BITS as usize)) != 0)
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
