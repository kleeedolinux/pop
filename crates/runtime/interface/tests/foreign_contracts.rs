use pop_runtime_interface::{
    FfiCallbackLifetime, FfiCallbackRegistrationId, FfiCallbackSiteId, FfiCallbackThread,
    FfiCallbackTransitionId, ForeignCallMode, ForeignTransitionId, ManagedThreadBindingId,
    RuntimeOperation,
};

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
fn managed_thread_binding_id_is_distinct_and_nonzero() {
    assert_eq!(ManagedThreadBindingId::new(0), None);
    let binding = ManagedThreadBindingId::new(23).expect("nonzero managed binding identity");
    assert_eq!(binding.raw(), 23);
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
    assert_ne!(
        RuntimeOperation::AttachManagedThread,
        RuntimeOperation::DetachManagedThread
    );
}

#[test]
fn callback_contracts_are_closed_and_use_distinct_nonzero_identities() {
    assert_eq!(
        FfiCallbackLifetime::from_raw(0),
        Some(FfiCallbackLifetime::CallScoped)
    );
    assert_eq!(
        FfiCallbackLifetime::from_raw(1),
        Some(FfiCallbackLifetime::Registered)
    );
    assert_eq!(FfiCallbackLifetime::from_raw(2), None);
    assert_eq!(
        FfiCallbackThread::from_raw(0),
        Some(FfiCallbackThread::CallingThread)
    );
    assert_eq!(
        FfiCallbackThread::from_raw(1),
        Some(FfiCallbackThread::AttachedThread)
    );
    assert_eq!(FfiCallbackThread::from_raw(2), None);

    assert_eq!(FfiCallbackSiteId::new(0), None);
    assert_eq!(FfiCallbackRegistrationId::new(0), None);
    assert_eq!(FfiCallbackTransitionId::new(0), None);
    assert_eq!(FfiCallbackSiteId::new(11).expect("site").raw(), 11);
    assert_eq!(
        FfiCallbackRegistrationId::new(12)
            .expect("registration")
            .raw(),
        12
    );
    assert_eq!(
        FfiCallbackTransitionId::new(13).expect("transition").raw(),
        13
    );
}

#[test]
fn callback_runtime_operations_have_no_dynamic_fallback() {
    let operations = [
        RuntimeOperation::FfiCallbackOpen,
        RuntimeOperation::FfiCallbackEnter,
        RuntimeOperation::FfiCallbackLeave,
        RuntimeOperation::FfiCallbackClose,
    ];
    for (index, operation) in operations.into_iter().enumerate() {
        assert!(
            operations
                .iter()
                .skip(index + 1)
                .all(|other| operation != *other)
        );
        assert_ne!(operation, RuntimeOperation::DispatchCall);
    }
}
