//! Deterministic coordinator state machine with no heap tracing in handshakes.

use std::collections::{BTreeMap, BTreeSet};

use super::model::{
    CollectorEpoch, CollectorPhase, EpochCoordinatorConfig, EpochCoordinatorError,
    EpochCoordinatorTelemetry, EpochProgress, MutatorExecutionState, MutatorId, MutatorPublication,
};

#[derive(Clone, Copy, Debug)]
struct MutatorRecord {
    state: MutatorExecutionState,
    acknowledged_epoch: Option<CollectorEpoch>,
}

#[derive(Clone, Copy, Debug)]
struct ActiveEpoch {
    id: CollectorEpoch,
    phase: CollectorPhase,
}

pub struct EpochCoordinator {
    config: EpochCoordinatorConfig,
    mutators: BTreeMap<MutatorId, MutatorRecord>,
    pending: BTreeSet<MutatorId>,
    publications: BTreeMap<MutatorId, MutatorPublication>,
    active: Option<ActiveEpoch>,
    next_mutator: u32,
    next_epoch: u64,
    telemetry: EpochCoordinatorTelemetry,
}

impl EpochCoordinator {
    #[must_use]
    pub fn new(config: EpochCoordinatorConfig) -> Self {
        Self {
            config,
            mutators: BTreeMap::new(),
            pending: BTreeSet::new(),
            publications: BTreeMap::new(),
            active: None,
            next_mutator: 1,
            next_epoch: 1,
            telemetry: EpochCoordinatorTelemetry::default(),
        }
    }

    /// Registers one typed mutator execution state.
    ///
    /// # Errors
    ///
    /// Rejects capacity or typed-ID exhaustion.
    pub fn register_mutator(
        &mut self,
        state: MutatorExecutionState,
    ) -> Result<MutatorId, EpochCoordinatorError> {
        if self.mutators.len() >= self.config.maximum_mutators {
            return Err(EpochCoordinatorError::MutatorCapacity);
        }
        let id = MutatorId(self.next_mutator);
        self.next_mutator = self
            .next_mutator
            .checked_add(1)
            .ok_or(EpochCoordinatorError::MutatorIdentityOverflow)?;
        let acknowledged_epoch = self
            .active
            .and_then(|active| state.acknowledges_without_poll().then_some(active.id));
        self.mutators.insert(
            id,
            MutatorRecord {
                state,
                acknowledged_epoch,
            },
        );
        if let Some(active) = self.active {
            if acknowledged_epoch == Some(active.id) {
                self.telemetry.automatic_acknowledgements =
                    self.telemetry.automatic_acknowledgements.saturating_add(1);
            } else {
                self.pending.insert(id);
                self.record_maximum_pending();
            }
        }
        self.telemetry.mutators_registered = self.telemetry.mutators_registered.saturating_add(1);
        Ok(id)
    }

    /// Removes one mutator and its pending acknowledgement/publication.
    ///
    /// # Errors
    ///
    /// Rejects an unknown typed mutator ID.
    pub fn unregister_mutator(&mut self, id: MutatorId) -> Result<(), EpochCoordinatorError> {
        self.mutators
            .remove(&id)
            .ok_or(EpochCoordinatorError::UnknownMutator(id))?;
        self.pending.remove(&id);
        self.publications.remove(&id);
        self.telemetry.mutators_unregistered =
            self.telemetry.mutators_unregistered.saturating_add(1);
        Ok(())
    }

    /// Starts one collector protocol epoch without tracing heap work.
    ///
    /// # Errors
    ///
    /// Rejects overlapping epochs or ID exhaustion.
    pub fn begin_epoch(
        &mut self,
        phase: CollectorPhase,
    ) -> Result<CollectorEpoch, EpochCoordinatorError> {
        if let Some(active) = self.active {
            return Err(EpochCoordinatorError::EpochAlreadyActive(active.id));
        }
        let epoch = CollectorEpoch(self.next_epoch);
        self.next_epoch = self
            .next_epoch
            .checked_add(1)
            .ok_or(EpochCoordinatorError::EpochOverflow)?;
        self.pending.clear();
        for (id, mutator) in &mut self.mutators {
            if mutator.state.acknowledges_without_poll() {
                mutator.acknowledged_epoch = Some(epoch);
                self.telemetry.automatic_acknowledgements =
                    self.telemetry.automatic_acknowledgements.saturating_add(1);
            } else {
                mutator.acknowledged_epoch = None;
                self.pending.insert(*id);
            }
        }
        self.active = Some(ActiveEpoch { id: epoch, phase });
        self.telemetry.epochs_requested = self.telemetry.epochs_requested.saturating_add(1);
        self.record_maximum_pending();
        Ok(epoch)
    }

