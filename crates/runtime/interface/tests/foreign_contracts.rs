use pop_runtime_interface::{ForeignCallMode, ForeignTransitionId, RuntimeOperation};

#[test]
fn foreign_call_modes_are_closed_and_explicit() {
    assert_eq!(
        ForeignCallMode::from_raw(0),
        Some(ForeignCallMode::Blocking)
    );
    assert_eq!(
        ForeignCallMode::from_raw(1),
        Some(ForeignCallMode::BoundedNonblocking)
    );
    assert_eq!(ForeignCallMode::from_raw(2), None);
    assert_eq!(ForeignCallMode::Blocking.raw(), 0);
    assert_eq!(ForeignCallMode::BoundedNonblocking.raw(), 1);
}

#[test]
fn foreign_transition_id_is_a_distinct_nonzero_identity() {
    assert_eq!(ForeignTransitionId::new(0), None);
    let transition = ForeignTransitionId::new(17).expect("nonzero transition identity");
    assert_eq!(transition.raw(), 17);
}

#[test]
fn foreign_transitions_are_distinct_runtime_operations() {
    assert_ne!(
        RuntimeOperation::EnterForeign,
        RuntimeOperation::LeaveForeign
    );
    assert_ne!(
        RuntimeOperation::EnterForeign,
        RuntimeOperation::GcSafePoint
    );
}
