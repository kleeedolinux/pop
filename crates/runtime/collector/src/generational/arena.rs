//! Scoped bump arenas with typed internal edges and precise managed roots.

use std::collections::BTreeMap;

use pop_runtime_interface::{ObjectSlot, RootHandle, RuntimeAdapter, RuntimeFailure};

use crate::arena::{
    ArenaAllocationRequest, ArenaCloseStatistics, ArenaConfig, ArenaId, ArenaReference,
    ArenaSlotValue, ArenaTelemetry,
};
use crate::{BootstrapRuntime, ObjectOwnership, SchedulerId};

use super::heap::GenerationalRuntime;

enum StoredSlot {
    Scalar(u64),
    ManagedReference(Option<RootHandle>),
    ArenaReference(Option<ArenaReference>),
}

struct ArenaObject {
    slots: Vec<StoredSlot>,
}

struct ArenaRecord {
    scheduler: SchedulerId,
    capacity_bytes: usize,
    used_bytes: usize,
    objects: BTreeMap<usize, ArenaObject>,
}

pub(crate) struct ArenaState {
    arenas: BTreeMap<ArenaId, ArenaRecord>,
    next_arena: u64,
    telemetry: ArenaTelemetry,
}

impl ArenaState {
    pub(crate) fn new() -> Self {
        Self {
            arenas: BTreeMap::new(),
            next_arena: 1,
            telemetry: ArenaTelemetry::default(),
        }
    }
}

impl GenerationalRuntime {
    /// Creates an empty scheduler-owned scoped arena.
    ///
    /// # Errors
    ///
    /// Rejects arena identity exhaustion.
    pub fn create_arena(&mut self, config: ArenaConfig) -> Result<ArenaId, RuntimeFailure> {
        let arena = ArenaId::new(self.arenas.next_arena);
        self.arenas.next_arena = self
            .arenas
            .next_arena
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        self.arenas.arenas.insert(
            arena,
            ArenaRecord {
                scheduler: self.scheduler,
                capacity_bytes: config.capacity_bytes(),
                used_bytes: 0,
                objects: BTreeMap::new(),
            },
        );
        self.arenas.telemetry.arenas_created =
            self.arenas.telemetry.arenas_created.saturating_add(1);
        Ok(arena)
    }

