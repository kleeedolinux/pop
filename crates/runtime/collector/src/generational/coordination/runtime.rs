//! Runtime-owned major-cycle handshake integration.

use pop_runtime_interface::{RootPublication, RuntimeFailure};

use super::model::{
    CollectorEpoch, CollectorPhase, EpochCoordinatorError, EpochCoordinatorTelemetry,
    EpochProgress, MajorCollectionHandshakeError, MutatorExecutionState, MutatorId,
    MutatorPublication,
};
use crate::generational::heap::GenerationalRuntime;

impl GenerationalRuntime {
    /// Registers a mutator at the currently selected scheduler boundary.
    ///
    /// # Errors
    ///
    /// Rejects coordinator capacity or typed-identity exhaustion.
    pub fn register_mutator(
        &mut self,
        state: MutatorExecutionState,
    ) -> Result<MutatorId, EpochCoordinatorError> {
        let id = self.coordination.register_mutator(state)?;
        self.mutator_schedulers.insert(id, self.scheduler);
        Ok(id)
    }

    /// Removes a mutator from the active handshake snapshot.
    ///
    /// # Errors
    ///
    /// Rejects unknown mutators and reports a runtime invariant if completing
    /// the now-unblocked handshake cannot establish the major snapshot.
    pub fn unregister_mutator(
        &mut self,
        id: MutatorId,
    ) -> Result<(), MajorCollectionHandshakeError> {
        self.coordination.unregister_mutator(id)?;
        self.mutator_schedulers.remove(&id);
        self.major_root_snapshots.remove(&id);
        self.finish_major_collection_handshake_if_ready()
    }

    /// Changes a mutator's typed execution state during an epoch.
    ///
    /// # Errors
    ///
    /// Rejects unknown mutators and reports any failure to establish a major
    /// snapshot after an automatic acknowledgement completes the handshake.
    pub fn transition_mutator(
        &mut self,
        id: MutatorId,
        state: MutatorExecutionState,
    ) -> Result<EpochProgress, MajorCollectionHandshakeError> {
        let progress = self.coordination.transition_mutator(id, state)?;
        if progress.complete() {
            self.finish_major_collection_handshake_if_ready()?;
        }
        Ok(progress)
    }

    /// Begins the bounded root-publication handshake for a requested major
    /// collection. No tracing or worker work is dispatched by this step.
    ///
    /// # Errors
    ///
    /// Rejects an absent collection request, overlapping epoch, or major-cycle
    /// invariant failure.
    pub fn begin_major_collection_handshake(
        &mut self,
    ) -> Result<CollectorEpoch, MajorCollectionHandshakeError> {
        if !self.major_requested {
            return Err(MajorCollectionHandshakeError::CollectionNotRequested);
        }
        if self.major_cycle_active() {
            return Err(RuntimeFailure::runtime_invariant().into());
        }
        let epoch = self.coordination.begin_epoch(CollectorPhase::Marking)?;
        self.major_epoch = Some(epoch);
        self.major_root_snapshots.clear();
        self.finish_major_collection_handshake_if_ready()?;
        Ok(epoch)
    }

    /// Publishes one precise root snapshot and acknowledges the active major
    /// epoch. The final acknowledgement establishes marking only after all
    /// snapshots have passed validation.
    ///
    /// # Errors
    ///
    /// Rejects stale roots before changing acknowledgement state, or forwards
    /// typed coordinator failures for stale, duplicate, or invalid polls.
    pub fn acknowledge_major_collection_handshake(
        &mut self,
        id: MutatorId,
        epoch: CollectorEpoch,
        roots: &RootPublication,
    ) -> Result<EpochProgress, MajorCollectionHandshakeError> {
        self.validate_published_roots(roots)?;
        let scheduler = self
            .mutator_schedulers
            .get(&id)
            .copied()
            .ok_or(EpochCoordinatorError::UnknownMutator(id))?;
        let publication = MutatorPublication::new(
            roots,
            self.allocation.tlab_top_bytes(scheduler),
            self.major.satb.len(),
            self.nursery.dirty_cards.len(),
        );
        let progress = self.coordination.acknowledge(id, epoch, publication)?;
        self.major_root_snapshots.insert(id, roots.clone());
        if progress.complete() {
            self.finish_major_collection_handshake_if_ready()?;
        }
        Ok(progress)
    }

    #[must_use]
    pub const fn active_major_collection_epoch(&self) -> Option<CollectorEpoch> {
        self.major_epoch
    }

    #[must_use]
    pub fn pending_major_acknowledgements(&self) -> usize {
        self.coordination.pending_acknowledgements()
    }

    #[must_use]
    pub fn mutator_publication(&self, id: MutatorId) -> Option<MutatorPublication> {
        self.coordination.publication(id)
    }

    #[must_use]
    pub const fn epoch_coordinator_telemetry(&self) -> EpochCoordinatorTelemetry {
        self.coordination.telemetry()
    }

    pub(crate) fn has_registered_mutators(&self) -> bool {
        self.coordination.registered_mutators() != 0
    }

    fn validate_published_roots(&self, roots: &RootPublication) -> Result<(), RuntimeFailure> {
        if roots
            .managed_references()
            .all(|reference| self.nursery.contains(reference))
        {
            Ok(())
        } else {
            Err(RuntimeFailure::runtime_invariant())
        }
    }

    fn finish_major_collection_handshake_if_ready(
        &mut self,
    ) -> Result<(), MajorCollectionHandshakeError> {
        let Some(epoch) = self.major_epoch else {
            return Ok(());
        };
        if self.coordination.pending_acknowledgements() != 0 {
            return Ok(());
        }
        let roots = self
            .major_root_snapshots
            .values()
            .flat_map(RootPublication::managed_references)
            .collect::<Vec<_>>();
        self.validate_major_references(&roots)?;
        self.coordination.finish_epoch(epoch)?;
        self.begin_major_references(roots)?;
        self.major_epoch = None;
        self.major_root_snapshots.clear();
        Ok(())
    }
}
