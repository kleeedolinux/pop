//! Versioned native C ABI vocabulary for PLRI operations.

mod symbol;
mod version;

pub use symbol::symbol;
pub use version::{INVALID_HANDLE, NATIVE_ABI_VERSION, NativeAbiVersion};
