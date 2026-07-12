//! Native runtime ABI and collector-stage identity exports.

use pop_runtime_native_abi::NATIVE_ABI_VERSION;

/// C-compatible bootstrap runtime identity. The bootstrap collector is
/// intentionally versioned separately from the future production collector.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_major() -> u16 {
    NATIVE_ABI_VERSION.major()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_minor() -> u16 {
    NATIVE_ABI_VERSION.minor()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_gc_stage() -> u8 {
    1
}
