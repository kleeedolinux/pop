use std::io::Write;

use pop_library_bridge::poplib;

/// Prints one Pop `Int` followed by a newline for the native bootstrap host.
///
/// This fixed ABI adapter is linked by the toolchain and is not resolved from
/// user source by symbol spelling.
#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "print",
    parameters(Int),
    results(),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_print_int(value: i64) {
    let _ = writeln!(std::io::stdout().lock(), "{value}");
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "print",
    parameters(UInt64),
    results(),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_print_uint64(value: u64) {
    let _ = writeln!(std::io::stdout().lock(), "{value}");
}

#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "print",
    parameters(Boolean),
    results(),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_print_boolean(value: bool) {
    let text = if value { "true" } else { "false" };
    let _ = writeln!(std::io::stdout().lock(), "{text}");
}

/// Prints one already validated Pop `String` followed by a newline.
pub fn print_string(value: &str) {
    let mut output = std::io::stdout().lock();
    let _ = output.write_all(value.as_bytes());
    let _ = output.write_all(b"\n");
}

/// Prints one managed Pop `String` followed by a newline for the native
/// bootstrap host.
///
/// This fixed ABI adapter is linked by the toolchain and is not resolved from
/// user source by symbol spelling.
#[poplib(
    bubble = Standard,
    namespace = "Pop",
    name = "print",
    parameters(String),
    results(),
    effects(AmbientIo),
)]
pub extern "C" fn pop_std_print_string(reference: u64) {
    let Some(bytes) = pop_internal::runtime::string_bytes(reference) else {
        return;
    };
    let Ok(value) = std::str::from_utf8(&bytes) else {
        return;
    };
    print_string(value);
}
