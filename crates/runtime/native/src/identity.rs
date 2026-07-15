//! Native runtime ABI and collector-stage identity exports.

use pop_runtime_native_abi::NATIVE_ABI_1_VERSION;

/// C-compatible stable-token generational runtime identity.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_major() -> u16 {
    NATIVE_ABI_1_VERSION.major()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_minor() -> u16 {
    NATIVE_ABI_1_VERSION.minor()
}

/// Reports complete native-facade support for one exact ABI descriptor.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_supports_abi(major: u16, minor: u16) -> u8 {
    u8::from(
        major == NATIVE_ABI_1_VERSION.major()
            && matches!(minor, 11 | 12)
            && minor <= NATIVE_ABI_1_VERSION.minor(),
    )
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_gc_stage() -> u8 {
    2
}
