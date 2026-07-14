//! Fallible typed payload construction before relocation-heap publication.

use pop_runtime_interface::{
    AllocationClass, ArrayElementMap, ManagedReference, ObjectMap, ObjectSlot, RuntimeFailure,
    RuntimeTypeId,
};

use crate::heap::{Allocation, AllocationKind, SlotValue};
use crate::ownership::ObjectOwnership;

use super::heap::{
    CollectorGeneration, CollectorObjectId, RelocationAllocation, RelocationRuntime,
};

impl RelocationRuntime {
    pub(super) fn allocate(
        &mut self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        kind: AllocationKind,
        object_map: ObjectMap,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(object_map.slot_count() as usize)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        for index in 0..object_map.slot_count() {
            slots.push(if object_map.is_reference_slot(ObjectSlot::new(index)) {
                SlotValue::Reference(None)
            } else {
                SlotValue::Scalar(0)
            });
        }
        self.allocate_initialized(type_id, class, kind, object_map, slots)
    }

    pub(crate) fn allocate_scalar_array_filled(
        &mut self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        object_map: ObjectMap,
        value: u64,
    ) -> Result<ManagedReference, RuntimeFailure> {
        if !object_map.reference_slots().is_empty() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let length = usize::try_from(object_map.slot_count())
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(length)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        slots.resize(length, SlotValue::Scalar(value));
        self.allocate_initialized(
            type_id,
            class,
            AllocationKind::Array(ArrayElementMap::Scalar),
            object_map,
            slots,
        )
    }

    fn allocate_initialized(
        &mut self,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        kind: AllocationKind,
        object_map: ObjectMap,
        slots: Vec<SlotValue>,
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
                },
                ownership: ObjectOwnership::default(),
            },
        );
        self.metrics.record_allocation();
        Ok(reference)
    }
}
