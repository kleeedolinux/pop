//! Trusted Rust-side adapters for the reserved `Pop.Internal` Bubble.

pub mod runtime {
    use pop_runtime_interface::{
        GarbageCollectorContract, GarbageCollectorStage, RuntimeOperation,
    };

    #[must_use]
    pub const fn garbage_collector_stage() -> GarbageCollectorStage {
        GarbageCollectorContract::bootstrap_stage1().stage()
    }

    #[must_use]
    pub const fn runtime_symbol(operation: RuntimeOperation) -> &'static str {
        operation.abi_symbol()
    }
}
