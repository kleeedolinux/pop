use pop_library_bridge::{FoundationBubble, NativeEffect, PopAbiType, poplib};

#[poplib(
    bubble = Standard,
    namespace = "Pop.Math",
    name = "contributorIdentity",
    parameters(Int),
    results(Int),
    effects(),
)]
pub extern "C" fn contributor_identity(value: i64) -> i64 {
    value
}

#[poplib(
    bubble = Internal,
    namespace = "Pop.Internal.Gc",
    name = "Gc.SafePoint",
    parameters(ManagedReference),
    results(),
    effects(ForeignFunction, GcSafePoint),
)]
pub extern "C" fn internal_safe_point(_reference: u64) {}

#[poplib(
    bubble = Internal,
    namespace = "Pop.Internal.Task",
    name = "Task.BlockingPoll",
    parameters(),
    results(),
    effects(Blocks),
)]
pub extern "C" fn internal_blocking_poll() {}

#[test]
fn standard_descriptor_preserves_the_typed_binding_and_abi() {
    let export = CONTRIBUTOR_IDENTITY_POPLIB_EXPORT;
    assert_eq!(contributor_identity(41), 41);
    assert_eq!(export.bubble(), FoundationBubble::Standard);
    assert_eq!(export.namespace(), "Pop.Math");
    assert_eq!(export.name(), "contributorIdentity");
    assert_eq!(export.native_symbol(), "contributor_identity");
    assert_eq!(export.parameters(), &[PopAbiType::Int]);
    assert_eq!(export.results(), &[PopAbiType::Int]);
    assert!(export.effects().is_empty());
}

#[test]
fn internal_descriptor_uses_the_same_closed_contract() {
    let export = INTERNAL_SAFE_POINT_POPLIB_EXPORT;
    internal_safe_point(7);
    assert_eq!(export.bubble(), FoundationBubble::Internal);
    assert_eq!(export.namespace(), "Pop.Internal.Gc");
    assert_eq!(export.name(), "Gc.SafePoint");
    assert_eq!(export.parameters(), &[PopAbiType::ManagedReference]);
    assert!(export.results().is_empty());
    assert_eq!(
        export.effects(),
        &[NativeEffect::ForeignFunction, NativeEffect::GcSafePoint]
    );
}

#[test]
fn native_descriptor_keeps_blocking_distinct_from_suspension() {
    let export = INTERNAL_BLOCKING_POLL_POPLIB_EXPORT;
    internal_blocking_poll();
    assert_eq!(export.bubble(), FoundationBubble::Internal);
    assert_eq!(export.namespace(), "Pop.Internal.Task");
    assert_eq!(export.name(), "Task.BlockingPoll");
    assert!(export.parameters().is_empty());
    assert!(export.results().is_empty());
    assert_eq!(export.effects(), &[NativeEffect::Blocks]);
}