    /// Bump-allocates one typed arena object without adding it to the GC heap.
    ///
    /// # Errors
    ///
    /// Rejects stale/foreign arenas, capacity overflow, and hard-limit pressure.
    pub fn allocate_in_arena(
        &mut self,
        arena: ArenaId,
        request: &ArenaAllocationRequest,
    ) -> Result<ArenaReference, RuntimeFailure> {
        let record = self
            .arenas
            .arenas
            .get_mut(&arena)
            .filter(|record| record.scheduler == self.scheduler)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let size = usize::try_from(request.slot_count())
            .map_err(|_| RuntimeFailure::runtime_invariant())?
            .checked_mul(8)
            .map(|bytes| bytes.max(8))
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let next = record
            .used_bytes
            .checked_add(size)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if next > record.capacity_bytes {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let arena_bytes_after = self.arenas.telemetry.live_bytes.saturating_add(size);
        if !self
            .memory
            .admits_arena_bytes(arena_bytes_after, self.allocation.committed_bytes())
        {
            self.memory.record_out_of_memory();
            return Err(BootstrapRuntime::out_of_memory(
                1,
                request.slot_count() as usize,
            ));
        }
        let offset = record.used_bytes;
        let slots = (0..request.slot_count)
            .map(|index| {
                let slot = ObjectSlot::new(index);
                if request.managed_slots.contains(&slot) {
                    StoredSlot::ManagedReference(None)
                } else if request.arena_slots.contains(&slot) {
                    StoredSlot::ArenaReference(None)
                } else {
                    StoredSlot::Scalar(0)
                }
            })
            .collect();
        record.objects.insert(offset, ArenaObject { slots });
        record.used_bytes = next;
        self.arenas.telemetry.objects_allocated =
            self.arenas.telemetry.objects_allocated.saturating_add(1);
        self.arenas.telemetry.live_bytes = self.arenas.telemetry.live_bytes.saturating_add(size);
        self.memory
            .set_arena_bytes(self.arenas.telemetry.live_bytes);
        self.arenas.telemetry.peak_bytes = self
            .arenas
            .telemetry
            .peak_bytes
            .max(self.arenas.telemetry.live_bytes);
        Ok(ArenaReference::new(arena, offset))
    }

    /// Stores a same-arena reference in a declared arena-reference slot.
    ///
    /// # Errors
    ///
    /// Rejects stale tokens, cross-arena edges, and slot-kind mismatches.
    pub fn store_arena_reference(
        &mut self,
        owner: ArenaReference,
        slot: ObjectSlot,
        value: Option<ArenaReference>,
    ) -> Result<(), RuntimeFailure> {
        if value.is_some_and(|target| target.arena() != owner.arena()) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        if let Some(target) = value {
            self.arena_object(target)?;
        }
        let stored = self.arena_slot_mut(owner, slot)?;
        match stored {
            StoredSlot::ArenaReference(current) => {
                *current = value;
                Ok(())
            }
            StoredSlot::Scalar(_) | StoredSlot::ManagedReference(_) => {
                Err(RuntimeFailure::runtime_invariant())
            }
        }
    }

    /// Stores a precise managed root in a declared managed-reference slot.
    ///
    /// # Errors
    ///
    /// Rejects stale tokens, foreign scheduler-local or isolated targets, and
    /// slot-kind mismatches.
    pub fn store_arena_managed_reference(
        &mut self,
        owner: ArenaReference,
        slot: ObjectSlot,
        value: Option<pop_runtime_interface::ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        let scheduler = self.arena_scheduler(owner.arena())?;
        if let Some(target) = value {
            let allowed = matches!(self.ownership(target), Some(ObjectOwnership::Shared))
                || self.ownership(target) == Some(ObjectOwnership::SchedulerLocal(scheduler));
            if !allowed {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }
        if !matches!(
            self.arena_slot(owner, slot)?,
            StoredSlot::ManagedReference(_)
        ) {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let next = if let Some(target) = value {
            let handle = self.nursery.retain_root(target)?;
            self.shade_new_root(target);
            Some(handle)
        } else {
            None
        };
        let previous = match self.arena_slot_mut(owner, slot)? {
            StoredSlot::ManagedReference(current) => std::mem::replace(current, next),
            StoredSlot::Scalar(_) | StoredSlot::ArenaReference(_) => {
                if let Some(next) = next {
                    self.nursery.release_root(next)?;
                }
                return Err(RuntimeFailure::runtime_invariant());
            }
        };
        if let Some(previous) = previous {
            self.nursery.release_root(previous)?;
        }
        Ok(())
    }

    /// Loads one typed arena slot.
    ///
    /// # Errors
    ///
    /// Rejects stale arena/object tokens, invalid slots, and stale managed roots.
    pub fn load_arena_slot(
        &self,
        reference: ArenaReference,
        slot: ObjectSlot,
    ) -> Result<ArenaSlotValue, RuntimeFailure> {
        match self.arena_slot(reference, slot)? {
            StoredSlot::Scalar(value) => Ok(ArenaSlotValue::Scalar(*value)),
            StoredSlot::ArenaReference(value) => Ok(ArenaSlotValue::ArenaReference(*value)),
            StoredSlot::ManagedReference(value) => Ok(ArenaSlotValue::ManagedReference(
                value
                    .map(|handle| self.nursery.roots.get(&handle).copied())
                    .transpose_option()?,
            )),
        }
    }

    /// Releases every managed root and bulk-reclaims an arena.
    ///
    /// # Errors
    ///
    /// Rejects stale/foreign arenas and inconsistent internal roots without
    /// partially closing the arena.
    pub fn close_arena(&mut self, arena: ArenaId) -> Result<ArenaCloseStatistics, RuntimeFailure> {
        let record = self
            .arenas
            .arenas
            .get(&arena)
            .filter(|record| record.scheduler == self.scheduler)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let handles: Vec<_> = record
            .objects
            .values()
            .flat_map(|object| &object.slots)
            .filter_map(|slot| match slot {
                StoredSlot::ManagedReference(handle) => *handle,
                StoredSlot::Scalar(_) | StoredSlot::ArenaReference(_) => None,
            })
            .collect();
        if handles
            .iter()
            .any(|handle| !self.nursery.roots.contains_key(handle))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        let record = self
            .arenas
            .arenas
            .remove(&arena)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        for handle in handles {
            self.nursery.release_root(handle)?;
        }
        let statistics = ArenaCloseStatistics::new(record.objects.len(), record.used_bytes);
        self.arenas.telemetry.arenas_closed = self.arenas.telemetry.arenas_closed.saturating_add(1);
        self.arenas.telemetry.objects_bulk_reclaimed = self
            .arenas
            .telemetry
            .objects_bulk_reclaimed
            .saturating_add(statistics.objects_reclaimed());
        self.arenas.telemetry.live_bytes = self
            .arenas
            .telemetry
            .live_bytes
            .saturating_sub(statistics.bytes_reclaimed());
        self.memory
            .set_arena_bytes(self.arenas.telemetry.live_bytes);
        Ok(statistics)
    }

    #[must_use]
    pub const fn arena_telemetry(&self) -> ArenaTelemetry {
        self.arenas.telemetry
    }

    fn arena_scheduler(&self, arena: ArenaId) -> Result<SchedulerId, RuntimeFailure> {
        self.arenas
            .arenas
            .get(&arena)
            .map(|record| record.scheduler)
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn arena_object(&self, reference: ArenaReference) -> Result<&ArenaObject, RuntimeFailure> {
        self.arenas
            .arenas
            .get(&reference.arena())
            .and_then(|arena| arena.objects.get(&reference.offset_bytes()))
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn arena_slot(
        &self,
        reference: ArenaReference,
        slot: ObjectSlot,
    ) -> Result<&StoredSlot, RuntimeFailure> {
        self.arena_object(reference)?
            .slots
            .get(slot.raw() as usize)
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }

    fn arena_slot_mut(
        &mut self,
        reference: ArenaReference,
        slot: ObjectSlot,
    ) -> Result<&mut StoredSlot, RuntimeFailure> {
        self.arenas
            .arenas
            .get_mut(&reference.arena())
            .and_then(|arena| arena.objects.get_mut(&reference.offset_bytes()))
            .and_then(|object| object.slots.get_mut(slot.raw() as usize))
            .ok_or_else(RuntimeFailure::runtime_invariant)
    }
}

trait TransposeOption<T> {
    fn transpose_option(self) -> Result<Option<T>, RuntimeFailure>;
}

impl<T> TransposeOption<T> for Option<Option<T>> {
    fn transpose_option(self) -> Result<Option<T>, RuntimeFailure> {
        match self {
            Some(Some(value)) => Ok(Some(value)),
            None => Ok(None),
            Some(None) => Err(RuntimeFailure::runtime_invariant()),
        }
    }
}
