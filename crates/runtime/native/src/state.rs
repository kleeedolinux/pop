//! Process-global native bootstrap composition state.

use std::sync::{Mutex, OnceLock};

use pop_runtime_collector::BootstrapRuntime;

static ABI_RUNTIME: OnceLock<Mutex<BootstrapRuntime>> = OnceLock::new();

pub(crate) fn abi_runtime() -> &'static Mutex<BootstrapRuntime> {
    ABI_RUNTIME.get_or_init(|| Mutex::new(BootstrapRuntime::new()))
}