    /// Publishes bounded per-mutator protocol state and acknowledges an epoch.
    ///
    /// # Errors
    ///
    /// Rejects stale/duplicate polls, unknown mutators, and foreign states that
    /// cannot publish managed roots.
    pub fn acknowledge(
        &mut self,
        id: MutatorId,
        epoch: CollectorEpoch,
        publication: MutatorPublication,
    ) -> Result<EpochProgress, EpochCoordinatorError> {
        let active = self.active.ok_or(EpochCoordinatorError::NoActiveEpoch)?;
        if active.id != epoch {
            self.telemetry.stale_epoch_polls = self.telemetry.stale_epoch_polls.saturating_add(1);
            return Err(EpochCoordinatorError::StaleEpoch {
                expected: active.id,
                found: epoch,
            });
        }
        let mutator = self
            .mutators
            .get_mut(&id)
            .ok_or(EpochCoordinatorError::UnknownMutator(id))?;
        if !self.pending.contains(&id) || mutator.acknowledged_epoch == Some(epoch) {
            return Err(EpochCoordinatorError::AlreadyAcknowledged(id));
        }
        if !mutator.state.can_publish() {
            if mutator.state == MutatorExecutionState::BoundedForeign {
                self.telemetry.blocked_foreign_polls =
                    self.telemetry.blocked_foreign_polls.saturating_add(1);
            }
            return Err(EpochCoordinatorError::MutatorCannotAcknowledge(id));
        }
        mutator.acknowledged_epoch = Some(epoch);
        self.pending.remove(&id);
        self.publications.insert(id, publication);
        self.telemetry.acknowledgements = self.telemetry.acknowledgements.saturating_add(1);
        Ok(self.progress())
    }

    /// Changes one foreign/managed transition state while preserving the
    /// active epoch's acknowledgement requirement.
    ///
    /// # Errors
    ///
    /// Rejects an unknown typed mutator ID.
    pub fn transition_mutator(
        &mut self,
        id: MutatorId,
        state: MutatorExecutionState,
    ) -> Result<EpochProgress, EpochCoordinatorError> {
        let active = self.active;
        let mutator = self
            .mutators
            .get_mut(&id)
            .ok_or(EpochCoordinatorError::UnknownMutator(id))?;
        let previous = mutator.state;
        mutator.state = state;
        let mut pending_grew = false;
        if let Some(active) = active {
            if state.acknowledges_without_poll() {
                if self.pending.remove(&id) {
                    self.telemetry.automatic_acknowledgements =
                        self.telemetry.automatic_acknowledgements.saturating_add(1);
                }
                mutator.acknowledged_epoch = Some(active.id);
            } else if previous.acknowledges_without_poll() {
                mutator.acknowledged_epoch = None;
                self.pending.insert(id);
                pending_grew = true;
            }
        }
        if pending_grew {
            self.record_maximum_pending();
        }
        Ok(self.progress())
    }

    /// Completes an acknowledged epoch without doing heap-sized work.
    ///
    /// # Errors
    ///
    /// Rejects absent/stale epochs and outstanding mutators.
    pub fn finish_epoch(&mut self, epoch: CollectorEpoch) -> Result<(), EpochCoordinatorError> {
        let active = self.active.ok_or(EpochCoordinatorError::NoActiveEpoch)?;
        if active.id != epoch {
            return Err(EpochCoordinatorError::StaleEpoch {
                expected: active.id,
                found: epoch,
            });
        }
        if !self.pending.is_empty() {
            return Err(EpochCoordinatorError::AcknowledgementsPending(
                self.pending.len(),
            ));
        }
        self.active = None;
        self.telemetry.epochs_completed = self.telemetry.epochs_completed.saturating_add(1);
        Ok(())
    }

    #[must_use]
    pub fn pending_acknowledgements(&self) -> usize {
        self.pending.len()
    }

    #[must_use]
    pub fn registered_mutators(&self) -> usize {
        self.mutators.len()
    }

    #[must_use]
    pub fn active_phase(&self) -> Option<CollectorPhase> {
        self.active.map(|active| active.phase)
    }

    #[must_use]
    pub fn publication(&self, id: MutatorId) -> Option<MutatorPublication> {
        self.publications.get(&id).copied()
    }

    #[must_use]
    pub const fn telemetry(&self) -> EpochCoordinatorTelemetry {
        self.telemetry
    }

    fn progress(&self) -> EpochProgress {
        EpochProgress {
            pending: self.pending.len(),
        }
    }

    fn record_maximum_pending(&mut self) {
        self.telemetry.maximum_pending_acknowledgements = self
            .telemetry
            .maximum_pending_acknowledgements
            .max(self.pending.len());
    }
}

impl Default for EpochCoordinator {
    fn default() -> Self {
        Self::new(EpochCoordinatorConfig::default())
    }
}
