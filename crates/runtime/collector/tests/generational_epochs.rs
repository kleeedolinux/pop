use pop_runtime_collector::{
    BackgroundWorkerConfig, CollectorPhase, EpochCoordinator, EpochCoordinatorConfig,
    EpochCoordinatorConfigError, EpochCoordinatorError, GenerationalRuntime,
    MajorCollectionHandshakeError, MajorCyclePhase, MutatorExecutionState, MutatorPublication,
    SchedulerId, StableGenerationalRuntime,
};
use pop_runtime_interface::{
    AllocationClass, ManagedReference, ObjectAllocationRequest, ObjectMap, RootPublication,
    RootSlot, RuntimeAdapter, RuntimeTypeId, SafePointId, StackMap,
};

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

fn one_root(id: u32, reference: ManagedReference) -> RootPublication {
    RootPublication::new(
        StackMap::new(SafePointId::new(id), vec![RootSlot::new(0)]).expect("stack map"),
        vec![Some(reference)],
    )
    .expect("root publication")
}

fn empty_roots(id: u32) -> RootPublication {
    roots(id, 0)
}

fn mature_object() -> ObjectAllocationRequest {
    ObjectAllocationRequest::new(
        RuntimeTypeId::new(71),
        AllocationClass::Mature,
        ObjectMap::new(0, Vec::new()).expect("object map"),
    )
}

fn finish_major(runtime: &mut GenerationalRuntime, roots: &mut RootPublication) {
    for _ in 0..64 {
        if runtime
            .safe_point(roots)
            .expect("major collection slice")
            .collection()
            .is_some()
        {
            return;
        }
    }
    panic!("major collection did not finish within its deterministic work bound");
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

#[test]
fn registered_mutators_gate_major_workers_until_every_root_snapshot_is_published() {
    let mut runtime = GenerationalRuntime::with_background_workers(
        BackgroundWorkerConfig::new(2, 2).expect("worker configuration"),
    )
    .expect("background workers");
    let first_object = runtime
        .allocate_object(&mature_object())
        .expect("first mature object");
    let second_object = runtime
        .allocate_object(&mature_object())
        .expect("second mature object");
    let first = runtime
        .register_mutator(MutatorExecutionState::Managed)
        .expect("first mutator");
    let second = runtime
        .register_mutator(MutatorExecutionState::Managed)
        .expect("second mutator");
    runtime.request_major_collection();

    let epoch = runtime
        .begin_major_collection_handshake()
        .expect("begin major handshake");
    assert_eq!(runtime.active_major_collection_epoch(), Some(epoch));
    assert_eq!(runtime.pending_major_acknowledgements(), 2);
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Idle);

    let progress = runtime
        .acknowledge_major_collection_handshake(first, epoch, &one_root(20, first_object))
        .expect("first mutator acknowledgement");
    assert_eq!(progress.pending(), 1);
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Idle);
    assert_eq!(
        runtime
            .background_worker_telemetry()
            .expect("worker telemetry")
            .jobs_submitted(),
        0
    );

    let progress = runtime
        .acknowledge_major_collection_handshake(second, epoch, &one_root(21, second_object))
        .expect("second mutator acknowledgement");
    assert!(progress.complete());
    assert_eq!(runtime.active_major_collection_epoch(), None);
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Marking);
    assert_eq!(
        runtime
            .mutator_publication(first)
            .expect("first publication")
            .managed_roots(),
        1
    );

    finish_major(&mut runtime, &mut empty_roots(22));
    assert!(runtime.contains(first_object));
    assert!(runtime.contains(second_object));
    let telemetry = runtime.epoch_coordinator_telemetry();
    assert_eq!(telemetry.epochs_requested(), 1);
    assert_eq!(telemetry.epochs_completed(), 1);
    assert!(
        runtime
            .background_worker_telemetry()
            .expect("worker telemetry")
            .jobs_completed()
            > 0
    );
}

#[test]
fn invalid_registered_root_fails_before_acknowledgement_or_worker_dispatch() {
    let mut runtime = GenerationalRuntime::with_background_workers(
        BackgroundWorkerConfig::new(2, 1).expect("worker configuration"),
    )
    .expect("background workers");
    let mutator = runtime
        .register_mutator(MutatorExecutionState::Managed)
        .expect("managed mutator");
    runtime.request_major_collection();
    let epoch = runtime
        .begin_major_collection_handshake()
        .expect("begin major handshake");
    let stale = one_root(23, ManagedReference::new(u64::MAX));

    assert!(matches!(
        runtime.acknowledge_major_collection_handshake(mutator, epoch, &stale),
        Err(MajorCollectionHandshakeError::Runtime(_))
    ));
    assert_eq!(runtime.pending_major_acknowledgements(), 1);
    assert_eq!(runtime.active_major_collection_epoch(), Some(epoch));
    assert_eq!(runtime.major_phase(), MajorCyclePhase::Idle);
    assert_eq!(
        runtime
            .background_worker_telemetry()
            .expect("worker telemetry")
            .jobs_submitted(),
        0
    );
}

#[test]
fn stable_scheduler_safe_point_acknowledges_each_epoch_at_most_once() {
    let first_scheduler = SchedulerId::new(1);
    let second_scheduler = SchedulerId::new(2);
    let mut runtime = StableGenerationalRuntime::new();
    let first = runtime
        .register_scheduler_mutator(first_scheduler, MutatorExecutionState::Managed)
        .expect("first scheduler mutator");
    let second = runtime
        .register_scheduler_mutator(second_scheduler, MutatorExecutionState::Managed)
        .expect("second scheduler mutator");
    runtime.request_collection();

    let (_, first_acknowledged) = runtime
        .scheduler_mutator_safe_point(first, first_scheduler, &mut empty_roots(24))
        .expect("first managed safe point");
    assert!(first_acknowledged);
    let (_, duplicate_acknowledged) = runtime
        .scheduler_mutator_safe_point(first, first_scheduler, &mut empty_roots(25))
        .expect("duplicate epoch poll is a successful no-op");
    assert!(!duplicate_acknowledged);
    assert_eq!(runtime.epoch_coordinator_telemetry().acknowledgements(), 1);

    runtime
        .transition_scheduler_mutator(second, second_scheduler, MutatorExecutionState::Detached)
        .expect("detached peer automatically acknowledges");
    assert_eq!(runtime.epoch_coordinator_telemetry().epochs_completed(), 1);
    runtime
        .unregister_scheduler_mutator(first, first_scheduler)
        .expect("unregister first mutator");
    runtime
        .unregister_scheduler_mutator(second, second_scheduler)
        .expect("unregister second mutator");
}
