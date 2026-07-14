//! Native closed trap and panic-boundary termination exports.

/// Terminates native execution for a verified MIR trap edge.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_trap() -> ! {
    std::process::abort()
}

/// Terminates the process when a panic unwind reaches the native
/// runtime boundary. Typed expected failures do not use this path.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_continue_unwind() -> ! {
    std::process::abort()
}
