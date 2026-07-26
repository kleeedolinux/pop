//! Fallible typed payload construction before relocation-heap publication.

use std::sync::Arc;

use pop_runtime_interface::{
    AllocationClass, ArrayElementMap, ManagedReference, ObjectMap, RuntimeFailure, RuntimeTypeId,
};

use crate::heap::{Allocation, AllocationKind, SlotStorage, SlotValue};
use crate::ownership::ObjectOwnership;

use super::heap::{
    CollectorGeneration, CollectorObjectId, RelocationAllocation, RelocationRuntime,
};

impl RelocationRuntime {
    pub(crate) fn reserve_mature_identities(
        &mut self,
        count: usize,
    ) -> Result<crate::generational::ReservedMatureIdentity, RuntimeFailure> {
        let count = u64::try_from(count).map_err(|_| RuntimeFailure::runtime_invariant())?;
        if count == 0 {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let next_reference = self
            .next_reference
            .checked_add(count)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if next_reference > self.reference_limit {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let next_identity = self
            .next_identity
            .checked_add(count)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let first_reference = self.next_reference;
        let first_identity = self.next_identity;
        self.next_reference = next_reference;
        self.next_identity = next_identity;
        Ok(crate::generational::ReservedMatureIdentity::new(
            ManagedReference::new(first_reference),
            CollectorObjectId(first_identity),
            usize::try_from(count).map_err(|_| RuntimeFailure::runtime_invariant())?,
        ))
    }

    pub(crate) fn reserve_identity(&mut self) -> Result<CollectorObjectId, RuntimeFailure> {
        let identity = CollectorObjectId(self.next_identity);
        self.next_identity = self
            .next_identity
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        Ok(identity)
    }

    pub(crate) fn publish_reserved_mature(
        &mut self,
        pending: crate::generational::PendingMatureObject,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let (reference, allocation) = pending.into_relocation_allocation();
        self.objects
            .insert_reserved(reference, allocation)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        self.metrics.record_allocation();
        Ok(reference)
    }

    pub(crate) fn publish_reserved_mature_batch<I>(
        &mut self,
        pending: I,
    ) -> Result<(), RuntimeFailure>
    where
        I: Iterator<Item = crate::generational::PendingMatureObject> + ExactSizeIterator,
    {
        let count = pending.len();
        self.objects
            .insert_reserved_batch(
                pending.map(crate::generational::PendingMatureObject::into_relocation_allocation),
            )
            .map_err(|()| RuntimeFailure::runtime_invariant())?;
        self.metrics.record_allocations(count);
        Ok(())
    }

    pub(crate) fn publish_object_initialized_on_page(
        &mut self,
        reference: ManagedReference,
        request: &pop_runtime_interface::ObjectAllocationRequest,
        values: &[u64],
        page_object_map: Arc<ObjectMap>,
        words: crate::heap::PageWords,
        start: usize,
    ) -> Result<(), RuntimeFailure> {
        self.validate_object_initializer(request.object_map(), values)?;
        if request.object_map() != page_object_map.as_ref() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let slots = SlotStorage::from_page_values(words, start, values)
            .map_err(|()| RuntimeFailure::runtime_invariant())?;
        self.publish_initialized(
            reference,
            request.allocation_site(),
            request.type_id(),
            request.allocation_class(),
            AllocationKind::Object,
            page_object_map,
            slots,
        )
    }

    pub(super) fn allocate(
        &mut self,
        site: Option<pop_runtime_interface::RuntimeAllocationSiteId>,
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
        self.allocate_initialized(site, type_id, class, kind, Arc::new(object_map), slots)
    }

    pub(crate) fn publish_array_filled_on_page(
        &mut self,
        reference: ManagedReference,
        request: &pop_runtime_interface::ArrayAllocationRequest,
        page_object_map: Arc<ObjectMap>,
        words: crate::heap::PageWords,
        start: usize,
        value: u64,
    ) -> Result<(), RuntimeFailure> {
        let length = usize::try_from(page_object_map.slot_count())
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        if page_object_map.slot_count() != request.length() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        match request.element_map() {
            ArrayElementMap::Scalar if page_object_map.has_reference_slots() => {
                return Err(RuntimeFailure::runtime_invariant());
            }
            ArrayElementMap::ManagedReference
                if page_object_map.reference_slot_count() != length =>
            {
                return Err(RuntimeFailure::runtime_invariant());
            }
            ArrayElementMap::ManagedReference if value != 0 => {
                self.validate_reference(ManagedReference::new(value))?;
            }
            ArrayElementMap::Scalar | ArrayElementMap::ManagedReference => {}
        }
        let slots = SlotStorage::from_page_fill(words, start, length, value)
            .map_err(|()| RuntimeFailure::runtime_invariant())?;
        let remembers_nursery = matches!(
            request.allocation_class(),
            AllocationClass::Mature | AllocationClass::Large | AllocationClass::Pinned
        ) && request.element_map() == ArrayElementMap::ManagedReference
            && value != 0
            && matches!(
                self.generation(ManagedReference::new(value)),
                Some(CollectorGeneration::Nursery { .. })
            );
        self.publish_initialized_with_remembered(
            reference,
            None,
            request.type_id(),
            request.allocation_class(),
            AllocationKind::Array(request.element_map()),
            page_object_map,
            slots,
            Some(remembers_nursery),
        )
    }

    fn allocate_initialized(
        &mut self,
        site: Option<pop_runtime_interface::RuntimeAllocationSiteId>,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        kind: AllocationKind,
        object_map: Arc<ObjectMap>,
        slots: SlotStorage,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let reference = self.fresh_reference()?;
        self.publish_initialized(reference, site, type_id, class, kind, object_map, slots)?;
        Ok(reference)
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_initialized(
        &mut self,
        reference: ManagedReference,
        site: Option<pop_runtime_interface::RuntimeAllocationSiteId>,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        kind: AllocationKind,
        object_map: Arc<ObjectMap>,
        slots: SlotStorage,
    ) -> Result<(), RuntimeFailure> {
        self.publish_initialized_with_remembered(
            reference, site, type_id, class, kind, object_map, slots, None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_initialized_with_remembered(
        &mut self,
        reference: ManagedReference,
        site: Option<pop_runtime_interface::RuntimeAllocationSiteId>,
        type_id: RuntimeTypeId,
        class: AllocationClass,
        kind: AllocationKind,
        object_map: Arc<ObjectMap>,
        slots: SlotStorage,
        remembers_nursery: Option<bool>,
    ) -> Result<(), RuntimeFailure> {
        let identity = self.reserve_identity()?;
        let generation = match class {
            AllocationClass::NurseryEligible => CollectorGeneration::Nursery { age: 0 },
            AllocationClass::Mature | AllocationClass::Large | AllocationClass::Pinned => {
                CollectorGeneration::Mature
            }
        };
        let remembers_nursery = remembers_nursery.unwrap_or_else(|| {
            generation == CollectorGeneration::Mature
                && object_map.iter_reference_slots().any(|slot| {
                    slots
                        .get(slot.raw() as usize)
                        .expect("validated object slot")
                        .as_reference()
                        .is_some_and(|child| {
                            matches!(
                                self.generation(child),
                                Some(CollectorGeneration::Nursery { .. })
                            )
                        })
                })
        });
        self.objects
            .insert_fresh(
                reference,
                RelocationAllocation {
                    identity,
                    generation,
                    allocation: Allocation {
                        kind,
                        site,
                        type_id,
                        class,
                        object_map,
                        slots,
                        immutable_bytes: None,
                    },
                    ownership: ObjectOwnership::default(),
                    mutability: crate::ObjectMutability::Mutable,
                },
            )
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        if remembers_nursery {
            self.dirty_cards.insert(reference);
        }
        self.metrics.record_allocation();
        Ok(())
    }

    fn validate_object_initializer(
        &self,
        object_map: &ObjectMap,
        values: &[u64],
    ) -> Result<(), RuntimeFailure> {
        if values.len() != object_map.slot_count() as usize {
            return Err(RuntimeFailure::runtime_invariant());
        }
        for slot in object_map.iter_reference_slots() {
            let value = values
                .get(slot.raw() as usize)
                .copied()
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            if value != 0 {
                self.validate_reference(ManagedReference::new(value))?;
            }
        }
        Ok(())
    }
}
