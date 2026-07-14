use pop_runtime_collector::{
    CollectorPhase, EpochCoordinator, EpochCoordinatorConfig, EpochCoordinatorConfigError,
    EpochCoordinatorError, MutatorExecutionState, MutatorPublication,
};
use pop_runtime_interface::{ManagedReference, RootPublication, RootSlot, SafePointId, StackMap};

fn roots(id: u32, count: u32) -> RootPublication {
    RootPublication::new(
        StackMap::new(
            SafePointId::new(id),
            (0..count).map(RootSlot::new).collect(),
        )
        .expect("stack map"),
        (0..count)
            .map(|index| Some(ManagedReference::new(u64::from(index) + 1)))
            .collect(),
    )
    .expect("root publication")
}

fn publication(id: u32, roots: u32) -> MutatorPublication {
    MutatorPublication::new(&self::roots(id, roots), 128, 3, 2)
}

#[test]
fn coordinator_configuration_and_mutator_capacity_fail_closed() {
    assert_eq!(
        EpochCoordinatorConfig::new(0),
        Err(EpochCoordinatorConfigError::ZeroMutators)
    );
    let mut coordinator =
        EpochCoordinator::new(EpochCoordinatorConfig::new(1).expect("coordinator configuration"));
    coordinator
        .register_mutator(MutatorExecutionState::Managed)
        .expect("first mutator");
    assert_eq!(
        coordinator.register_mutator(MutatorExecutionState::Managed),
        Err(EpochCoordinatorError::MutatorCapacity)
    );
}

#[test]
fn epoch_finishes_only_after_each_managed_mutator_publishes_once() {
    let mut coordinator = EpochCoordinator::default();
    let first = coordinator
        .register_mutator(MutatorExecutionState::Managed)
        .expect("first mutator");
    let second = coordinator
        .register_mutator(MutatorExecutionState::Managed)
        .expect("second mutator");
    let epoch = coordinator
        .begin_epoch(CollectorPhase::Marking)
        .expect("mark epoch");
    assert_eq!(coordinator.pending_acknowledgements(), 2);

    let progress = coordinator
        .acknowledge(first, epoch, publication(1, 2))
        .expect("first acknowledgement");
    assert_eq!(progress.pending(), 1);
    assert!(!progress.complete());
    assert_eq!(
        coordinator.acknowledge(first, epoch, publication(1, 2)),
        Err(EpochCoordinatorError::AlreadyAcknowledged(first))
    );
    assert_eq!(
        coordinator.finish_epoch(epoch),
        Err(EpochCoordinatorError::AcknowledgementsPending(1))
    );

    assert!(
        coordinator
            .acknowledge(second, epoch, publication(2, 1))
            .expect("second acknowledgement")
            .complete()
    );
    coordinator.finish_epoch(epoch).expect("finish epoch");
    assert_eq!(coordinator.active_phase(), None);
    assert_eq!(
        coordinator
            .publication(first)
            .expect("first publication")
            .root_slots(),
        2
    );
}

#[test]
fn detached_and_handle_only_mutators_auto_ack_but_bounded_foreign_code_blocks() {
    let mut coordinator = EpochCoordinator::default();
    coordinator
        .register_mutator(MutatorExecutionState::Detached)
        .expect("detached mutator");
    coordinator
        .register_mutator(MutatorExecutionState::HandlesOnly)
        .expect("handle-only mutator");
    let foreign = coordinator
        .register_mutator(MutatorExecutionState::BoundedForeign)
        .expect("foreign mutator");
    let epoch = coordinator
        .begin_epoch(CollectorPhase::Marking)
        .expect("mark epoch");
    assert_eq!(coordinator.pending_acknowledgements(), 1);
    assert_eq!(
        coordinator.acknowledge(foreign, epoch, publication(3, 0)),
        Err(EpochCoordinatorError::MutatorCannotAcknowledge(foreign))
    );

    assert!(
        coordinator
            .transition_mutator(foreign, MutatorExecutionState::HandlesOnly)
            .expect("foreign call retains only registered handles")
            .complete()
    );
    coordinator.finish_epoch(epoch).expect("finish epoch");
    let telemetry = coordinator.telemetry();
    assert_eq!(telemetry.automatic_acknowledgements(), 3);
    assert_eq!(telemetry.blocked_foreign_polls(), 1);
}

#[test]
fn registration_and_unregistration_during_an_epoch_preserve_the_snapshot() {
    let mut coordinator = EpochCoordinator::default();
    let first = coordinator
        .register_mutator(MutatorExecutionState::Managed)
        .expect("first mutator");
    let epoch = coordinator
        .begin_epoch(CollectorPhase::Marking)
        .expect("mark epoch");
    let late = coordinator
        .register_mutator(MutatorExecutionState::Managed)
        .expect("late mutator joins current epoch");
    assert_eq!(coordinator.pending_acknowledgements(), 2);

    coordinator
        .unregister_mutator(first)
        .expect("unregister pending mutator");
    assert_eq!(coordinator.pending_acknowledgements(), 1);
    coordinator
        .acknowledge(late, epoch, publication(4, 0))
        .expect("late mutator acknowledges");
    coordinator.finish_epoch(epoch).expect("finish epoch");

    assert_eq!(
        coordinator.acknowledge(late, epoch, publication(4, 0)),
        Err(EpochCoordinatorError::NoActiveEpoch)
    );
}

#[test]
fn epoch_ids_phases_and_telemetry_advance_deterministically() {
    let mut coordinator = EpochCoordinator::default();
    let mutator = coordinator
        .register_mutator(MutatorExecutionState::Managed)
        .expect("mutator");
    let mark = coordinator
        .begin_epoch(CollectorPhase::Marking)
        .expect("mark epoch");
    coordinator
        .acknowledge(mutator, mark, publication(5, 1))
        .expect("mark acknowledgement");
    coordinator.finish_epoch(mark).expect("finish marking");
    let sweep = coordinator
        .begin_epoch(CollectorPhase::Sweeping)
        .expect("sweep epoch");
    assert!(sweep.raw() > mark.raw());
    assert_eq!(coordinator.active_phase(), Some(CollectorPhase::Sweeping));
    assert_eq!(
        coordinator.acknowledge(mutator, mark, publication(6, 1)),
        Err(EpochCoordinatorError::StaleEpoch {
            expected: sweep,
            found: mark,
        })
    );
    coordinator
        .acknowledge(mutator, sweep, publication(6, 1))
        .expect("sweep acknowledgement");
    coordinator.finish_epoch(sweep).expect("finish sweeping");

    let telemetry = coordinator.telemetry();
    assert_eq!(telemetry.epochs_requested(), 2);
    assert_eq!(telemetry.epochs_completed(), 2);
    assert_eq!(telemetry.acknowledgements(), 2);
    assert_eq!(telemetry.maximum_pending_acknowledgements(), 1);
    assert_eq!(telemetry.stale_epoch_polls(), 1);
}
