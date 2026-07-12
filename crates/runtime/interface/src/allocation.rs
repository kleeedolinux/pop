use crate::{ObjectMap, ObjectMapError, ObjectSlot, RuntimeTypeId};

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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TableAllocationError {
    EntryCapacityOverflow(u32),
    InvalidObjectMap(ObjectMapError),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TableAllocationRequest {
    type_id: RuntimeTypeId,
    allocation_class: AllocationClass,
    entry_count: u32,
    key_map: ArrayElementMap,
    value_map: ArrayElementMap,
    object_map: ObjectMap,
}

impl TableAllocationRequest {
    /// Constructs the homogeneous interleaved key/value layout for a table.
    ///
    /// # Errors
    ///
    /// Returns an error when twice the entry capacity cannot be represented by
    /// the portable logical-slot index.
    pub fn new(
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        entry_count: u32,
        key_map: ArrayElementMap,
        value_map: ArrayElementMap,
    ) -> Result<Self, TableAllocationError> {
        let slot_count = entry_count
            .checked_mul(2)
            .ok_or(TableAllocationError::EntryCapacityOverflow(entry_count))?;
        let reference_capacity = usize::try_from(entry_count)
            .unwrap_or(usize::MAX)
            .saturating_mul(usize::from(key_map == ArrayElementMap::ManagedReference))
            .saturating_add(
                usize::try_from(entry_count)
                    .unwrap_or(usize::MAX)
                    .saturating_mul(usize::from(value_map == ArrayElementMap::ManagedReference)),
            );
        let mut reference_slots = Vec::with_capacity(reference_capacity);
        for entry in 0..entry_count {
            let key_slot = entry * 2;
            if key_map == ArrayElementMap::ManagedReference {
                reference_slots.push(ObjectSlot::new(key_slot));
            }
            if value_map == ArrayElementMap::ManagedReference {
                reference_slots.push(ObjectSlot::new(key_slot + 1));
            }
        }
        let object_map = ObjectMap::new(slot_count, reference_slots)
            .map_err(TableAllocationError::InvalidObjectMap)?;
        Ok(Self {
            type_id,
            allocation_class,
            entry_count,
            key_map,
            value_map,
            object_map,
        })
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
    pub const fn entry_count(&self) -> u32 {
        self.entry_count
    }

    #[must_use]
    pub const fn key_map(&self) -> ArrayElementMap {
        self.key_map
    }

    #[must_use]
    pub const fn value_map(&self) -> ArrayElementMap {
        self.value_map
    }

    #[must_use]
    pub const fn object_map(&self) -> &ObjectMap {
        &self.object_map
    }
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
