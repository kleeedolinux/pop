use pop_runtime_interface::{
    ErrorContract, GarbageCollectorContract, InitializationState, PinHandle, PlriVersion,
};

#[test]
fn plri_version_is_explicit_and_ordered() {
    assert!(PlriVersion::new(1, 1) > PlriVersion::new(1, 0));
    assert!(PlriVersion::new(1, 2) > PlriVersion::new(1, 1));
    assert!(PlriVersion::new(1, 3) > PlriVersion::new(1, 2));
    assert!(PlriVersion::new(1, 4) > PlriVersion::new(1, 3));
    assert!(PlriVersion::new(2, 0) > PlriVersion::new(1, 99));
}

#[test]
fn pin_handles_are_runtime_private_opaque_tokens() {
    let pin = PinHandle::new(7);

    assert_eq!(pin.raw(), 7);
}

#[test]
fn pop_gc_contract_has_precise_generational_concurrent_invariants() {
    let gc = GarbageCollectorContract::pop_v1();

    assert!(gc.precise_roots());
    assert!(gc.moving_nursery());
    assert!(gc.mostly_non_moving_mature_heap());
    assert!(gc.concurrent_mature_marking());
    assert!(gc.satb_barrier());
    assert!(gc.generational_card_barrier());
    assert!(!gc.user_finalizers());
    assert!(!gc.weak_references());
    assert!(!gc.conservative_scanning());
}

#[test]
fn expected_failures_are_typed_results_not_exceptions() {
    let errors = ErrorContract::pop_v1();

    assert!(errors.uses_typed_results());
    assert!(errors.panics_unwind());
    assert!(!errors.exceptions_are_ordinary_errors());
}

#[test]
fn bubble_initialization_state_machine_rejects_cycles_and_retries() {
    assert!(InitializationState::Unloaded.can_transition_to(InitializationState::Loading));
    assert!(InitializationState::Loading.can_transition_to(InitializationState::Loaded));
    assert!(InitializationState::Loaded.can_transition_to(InitializationState::Initializing));
    assert!(InitializationState::Initializing.can_transition_to(InitializationState::Ready));
    assert!(InitializationState::Initializing.can_transition_to(InitializationState::Failed));
    assert!(!InitializationState::Initializing.can_transition_to(InitializationState::Loading));
    assert!(!InitializationState::Failed.can_transition_to(InitializationState::Loading));
}
