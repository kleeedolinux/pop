//! Explicit local-to-shared graph publication and ownership barrier checks.

use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::{ManagedReference, RootHandle, RootPublication, RuntimeFailure};

use crate::SchedulerId;
use crate::heap::BootstrapRuntime;
use crate::ownership::{
    FreezeStatistics, IsolatedRegionId, IsolationStatistics, IsolationTelemetry, ObjectMutability,
    ObjectOwnership, PublicationStatistics,
};
use crate::relocation::CollectorGeneration;

use super::heap::GenerationalRuntime;

#[derive(Clone)]
struct IsolatedRegionRecord {
    owner: SchedulerId,
    owner_handle: RootHandle,
    objects: BTreeSet<ManagedReference>,
}

pub(crate) struct IsolationState {
    regions: BTreeMap<IsolatedRegionId, IsolatedRegionRecord>,
    next_region: u64,
    telemetry: IsolationTelemetry,
}

impl IsolationState {
    pub(crate) fn new() -> Self {
        Self {
            regions: BTreeMap::new(),
            next_region: 1,
            telemetry: IsolationTelemetry::default(),
        }
    }

    fn fresh_region(&mut self) -> Result<IsolatedRegionId, RuntimeFailure> {
        let region = IsolatedRegionId::new(self.next_region);
        self.next_region = self
            .next_region
            .checked_add(1)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        Ok(region)
    }

    pub(crate) fn owns_handle(&self, handle: RootHandle) -> bool {
        self.regions
            .values()
            .any(|region| region.owner_handle == handle)
    }
}

impl GenerationalRuntime {
    #[must_use]
    pub fn ownership(&self, reference: ManagedReference) -> Option<ObjectOwnership> {
        self.nursery
            .objects
            .get(&reference)
            .map(|object| object.ownership)
    }

    #[must_use]
    pub fn mutability(&self, reference: ManagedReference) -> Option<ObjectMutability> {
        self.nursery
            .objects
            .get(&reference)
            .map(|object| object.mutability)
    }

