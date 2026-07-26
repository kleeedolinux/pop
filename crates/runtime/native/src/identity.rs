//! Native runtime ABI and collector-stage identity exports.

use pop_runtime_native_abi::{NATIVE_ABI_1_VERSION, NATIVE_ABI_2_VERSION};

/// C-compatible stable-token generational runtime identity.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_major() -> u16 {
    selected_abi().major()
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_abi_minor() -> u16 {
    selected_abi().minor()
}

/// Reports complete native-facade support for one exact ABI descriptor.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_supports_abi(major: u16, minor: u16) -> u8 {
    #[cfg(feature = "production-generational")]
    let supported = major == NATIVE_ABI_2_VERSION.major() && minor == NATIVE_ABI_2_VERSION.minor();
    #[cfg(not(feature = "production-generational"))]
    let supported = major == NATIVE_ABI_1_VERSION.major()
        && matches!(minor, 11..=22)
        && minor <= NATIVE_ABI_1_VERSION.minor();
    u8::from(supported)
}

#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_gc_stage() -> u8 {
    if cfg!(feature = "production-generational") {
        3
    } else {
        2
    }
}

const fn selected_abi() -> pop_runtime_native_abi::NativeAbiVersion {
    if cfg!(feature = "production-generational") {
        NATIVE_ABI_2_VERSION
    } else {
        NATIVE_ABI_1_VERSION
    }
}
