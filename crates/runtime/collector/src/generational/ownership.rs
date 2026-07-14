//! Explicit local-to-shared graph publication and ownership barrier checks.

use std::collections::BTreeSet;

use pop_runtime_interface::{ManagedReference, RuntimeFailure};

use crate::heap::{BootstrapRuntime, SlotValue};
use crate::ownership::{ObjectOwnership, PublicationStatistics};
use crate::relocation::CollectorGeneration;

use super::heap::GenerationalRuntime;

impl GenerationalRuntime {
    #[must_use]
    pub fn ownership(&self, reference: ManagedReference) -> Option<ObjectOwnership> {
        self.nursery
            .objects
            .get(&reference)
            .map(|object| object.ownership)
    }

    /// Publishes a complete scheduler-local graph into shared ownership.
    ///
    /// # Errors
    ///
    /// Rejects stale references, isolated ownership, invalid object maps, and
    /// memory-limit violations without publishing a partial graph.
    pub fn publish_shared(
        &mut self,
        root: ManagedReference,
    ) -> Result<PublicationStatistics, RuntimeFailure> {
        let mut pending = vec![root];
        let mut publish = BTreeSet::new();
        while let Some(reference) = pending.pop() {
            let object = self
                .nursery
                .objects
                .get(&reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            match object.ownership {
                ObjectOwnership::Shared => continue,
                ObjectOwnership::Isolated(_) => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
                ObjectOwnership::SchedulerLocal(_) => {}
            }
            if !publish.insert(reference) {
                continue;
            }
            for slot in object.allocation.object_map.reference_slots() {
                match object.allocation.slots.get(slot.raw() as usize) {
                    Some(SlotValue::Reference(Some(child))) => pending.push(*child),
                    Some(SlotValue::Reference(None)) => {}
                    Some(SlotValue::Scalar(_)) | None => {
                        return Err(RuntimeFailure::runtime_invariant());
                    }
                }
            }
        }

        let mut next_allocation = self.allocation.clone();
        for reference in &publish {
            let object = self
                .nursery
                .objects
                .get(reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            next_allocation.move_to_shared(
                *reference,
                object.allocation.type_id,
                &object.allocation.object_map,
            )?;
        }
        if !self.memory.admits(next_allocation.committed_bytes()) {
            self.memory.record_out_of_memory();
            return Err(BootstrapRuntime::out_of_memory(0, 0));
        }

        self.allocation = next_allocation;
        for reference in &publish {
            let object = self
                .nursery
                .objects
                .get_mut(reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            object.ownership = ObjectOwnership::Shared;
            object.generation = CollectorGeneration::Mature;
        }
        for reference in &publish {
            self.mark_new_allocation(*reference);
        }
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(PublicationStatistics::new(publish.len()))
    }

    pub(crate) fn validate_ownership_edge(
        &self,
        owner: ManagedReference,
        value: Option<ManagedReference>,
    ) -> Result<(), RuntimeFailure> {
        let owner = self
            .ownership(owner)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let Some(target) = value else {
            return Ok(());
        };
        let target = self
            .ownership(target)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let allowed = match (owner, target) {
            (_, ObjectOwnership::Shared)
            | (ObjectOwnership::SchedulerLocal(_), ObjectOwnership::Isolated(_)) => true,
            (ObjectOwnership::SchedulerLocal(owner), ObjectOwnership::SchedulerLocal(target)) => {
                owner == target
            }
            (ObjectOwnership::Isolated(owner), ObjectOwnership::Isolated(target)) => {
                owner == target
            }
            (ObjectOwnership::Shared, _)
            | (ObjectOwnership::Isolated(_), ObjectOwnership::SchedulerLocal(_)) => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }
}
