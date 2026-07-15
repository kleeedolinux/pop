//! Trusted immutable-byte storage and scoped payload borrowing.

use pop_runtime_interface::{
    AllocationClass, FfiBytesBorrow, FfiBytesBorrowId, ForeignAddress,
    IMMUTABLE_BYTES_RUNTIME_TYPE_ID, ManagedReference, ObjectAllocationRequest, ObjectMap,
    RuntimeAdapter, RuntimeFailure,
};

use super::heap::GenerationalRuntime;

impl GenerationalRuntime {
    pub(crate) fn allocate_immutable_bytes_with_class(
        &mut self,
        bytes: &[u8],
        class: AllocationClass,
    ) -> Result<ManagedReference, RuntimeFailure> {
        let request = ObjectAllocationRequest::new(
            IMMUTABLE_BYTES_RUNTIME_TYPE_ID,
            class,
            ObjectMap::scalar(0),
        );
        let reference = self.allocate_object(&request)?;
        let allocation = &mut self
            .nursery
            .objects
            .get_mut(&reference)
            .ok_or_else(RuntimeFailure::runtime_invariant)?
            .allocation;
        if allocation.type_id != IMMUTABLE_BYTES_RUNTIME_TYPE_ID
            || allocation.object_map.slot_count() != 0
            || allocation.immutable_bytes.is_some()
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        allocation.immutable_bytes = Some(std::sync::Arc::<[u8]>::from(bytes));
        Ok(reference)
    }

    fn immutable_bytes(&self, reference: ManagedReference) -> Option<&[u8]> {
        let allocation = &self.nursery.objects.get(&reference)?.allocation;
        (allocation.type_id == IMMUTABLE_BYTES_RUNTIME_TYPE_ID
            && allocation.object_map.slot_count() == 0)
            .then_some(allocation.immutable_bytes.as_deref())
            .flatten()
    }

    pub(super) fn borrow_immutable_bytes(
        &mut self,
        bytes: ManagedReference,
    ) -> Result<FfiBytesBorrow, RuntimeFailure> {
        if self
            .ffi_bytes_borrows
            .values()
            .any(|(owner, _)| *owner == bytes)
            || self.immutable_bytes(bytes).is_none()
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let pin = self.pin(bytes)?;
        let payload = self.immutable_bytes(bytes).and_then(|payload| {
            let length = u64::try_from(payload.len()).ok()?;
            let address = if payload.is_empty() {
                None
            } else {
                ForeignAddress::new(u64::try_from(payload.as_ptr().addr()).ok()?)
            };
            Some((address, length))
        });
        let Some((address, length)) = payload else {
            self.unpin(pin)?;
            return Err(RuntimeFailure::runtime_invariant());
        };
        let Some(raw) = self.next_ffi_bytes_borrow.checked_add(1) else {
            self.unpin(pin)?;
            return Err(RuntimeFailure::runtime_invariant());
        };
        let Some(borrow) = FfiBytesBorrowId::new(raw) else {
            self.unpin(pin)?;
            return Err(RuntimeFailure::runtime_invariant());
        };
        let Some(result) = FfiBytesBorrow::new(borrow, address, length) else {
            self.unpin(pin)?;
            return Err(RuntimeFailure::runtime_invariant());
        };
        self.next_ffi_bytes_borrow = raw;
        self.ffi_bytes_borrows.insert(borrow, (bytes, pin));
        Ok(result)
    }

    pub(super) fn end_immutable_bytes_borrow(
        &mut self,
        bytes: ManagedReference,
        borrow: FfiBytesBorrowId,
    ) -> Result<(), RuntimeFailure> {
        let Some((owner, pin)) = self.ffi_bytes_borrows.get(&borrow).copied() else {
            return Err(RuntimeFailure::runtime_invariant());
        };
        if owner != bytes {
            return Err(RuntimeFailure::runtime_invariant());
        }
        self.unpin(pin)?;
        self.ffi_bytes_borrows.remove(&borrow);
        Ok(())
    }
}