    /// Atomically freezes the complete managed-reference closure of a shared root.
    ///
    /// # Errors
    ///
    /// Rejects stale references, malformed maps, or any reached non-shared
    /// object without partially changing mutability.
    pub fn freeze_shared(
        &mut self,
        root: ManagedReference,
    ) -> Result<FreezeStatistics, RuntimeFailure> {
        let mut pending = vec![root];
        let mut freeze = BTreeSet::new();
        while let Some(reference) = pending.pop() {
            let object = self
                .nursery
                .objects
                .get(&reference)
                .filter(|object| object.ownership == ObjectOwnership::Shared)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            if !freeze.insert(reference) {
                continue;
            }
            append_object_references(object, &mut pending)?;
        }
        for reference in &freeze {
            self.nursery
                .objects
                .get_mut(reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?
                .mutability = ObjectMutability::SharedImmutable;
        }
        Ok(FreezeStatistics::new(freeze.len()))
    }

    pub(crate) fn ensure_mutable(&self, owner: ManagedReference) -> Result<(), RuntimeFailure> {
        match self.mutability(owner) {
            Some(ObjectMutability::Mutable) => Ok(()),
            Some(ObjectMutability::SharedImmutable) | None => {
                Err(RuntimeFailure::runtime_invariant())
            }
        }
    }

    /// Constructs an isolated region after verifying one external owner.
    ///
    /// # Errors
    ///
    /// Rejects stale handles, mixed local owners, existing isolated objects,
    /// additional roots/pins/stack roots, outside incoming edges, malformed
    /// object maps, and memory-limit violations without a partial transition.
    pub fn isolate_graph(
        &mut self,
        owner_handle: RootHandle,
        roots: &RootPublication,
        owner: SchedulerId,
    ) -> Result<IsolationStatistics, RuntimeFailure> {
        let root = self
            .nursery
            .roots
            .get(&owner_handle)
            .copied()
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let mut pending = vec![root];
        let mut objects = BTreeSet::new();
        while let Some(reference) = pending.pop() {
            let object = self
                .nursery
                .objects
                .get(&reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            match object.ownership {
                ObjectOwnership::SchedulerLocal(scheduler) if scheduler == owner => {}
                ObjectOwnership::Shared => continue,
                ObjectOwnership::SchedulerLocal(_) | ObjectOwnership::Isolated(_) => {
                    return Err(RuntimeFailure::runtime_invariant());
                }
            }
            if !objects.insert(reference) {
                continue;
            }
            append_object_references(object, &mut pending)?;
        }
        if objects.is_empty()
            || self
                .nursery
                .roots
                .iter()
                .any(|(handle, reference)| *handle != owner_handle && objects.contains(reference))
            || self
                .nursery
                .pins
                .values()
                .any(|reference| objects.contains(reference))
            || roots
                .managed_references()
                .any(|reference| objects.contains(&reference))
        {
            return Err(RuntimeFailure::runtime_invariant());
        }
        for (reference, object) in self.nursery.objects.iter() {
            if objects.contains(&reference) {
                continue;
            }
            let mut outgoing = Vec::new();
            append_object_references(object, &mut outgoing)?;
            if outgoing.iter().any(|target| objects.contains(target)) {
                return Err(RuntimeFailure::runtime_invariant());
            }
        }

        let mut next_allocation = self.allocation.clone();
        for reference in &objects {
            let object = self
                .nursery
                .objects
                .get(reference)
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            next_allocation.move_to_isolated(
                *reference,
                object.allocation.type_id,
                &object.allocation.object_map,
            )?;
        }
        if !self.memory.admits(next_allocation.committed_bytes()) {
            self.memory.record_out_of_memory();
            return Err(BootstrapRuntime::out_of_memory(0, 0));
        }

        let region = self.isolation.fresh_region()?;
        self.allocation = next_allocation;
        for (reference, object) in self.nursery.objects.iter_mut() {
            if objects.contains(&reference) {
                object.ownership = ObjectOwnership::Isolated(region);
                object.generation = CollectorGeneration::Mature;
            }
        }
        self.isolation.regions.insert(
            region,
            IsolatedRegionRecord {
                owner,
                owner_handle,
                objects: objects.clone(),
            },
        );
        self.isolation.telemetry.regions_created =
            self.isolation.telemetry.regions_created.saturating_add(1);
        for reference in &objects {
            self.mark_new_allocation(*reference);
        }
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(IsolationStatistics::new(region, objects.len()))
    }

    /// Transfers the unique external owner without copying the isolated graph.
    ///
    /// # Errors
    ///
    /// Rejects unknown regions and stale owner capabilities.
    pub fn transfer_isolated(
        &mut self,
        region: IsolatedRegionId,
        from: SchedulerId,
        to: SchedulerId,
    ) -> Result<(), RuntimeFailure> {
        let record = self
            .isolation
            .regions
            .get_mut(&region)
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if record.owner != from {
            return Err(RuntimeFailure::runtime_invariant());
        }
        record.owner = to;
        self.isolation.telemetry.transfers_completed = self
            .isolation
            .telemetry
            .transfers_completed
            .saturating_add(1);
        self.isolation.telemetry.objects_transferred = self
            .isolation
            .telemetry
            .objects_transferred
            .saturating_add(u64::try_from(record.objects.len()).unwrap_or(u64::MAX));
        Ok(())
    }

    /// Returns an isolated graph to its owning scheduler-local mature heap.
    ///
    /// # Errors
    ///
    /// Rejects unknown regions, stale owners, placement failures, and memory
    /// pressure without partially dissolving the region.
    pub fn dissolve_isolated(
        &mut self,
        region: IsolatedRegionId,
        owner: SchedulerId,
    ) -> Result<(), RuntimeFailure> {
        let record = self
            .isolation
            .regions
            .get(&region)
            .filter(|record| record.owner == owner)
            .cloned()
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        let mut next_allocation = self.allocation.clone();
        for reference in &record.objects {
            let object = self
                .nursery
                .objects
                .get(reference)
                .filter(|object| object.ownership == ObjectOwnership::Isolated(region))
                .ok_or_else(RuntimeFailure::runtime_invariant)?;
            next_allocation.move_to_local_mature(
                *reference,
                object.allocation.type_id,
                &object.allocation.object_map,
                owner,
            )?;
        }
        if !self.memory.admits(next_allocation.committed_bytes()) {
            self.memory.record_out_of_memory();
            return Err(BootstrapRuntime::out_of_memory(0, 0));
        }
        self.allocation = next_allocation;
        for (reference, object) in self.nursery.objects.iter_mut() {
            if record.objects.contains(&reference) {
                object.ownership = ObjectOwnership::SchedulerLocal(owner);
            }
        }
        self.isolation.regions.remove(&region);
        self.isolation.telemetry.regions_dissolved =
            self.isolation.telemetry.regions_dissolved.saturating_add(1);
        self.memory
            .observe_committed(self.allocation.committed_bytes());
        Ok(())
    }

    #[must_use]
    pub fn isolated_region_owner(&self, region: IsolatedRegionId) -> Option<SchedulerId> {
        self.isolation
            .regions
            .get(&region)
            .map(|record| record.owner)
    }

    #[must_use]
    pub const fn isolation_telemetry(&self) -> IsolationTelemetry {
        self.isolation.telemetry
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
                let value = object
                    .allocation
                    .slots
                    .get(slot.raw() as usize)
                    .copied()
                    .ok_or_else(RuntimeFailure::runtime_invariant)?;
                if let Some(child) = value.as_reference() {
                    pending.push(child);
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
            (_, ObjectOwnership::Shared) => true,
            (ObjectOwnership::SchedulerLocal(owner), ObjectOwnership::SchedulerLocal(target)) => {
                owner == target
            }
            (ObjectOwnership::Isolated(owner), ObjectOwnership::Isolated(target)) => {
                owner == target
            }
            (ObjectOwnership::Shared, _)
            | (ObjectOwnership::SchedulerLocal(_), ObjectOwnership::Isolated(_))
            | (ObjectOwnership::Isolated(_), ObjectOwnership::SchedulerLocal(_)) => false,
        };
        if allowed {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }
}

fn append_object_references(
    object: &crate::relocation::RelocationAllocation,
    pending: &mut Vec<ManagedReference>,
) -> Result<(), RuntimeFailure> {
    for slot in object.allocation.object_map.reference_slots() {
        let value = object
            .allocation
            .slots
            .get(slot.raw() as usize)
            .copied()
            .ok_or_else(RuntimeFailure::runtime_invariant)?;
        if let Some(reference) = value.as_reference() {
            pending.push(reference);
        }
    }
    Ok(())
}
