//! Versioned native C ABI vocabulary for PLRI operations.

mod symbol;
mod version;

pub use symbol::{TABLE_GET_CHECKED_SYMBOL, symbol};
pub use version::{
    ABI_SUPPORT_SYMBOL, GC_SAFE_POINT_V2_SYMBOL, INVALID_HANDLE, IterationCollectionKind,
    IterationStatus, NATIVE_ABI_1_VERSION, NATIVE_ABI_2_VERSION, NativeAbiVersion,
    NativeTaskStatus, StringFormatTag,
};
