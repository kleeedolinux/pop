use std::collections::BTreeSet;

use pop_runtime_interface::RuntimeOperation;
use pop_runtime_native_abi::{
    ABI_SUPPORT_SYMBOL, GC_SAFE_POINT_V2_SYMBOL, INVALID_HANDLE, NATIVE_ABI_1_VERSION,
    NATIVE_ABI_2_VERSION, symbol,
};

#[test]
fn abi_version_and_invalid_handle_are_explicit() {
    assert_eq!(NATIVE_ABI_1_VERSION.major(), 1);
    assert_eq!(NATIVE_ABI_1_VERSION.minor(), 16);
    assert_eq!(NATIVE_ABI_2_VERSION.major(), 2);
    assert_eq!(NATIVE_ABI_2_VERSION.minor(), 0);
    assert_ne!(NATIVE_ABI_1_VERSION, NATIVE_ABI_2_VERSION);
    assert_eq!(ABI_SUPPORT_SYMBOL, "pop_rt_supports_abi");
    assert_eq!(GC_SAFE_POINT_V2_SYMBOL, "pop_rt_gc_safe_point_v2");
    assert_eq!(INVALID_HANDLE, 0);
}

#[test]
fn supported_symbols_are_unique_and_native() {
    let operations = [
        RuntimeOperation::AllocateObject,
        RuntimeOperation::AllocateObjectInitialized,
        RuntimeOperation::AllocateArray,
        RuntimeOperation::AllocateArrayFilled,
        RuntimeOperation::AllocateTable,
        RuntimeOperation::TupleMake,
        RuntimeOperation::TableGet,
        RuntimeOperation::TableSet,
        RuntimeOperation::ArrayGet,
        RuntimeOperation::ArrayLength,
        RuntimeOperation::ArrayGetChecked,
        RuntimeOperation::ArraySet,
        RuntimeOperation::ArrayFill,
        RuntimeOperation::ListCreate,
        RuntimeOperation::ListLength,
        RuntimeOperation::ListGet,
        RuntimeOperation::ListGetChecked,
        RuntimeOperation::ListSet,
        RuntimeOperation::ListAdd,
        RuntimeOperation::RangeCreate,
        RuntimeOperation::IterationAcquire,
        RuntimeOperation::IterationNext,
        RuntimeOperation::FieldGet,
        RuntimeOperation::FieldSet,
        RuntimeOperation::StringConcat,
        RuntimeOperation::StringFormat,
        RuntimeOperation::FfiBufferOpen,
        RuntimeOperation::FfiBufferLength,
        RuntimeOperation::FfiBufferRead,
        RuntimeOperation::FfiBufferWrite,
        RuntimeOperation::FfiBufferBorrow,
        RuntimeOperation::FfiBufferEndBorrow,
        RuntimeOperation::FfiBufferClose,
        RuntimeOperation::RetainRoot,
        RuntimeOperation::ResolveRoot,
        RuntimeOperation::ReleaseRoot,
        RuntimeOperation::Pin,
        RuntimeOperation::Unpin,
        RuntimeOperation::AttachManagedThread,
        RuntimeOperation::DetachManagedThread,
        RuntimeOperation::EnterForeign,
        RuntimeOperation::LeaveForeign,
        RuntimeOperation::GcSafePoint,
        RuntimeOperation::SatbWriteBarrier,
        RuntimeOperation::Trap,
        RuntimeOperation::ContinueUnwind,
        RuntimeOperation::CancelSourceCreate,
        RuntimeOperation::CancelSourceToken,
        RuntimeOperation::CancelSourceRelease,
        RuntimeOperation::CancelTokenRelease,
        RuntimeOperation::TaskFrameCreate,
        RuntimeOperation::TaskFrameRelease,
        RuntimeOperation::TaskFrameLoad,
        RuntimeOperation::TaskFrameStore,
        RuntimeOperation::TaskFrameSetLiveMap,
        RuntimeOperation::TaskCreate,
        RuntimeOperation::TaskStartDirect,
        RuntimeOperation::TaskStartGroup,
        RuntimeOperation::TaskAwait,
        RuntimeOperation::TaskCompletionStore,
        RuntimeOperation::TaskRelease,
        RuntimeOperation::TaskGroupCreate,
        RuntimeOperation::TaskGroupWrap,
        RuntimeOperation::TaskGroupClose,
        RuntimeOperation::TaskGroupJoin,
        RuntimeOperation::Suspend,
        RuntimeOperation::Resume,
        RuntimeOperation::TaskCancel,
        RuntimeOperation::TaskCancellationRequested,
    ];
    let symbols: BTreeSet<_> = operations
        .into_iter()
        .map(|operation| symbol(operation).expect("supported native operation"))
        .collect();
    assert_eq!(symbols.len(), operations.len());
    assert!(symbols.iter().all(|name| name.starts_with("pop_rt_")));
}

#[test]
fn unsupported_operations_have_no_fallback_symbol() {
    assert_eq!(symbol(RuntimeOperation::DispatchCall), None);
    assert_eq!(symbol(RuntimeOperation::InitializeBubble), None);
}
