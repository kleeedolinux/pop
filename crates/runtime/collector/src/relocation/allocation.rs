//! Fallible typed payload construction before relocation-heap publication.

use pop_runtime_interface::{
    AllocationClass, ArrayElementMap, ManagedReference, ObjectMap, ObjectSlot, RuntimeFailure,
    RuntimeTypeId,
};

use crate::heap::{Allocation, AllocationKind, SlotStorage, SlotValue};
use crate::ownership::ObjectOwnership;

use super::heap::{
    CollectorGeneration, CollectorObjectId, RelocationAllocation, RelocationRuntime,
};

impl RelocationRuntime {
    pub(crate) fn allocate_object_initialized(
        &mut self,
        request: &pop_runtime_interface::ObjectAllocationRequest,
        values: &[u64],
    ) -> Result<ManagedReference, RuntimeFailure> {
        let object_map = request.object_map();
        if values.len() != object_map.slot_count() as usize {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let mut slots = SlotStorage::new();
        slots
            .try_reserve_exact(values.len())
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        for (index, value) in values.iter().copied().enumerate() {
            let slot = ObjectSlot::new(
                u32::try_from(index).map_err(|_| RuntimeFailure::runtime_invariant())?,
            );
            if object_map.is_reference_slot(slot) {
                let reference = (value != 0).then(|| ManagedReference::new(value));
                if let Some(reference) = reference {
                    self.validate_reference(reference)?;
                }
                slots.push(SlotValue::reference(reference));
            } else {
                slots.push(SlotValue::scalar(value));
            }
        }
        self.allocate_initialized(
            request.type_id(),
            request.allocation_class(),
            AllocationKind::Object,
            object_map.clone(),
            slots,
        )
    }

    pub(super) fn allocate(
        &mut self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        kind: AllocationKind,
        object_map: ObjectMap,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let mut slots = SlotStorage::new();
        slots
            .try_reserve_exact(object_map.slot_count() as usize)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        for _ in 0..object_map.slot_count() {
            slots.push(SlotValue::scalar(0));
        }
        self.allocate_initialized(type_id, class, kind, object_map, slots)
    }

    pub(crate) fn allocate_array_filled(
        &mut self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        element_map: ArrayElementMap,
        object_map: ObjectMap,
        value: u64,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let length = usize::try_from(object_map.slot_count())
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(length)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        match element_map {
            ArrayElementMap::Scalar => {
                if !object_map.reference_slots().is_empty() {
                    return Err(RuntimeFailure::runtime_invariant());
                }
                slots.resize(length, SlotValue::scalar(value));
            }
            ArrayElementMap::ManagedReference => {
                if object_map.reference_slots().len() != length {
                    return Err(RuntimeFailure::runtime_invariant());
                }
                let reference = (value != 0).then(|| ManagedReference::new(value));
                if let Some(reference) = reference {
                    self.validate_reference(reference)?;
                }
                slots.resize(length, SlotValue::reference(reference));
            }
        }
        self.allocate_initialized(
            type_id,
            class,
            AllocationKind::Array(element_map),
            object_map,
            slots.into(),
        )
    }

    fn allocate_initialized(
        &mut self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        kind: AllocationKind,
        object_map: ObjectMap,
        slots: SlotStorage,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference = self.fresh_reference()?;
        let identity = CollectorObjectId(self.next_identity);
        self.next_identity = self
            .next_identity
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let generation = match class {
            AllocationClass::NurseryEligible => CollectorGeneration::Nursery { age: 0 },
            AllocationClass::Mature | AllocationClass::Large | AllocationClass::Pinned => {
                CollectorGeneration::Mature
            }
        };
        self.objects.insert(
            reference,
            RelocationAllocation {
                identity,
                generation,
                allocation: Allocation {
                    kind,
                    type_id,
                    class,
                    object_map,
                    slots,
                    immutable_bytes: None,
                },
                ownership: ObjectOwnership::default(),
                mutability: crate::ObjectMutability::Mutable,
            },
        );
        self.metrics.record_allocation();
        Ok(reference)
    }
}
