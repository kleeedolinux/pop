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

    #[allow(unsafe_code)]
    unsafe extern "C" {
        fn pop_rt_string_read(reference: u64, target: *mut u8, capacity: u64) -> u64;
    }

    /// Copies a bootstrap managed `String` through the trusted runtime ABI.
    #[must_use]
    #[allow(unsafe_code)]
    pub fn string_bytes(reference: u64) -> Option<Vec<u8>> {
        // SAFETY: A null target requests only the validated byte length.
        let encoded_length = unsafe { pop_rt_string_read(reference, std::ptr::null_mut(), 0) };
        let length = encoded_length.checked_sub(1)?;
        let length = usize::try_from(length).ok()?;
        let mut bytes = vec![0_u8; length];
        // SAFETY: `bytes` exposes exactly `length` writable bytes.
        let copied = unsafe {
            pop_rt_string_read(reference, bytes.as_mut_ptr(), u64::try_from(length).ok()?)
        };
        (copied == encoded_length).then_some(bytes)
    }
}
