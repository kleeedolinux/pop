//! Precise reachability, allocation capacity, and collection work.

use std::collections::BTreeSet;

use pop_runtime_interface::{
    AllocationClass, CollectionStatistics, ManagedReference, ObjectMap, ObjectSlot, PanicPayload,
    RootPublication, RuntimeFailure, RuntimeTypeId,
};

use crate::heap::{Allocation, AllocationKind, BootstrapRuntime, SlotValue};

impl BootstrapRuntime {
    pub const fn request_collection(&mut self) {
        self.collection_requested = true;
    }

    /// Performs a precise stop-the-world collection using registered strong
    /// roots plus the stack roots published for this safe point.
    ///
    /// # Errors
    ///
    /// Returns a portable invariant panic if a root or traced edge names an
    /// invalid managed reference.
    pub fn collect(
        &mut self,
        stack_roots: &RootPublication,
    ) -> Result<CollectionStatistics, RuntimeFailure> {
        let mut roots: Vec<_> = self.roots.values().copied().collect();
        roots.extend(self.pins.values().copied());
        roots.extend(stack_roots.managed_references());
        self.collect_references(&roots)
    }

    pub(crate) fn allocate(
        &mut self,
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        kind: AllocationKind,
        object_map: ObjectMap,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocate_with_initial(type_id, allocation_class, kind, object_map, None)
    }

    pub(crate) fn allocate_filled(
        &mut self,
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        kind: AllocationKind,
        object_map: ObjectMap,
        initial_value: u64,
    ) -> Result<ManagedReference, RuntimeFailure> {
        self.allocate_with_initial(
            type_id,
            allocation_class,
            kind,
            object_map,
            Some(initial_value),
        )
    }

    fn allocate_with_initial(
        &mut self,
        type_id: RuntimeTypeId,
        allocation_class: AllocationClass,
        kind: AllocationKind,
        object_map: ObjectMap,
        initial_value: Option<u64>,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let requested_slots = usize::try_from(object_map.slot_count())
            .map_err(|_| Self::out_of_memory(1, usize::MAX))?;
        self.ensure_capacity(requested_slots)?;
        if !object_map.reference_slots().is_empty()
            && let Some(value) = initial_value.filter(|value| *value != 0)
        {
            self.validate_reference(ManagedReference::new(value))?;
        }
        let reference = ManagedReference::new(self.next_reference);
        self.next_reference = self
            .next_reference
            .checked_add(1)
            .ok_or_else(|| Self::out_of_memory(1, requested_slots))?;
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(requested_slots)
            .map_err(|_| Self::out_of_memory(1, requested_slots))?;
        for index in 0..object_map.slot_count() {
            slots.push(if object_map.is_reference_slot(ObjectSlot::new(index)) {
                SlotValue::Reference(
                    initial_value
                        .filter(|value| *value != 0)
                        .map(ManagedReference::new),
                )
            } else {
                SlotValue::Scalar(initial_value.unwrap_or(0))
            });
        }
        self.objects.insert(
            reference,
            Allocation {
                kind,
                type_id,
                class: allocation_class,
                object_map,
                slots,
            },
        );
        self.slot_count += requested_slots;
        self.metrics.record_allocation();
        Ok(reference)
    }

    fn ensure_capacity(&mut self, requested_slots: usize) -> Result<(), RuntimeFailure> {
        if self.has_capacity(requested_slots) {
            return Ok(());
        }
        let mut registered_roots: Vec<_> = self.roots.values().copied().collect();
        registered_roots.extend(self.pins.values().copied());
        self.collect_references(&registered_roots)?;
        if self.has_capacity(requested_slots) {
            Ok(())
        } else {
            Err(Self::out_of_memory(1, requested_slots))
        }
    }

    fn has_capacity(&self, requested_slots: usize) -> bool {
        self.objects.len() < self.limits.maximum_objects
            && self
                .slot_count
                .checked_add(requested_slots)
                .is_some_and(|slots| slots <= self.limits.maximum_slots)
    }

    fn collect_references(
        &mut self,
        roots: &[ManagedReference],
    ) -> Result<CollectionStatistics, RuntimeFailure> {
        let before = self.objects.len();
        let mut marked = BTreeSet::new();
        let mut pending = roots.to_vec();
        while let Some(reference) = pending.pop() {
            if !marked.insert(reference) {
                continue;
            }
            let allocation = self
                .objects
                .get(&reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            for slot in allocation.object_map.reference_slots() {
                match allocation.slots.get(slot.raw() as usize) {
                    Some(SlotValue::Reference(Some(child))) => pending.push(*child),
                    Some(SlotValue::Reference(None)) => {}
                    Some(SlotValue::Scalar(_)) | None => {
                        return Err(RuntimeFailure::runtime_invariant());
                    }
                }
            }
        }

        self.objects
            .retain(|reference, _| marked.contains(reference));
        self.slot_count = self
            .objects
            .values()
            .map(|allocation| allocation.slots.len())
            .sum();
        let live = self.objects.len();
        let statistics = CollectionStatistics::new(
            portable_count(live),
            portable_count(before - live),
            portable_count(marked.len()),
        );
        self.metrics
            .record_collection(statistics.reclaimed_objects(), statistics.scanned_objects());
        Ok(statistics)
    }

    pub(crate) fn validate_reference(
        &self,
        reference: ManagedReference,
    ) -> Result<(), RuntimeFailure> {
        if self.contains(reference) {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }

    pub(crate) fn out_of_memory(
        requested_objects: usize,
        requested_slots: usize,
    ) -> RuntimeFailure {
        RuntimeFailure::from_panic(PanicPayload::out_of_memory(
            portable_count(requested_objects),
            portable_count(requested_slots),
        ))
    }
}

fn portable_count(value: usize) -> u64 {
    u64::try_from(value).unwrap_or(u64::MAX)
}
