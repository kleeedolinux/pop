//! Opaque stable-token TLAB reservation and publication records.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectMap, RuntimeFailure, RuntimeTypeId, SchedulerId,
};

use crate::heap::{Allocation, AllocationKind, PageWords, SlotStorage};
use crate::ownership::{ObjectMutability, ObjectOwnership};
use crate::relocation::{CollectorGeneration, CollectorObjectId};

use super::{DirectReferenceLease, DirectReferenceValidation};

pub struct ReservedMatureObject {
    first_reference: ManagedReference,
    first_identity: CollectorObjectId,
    payload: PageWords,
    start: usize,
    count: usize,
    words_per_object: usize,
}

pub struct PendingMatureObject {
    pub(crate) reference: ManagedReference,
    pub(crate) identity: CollectorObjectId,
    pub(crate) allocation: Allocation,
    pub(crate) scheduler: SchedulerId,
}

pub struct ReservedMatureLease {
    type_id: RuntimeTypeId,
    object_map: Arc<ObjectMap>,
    scheduler: SchedulerId,
    reservations: Vec<ReservedMatureObject>,
    reserved: usize,
    initialized: usize,
    reservation_index: usize,
    reservation_offset: usize,
    validation: DirectReferenceLease,
}

pub(crate) struct ReservedMaturePublication {
    pub(crate) type_id: RuntimeTypeId,
    pub(crate) object_map: Arc<ObjectMap>,
    pub(crate) scheduler: SchedulerId,
    pub(crate) reservations: Vec<ReservedMatureObject>,
    pub(crate) reserved: usize,
    pub(crate) initialized: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct ReservedMatureIdentity {
    first_reference: ManagedReference,
    first_identity: CollectorObjectId,
    count: usize,
}

impl ReservedMatureIdentity {
    pub(crate) const fn new(
        first_reference: ManagedReference,
        first_identity: CollectorObjectId,
        count: usize,
    ) -> Self {
        Self {
            first_reference,
            first_identity,
            count,
        }
    }

    pub(crate) const fn len(self) -> usize {
        self.count
    }

    pub(crate) fn at(self, offset: usize) -> Option<(ManagedReference, CollectorObjectId)> {
        if offset >= self.count {
            return None;
        }
        let offset = u64::try_from(offset).ok()?;
        Some((
            ManagedReference::new(self.first_reference.raw().checked_add(offset)?),
            self.first_identity.checked_add(offset)?,
        ))
    }
}

impl ReservedMatureObject {
    pub(crate) fn new(
        identity: (ManagedReference, CollectorObjectId),
        payload: PageWords,
        start: usize,
        count: usize,
        words_per_object: usize,
    ) -> Self {
        Self {
            first_reference: identity.0,
            first_identity: identity.1,
            payload,
            start,
            count,
            words_per_object,
        }
    }

    #[must_use]
    pub const fn reference(&self) -> ManagedReference {
        self.first_reference
    }

    pub(crate) fn last_reference(&self) -> Option<ManagedReference> {
        let offset = u64::try_from(self.count.checked_sub(1)?).ok()?;
        Some(ManagedReference::new(
            self.first_reference.raw().checked_add(offset)?,
        ))
    }

    fn object_at(&self, offset: usize) -> Option<(ManagedReference, CollectorObjectId, usize)> {
        if offset >= self.count {
            return None;
        }
        let raw_offset = u64::try_from(offset).ok()?;
        Some((
            ManagedReference::new(self.first_reference.raw().checked_add(raw_offset)?),
            self.first_identity.checked_add(raw_offset)?,
            self.start
                .checked_add(offset.checked_mul(self.words_per_object)?)?,
        ))
    }

    fn truncate(&mut self, count: usize) {
        self.count = self.count.min(count);
    }
}

impl ReservedMatureLease {
    pub(crate) fn new(
        type_id: RuntimeTypeId,
        scheduler: SchedulerId,
        object_map: Arc<ObjectMap>,
        reservations: Vec<ReservedMatureObject>,
        validation: DirectReferenceLease,
    ) -> Self {
        let reserved = reservations.iter().map(|span| span.count).sum();
        Self {
            type_id,
            object_map,
            scheduler,
            reservations,
            reserved,
            initialized: 0,
            reservation_index: 0,
            reservation_offset: 0,
            validation,
        }
    }

    #[must_use]
    pub fn direct_validation(&self) -> DirectReferenceValidation {
        self.validation.validation()
    }

    /// Initializes and completes the next object in reservation order.
    ///
    /// # Errors
    ///
    /// Rejects an exhausted lease, an invalid initializer width or payload
    /// range, a non-pointer-free layout, or a stale/out-of-order lease.
    #[inline]
    pub fn initialize_next(&mut self, values: &[u64]) -> Result<ManagedReference, RuntimeFailure> {
        if self.object_map.has_reference_slots()
            || values.len() != self.object_map.slot_count() as usize
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let reservation = self
            .reservations
            .get(self.reservation_index)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let (reference, _, start) = reservation
            .object_at(self.reservation_offset)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let end = start
            .checked_add(values.len())
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let target = reservation
            .payload
            .get(start..end)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        for (word, value) in target.iter().zip(values.iter().copied()) {
            word.store(value, Ordering::Relaxed);
        }
        if !self.validation.complete(reference) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.initialized += 1;
        self.reservation_offset += 1;
        if self.reservation_offset == reservation.count {
            self.reservation_index += 1;
            self.reservation_offset = 0;
        }
        Ok(reference)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.initialized == self.reserved
    }

