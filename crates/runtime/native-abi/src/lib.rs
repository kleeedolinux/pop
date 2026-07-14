//! Versioned native C ABI vocabulary for PLRI operations.

mod symbol;
mod version;

pub use symbol::{TABLE_GET_CHECKED_SYMBOL, symbol};
pub use version::{
    INVALID_HANDLE, IterationCollectionKind, IterationStatus, NATIVE_ABI_VERSION, NativeAbiVersion,
    StringFormatTag,
};
