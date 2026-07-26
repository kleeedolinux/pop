use std::sync::Arc;

use crate::{ObjectMap, ObjectMapError, ObjectSlot, RuntimeTypeId};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AllocationClass {
    NurseryEligible,
    Mature,
    Large,
    Pinned,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RuntimeAllocationSiteId {
    bubble: u32,
    owner: u32,
    local: u32,
}

impl RuntimeAllocationSiteId {
    #[must_use]
    pub const fn new(bubble: u32, owner: u32, local: u32) -> Self {
        Self {
            bubble,
            owner,
            local,
        }
    }

    #[must_use]
    pub const fn bubble(self) -> u32 {
        self.bubble
    }

    #[must_use]
    pub const fn owner(self) -> u32 {
        self.owner
    }

    #[must_use]
    pub const fn local(self) -> u32 {
        self.local
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AllocationSiteDescriptor {
    site: RuntimeAllocationSiteId,
    type_id: RuntimeTypeId,
    allocation_class: AllocationClass,
    object_map: Arc<ObjectMap>,
}

impl AllocationSiteDescriptor {
    #[must_use]
    pub fn new(
        site: RuntimeAllocationSiteId,
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        object_map: ObjectMap,
    ) -> Self {
        Self {
            site,
            type_id,
            allocation_class,
            object_map: Arc::new(object_map),
        }
    }

    #[must_use]
    pub const fn site(&self) -> RuntimeAllocationSiteId {
        self.site
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
    pub fn object_map(&self) -> &ObjectMap {
        &self.object_map
    }

    #[must_use]
    pub fn shared_object_map(&self) -> Arc<ObjectMap> {
        self.object_map.clone()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObjectAllocationRequest {
    site: Option<RuntimeAllocationSiteId>,
    type_id: RuntimeTypeId,
    allocation_class: AllocationClass,
    object_map: Arc<ObjectMap>,
}

impl ObjectAllocationRequest {
    #[must_use]
    pub fn new(
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        object_map: ObjectMap,
    ) -> Self {
        Self {
            site: None,
            type_id,
            allocation_class,
            object_map: Arc::new(object_map),
        }
    }

    #[must_use]
    pub fn from_descriptor(descriptor: &AllocationSiteDescriptor) -> Self {
        Self {
            site: Some(descriptor.site),
            type_id: descriptor.type_id,
            allocation_class: descriptor.allocation_class,
            object_map: descriptor.object_map.clone(),
        }
    }

    #[must_use]
    pub const fn allocation_site(&self) -> Option<RuntimeAllocationSiteId> {
        self.site
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
    pub fn object_map(&self) -> &ObjectMap {
        &self.object_map
    }

    #[must_use]
    pub fn shared_object_map(&self) -> Arc<ObjectMap> {
        self.object_map.clone()
    }

    #[must_use]
    pub fn with_allocation_class(&self, allocation_class: AllocationClass) -> Self {
        Self {
            site: self.site,
            type_id: self.type_id,
            allocation_class,
            object_map: self.object_map.clone(),
        }
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
    /// Returns an error when twice the entry capacity or its precise
    /// interleaved reference layout cannot be represented.
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
        let object_map = if entry_count == 0 {
            ObjectMap::scalar(slot_count)
        } else {
            match (key_map, value_map) {
                (ArrayElementMap::Scalar, ArrayElementMap::Scalar) => ObjectMap::scalar(slot_count),
                (ArrayElementMap::ManagedReference, ArrayElementMap::ManagedReference) => {
                    ObjectMap::homogeneous_references(slot_count)
                }
                (ArrayElementMap::ManagedReference, ArrayElementMap::Scalar) => {
                    ObjectMap::strided_references(slot_count, 0, 2).ok_or(
                        TableAllocationError::InvalidObjectMap(ObjectMapError::SlotOutOfBounds {
                            slot: ObjectSlot::new(0),
                            slot_count,
                        }),
                    )?
                }
                (ArrayElementMap::Scalar, ArrayElementMap::ManagedReference) => {
                    ObjectMap::strided_references(slot_count, 1, 2).ok_or(
                        TableAllocationError::InvalidObjectMap(ObjectMapError::SlotOutOfBounds {
                            slot: ObjectSlot::new(1),
                            slot_count,
                        }),
                    )?
                }
            }
        };
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
