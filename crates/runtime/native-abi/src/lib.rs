//! Versioned native C ABI vocabulary for PLRI operations.

mod symbol;
mod version;

pub use symbol::{
    ALLOCATE_INITIALIZED_OBJECT_AT_SITE_AND_STORE_ARRAY_SYMBOL,
    ALLOCATE_INITIALIZED_SELF_REFERENTIAL_OBJECT_AT_SITE_SYMBOL,
    ARRAY_GET_OBJECT_FIELD_CHECKED_SYMBOL, ITERATION_MAKE_SYMBOL, TABLE_GET_CHECKED_SYMBOL, symbol,
};
pub use version::{
    ABI_SUPPORT_SYMBOL, AllocationSiteDescriptorAbi, CodecEventStatus, CodecEventTag,
    CodecReadEventAbi, CodecWriteEventAbi, GC_SAFE_POINT_V2_SYMBOL, INVALID_HANDLE,
    IterationCollectionKind, IterationStatus, NATIVE_ABI_1_VERSION, NATIVE_ABI_2_VERSION,
    NativeAbiVersion, NativeTaskStatus, StringFormatTag,
};
