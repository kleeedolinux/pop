use pop_internal::NATIVE_EXPORTS;
use pop_internal::runtime::{garbage_collector_stage, runtime_symbol};
use pop_runtime_interface::{GarbageCollectorStage, RuntimeOperation};

#[test]
fn internal_runtime_adapter_exposes_only_typed_contract_facts() {
    assert!(NATIVE_EXPORTS.is_empty());
    assert_eq!(
        garbage_collector_stage(),
        GarbageCollectorStage::BootstrapPreciseStopTheWorld
    );
    assert_eq!(
        runtime_symbol(RuntimeOperation::GcSafePoint),
        "pop_rt_gc_safe_point"
    );
}