    pub(crate) fn into_publication(self) -> Result<ReservedMaturePublication, RuntimeFailure> {
        if !self.validation.still_valid() {
            return Err(RuntimeFailure::runtime_invariant());
        }
        Ok(ReservedMaturePublication {
            type_id: self.type_id,
            object_map: self.object_map,
            scheduler: self.scheduler,
            reservations: self.reservations,
            reserved: self.reserved,
            initialized: self.initialized,
        })
    }
}

impl ReservedMaturePublication {
    pub(crate) fn bounds(&self) -> Option<(ManagedReference, ManagedReference)> {
        Some((
            self.reservations.first()?.reference(),
            self.reservations.last()?.last_reference()?,
        ))
    }

    pub(crate) fn initialized_bounds(&self) -> Option<(ManagedReference, ManagedReference)> {
        if self.initialized == 0 {
            return None;
        }
        let first = self.reservations.first()?.reference();
        let offset = u64::try_from(self.initialized - 1).ok()?;
        Some((
            first,
            ManagedReference::new(first.raw().checked_add(offset)?),
        ))
    }

    pub(crate) const fn initialized_count(&self) -> usize {
        self.initialized
    }

    pub(crate) fn contains(&self, reference: ManagedReference) -> bool {
        self.initialized_bounds()
            .is_some_and(|(first, last)| (first..=last).contains(&reference))
    }

    pub(crate) fn initialized_page_ranges_are_valid(&self) -> bool {
        let mut remaining = self.initialized;
        for reservation in &self.reservations {
            let count = reservation.count.min(remaining);
            let Some(words) = count.checked_mul(reservation.words_per_object) else {
                return false;
            };
            if count != 0
                && !SlotStorage::page_range_is_valid(&reservation.payload, reservation.start, words)
            {
                return false;
            }
            remaining -= count;
            if remaining == 0 {
                return true;
            }
        }
        remaining == 0
    }

    pub(crate) fn cancel_unused_tail(&mut self) -> Option<(ManagedReference, ManagedReference)> {
        if self.initialized == self.reserved {
            return None;
        }
        let first = self.reservations.first()?.reference();
        let unused_first = ManagedReference::new(
            first
                .raw()
                .checked_add(u64::try_from(self.initialized).ok()?)?,
        );
        let unused_last = self.reservations.last()?.last_reference()?;
        let mut remaining = self.initialized;
        self.reservations.retain_mut(|reservation| {
            if remaining == 0 {
                return false;
            }
            let retained = reservation.count.min(remaining);
            reservation.truncate(retained);
            remaining -= retained;
            true
        });
        self.reserved = self.initialized;
        Some((unused_first, unused_last))
    }

    pub(crate) fn into_pending(self) -> Result<Vec<PendingMatureObject>, RuntimeFailure> {
        let mut pending = Vec::new();
        pending
            .try_reserve_exact(self.initialized)
            .map_err(|_| RuntimeFailure::runtime_invariant())?;
        let mut remaining = self.initialized;
        for reservation in self.reservations {
            let count = reservation.count.min(remaining);
            for offset in 0..count {
                let (reference, identity, start) = reservation
                    .object_at(offset)
                    .ok_or_else(RuntimeFailure::runtime_invariant)?;
                if !SlotStorage::page_range_is_valid(
                    &reservation.payload,
                    start,
                    self.object_map.slot_count() as usize,
                ) {
                    return Err(RuntimeFailure::runtime_invariant());
                }
                pending.push(PendingMatureObject {
                    reference,
                    identity,
                    allocation: Allocation {
                        kind: AllocationKind::Object,
                        site: None,
                        type_id: self.type_id,
                        class: AllocationClass::Mature,
                        object_map: self.object_map.clone(),
                        slots: SlotStorage::from_validated_page_range(
                            reservation.payload.clone(),
                            start,
                            self.object_map.slot_count() as usize,
                        ),
                        immutable_bytes: None,
                    },
                    scheduler: self.scheduler,
                });
            }
            remaining -= count;
            if remaining == 0 {
                break;
            }
        }
        Ok(pending)
    }
}

impl PendingMatureObject {
    #[must_use]
    pub const fn reference(&self) -> ManagedReference {
        self.reference
    }

    pub(crate) fn into_relocation_allocation(
        self,
    ) -> (ManagedReference, crate::relocation::RelocationAllocation) {
        (
            self.reference,
            crate::relocation::RelocationAllocation {
                identity: self.identity,
                generation: CollectorGeneration::Mature,
                allocation: self.allocation,
                ownership: ObjectOwnership::SchedulerLocal(self.scheduler),
                mutability: ObjectMutability::Mutable,
            },
        )
    }
}
