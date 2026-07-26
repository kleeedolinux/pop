use std::collections::BTreeSet;

use pop_runtime_interface::RuntimeOperation;
use pop_runtime_native_abi::{
    ABI_SUPPORT_SYMBOL, ALLOCATE_INITIALIZED_OBJECT_AT_SITE_AND_STORE_ARRAY_SYMBOL,
    ALLOCATE_INITIALIZED_SELF_REFERENTIAL_OBJECT_AT_SITE_SYMBOL,
    ARRAY_GET_OBJECT_FIELD_CHECKED_SYMBOL, AllocationSiteDescriptorAbi, CodecEventStatus,
    CodecEventTag, CodecReadEventAbi, CodecWriteEventAbi, GC_SAFE_POINT_V2_SYMBOL, INVALID_HANDLE,
    ITERATION_MAKE_SYMBOL, NATIVE_ABI_1_VERSION, NATIVE_ABI_2_VERSION, symbol,
};

#[test]
fn abi_version_and_invalid_handle_are_explicit() {
    assert_eq!(NATIVE_ABI_1_VERSION.major(), 1);
    assert_eq!(NATIVE_ABI_1_VERSION.minor(), 22);
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
        RuntimeOperation::AllocateObjectInitializedAtSite,
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
        RuntimeOperation::FfiBytesBorrow,
        RuntimeOperation::FfiBytesEndBorrow,
        RuntimeOperation::FfiCallbackOpen,
        RuntimeOperation::FfiCallbackEnter,
        RuntimeOperation::FfiCallbackLeave,
        RuntimeOperation::FfiCallbackClose,
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
        RuntimeOperation::CodecWriteEvent,
        RuntimeOperation::CodecReadEvent,
    ];
    let symbols: BTreeSet<_> = operations
        .into_iter()
        .map(|operation| symbol(operation).expect("supported native operation"))
        .collect();
    assert_eq!(symbols.len(), operations.len());
    assert!(symbols.iter().all(|name| name.starts_with("pop_rt_")));
}

#[test]
fn codec_event_abi_has_closed_widths_and_statuses() {
    assert!(std::mem::size_of::<CodecWriteEventAbi>() > 0);
    assert!(std::mem::size_of::<CodecReadEventAbi>() > 0);
    assert_eq!(CodecEventStatus::from_raw(0), Some(CodecEventStatus::Ok));
    assert_eq!(
        CodecEventStatus::from_raw(3),
        Some(CodecEventStatus::CapabilityFailure)
    );
    assert_eq!(CodecEventStatus::from_raw(4), None);
    assert_eq!(CodecEventTag::from_raw(0), Some(CodecEventTag::RecordStart));
    assert_eq!(CodecEventTag::from_raw(26), Some(CodecEventTag::Bytes));
    assert_eq!(CodecEventTag::from_raw(27), None);
    assert_eq!(NATIVE_ABI_1_VERSION.minor(), 22);
}

#[test]
fn allocation_site_descriptor_abi_is_fixed_width_and_has_one_symbol() {
    assert!(std::mem::size_of::<AllocationSiteDescriptorAbi>() > 0);
    assert_eq!(
        symbol(RuntimeOperation::AllocateObjectInitializedAtSite),
        Some("pop_rt_allocate_initialized_object_at_site")
    );
}

#[test]
fn adjacent_heap_fusion_symbols_are_exact_and_distinct() {
    assert_eq!(
        ALLOCATE_INITIALIZED_OBJECT_AT_SITE_AND_STORE_ARRAY_SYMBOL,
        "pop_rt_allocate_initialized_object_at_site_and_store_array"
    );
    assert_eq!(
        ARRAY_GET_OBJECT_FIELD_CHECKED_SYMBOL,
        "pop_rt_array_get_object_field_checked"
    );
    assert_ne!(
        ALLOCATE_INITIALIZED_OBJECT_AT_SITE_AND_STORE_ARRAY_SYMBOL,
        ARRAY_GET_OBJECT_FIELD_CHECKED_SYMBOL
    );
}

#[test]
fn abi_one_twenty_two_layout_symbols_are_exact_and_distinct() {
    assert_eq!(
        ALLOCATE_INITIALIZED_SELF_REFERENTIAL_OBJECT_AT_SITE_SYMBOL,
        "pop_rt_allocate_initialized_self_referential_object_at_site"
    );
    assert_eq!(ITERATION_MAKE_SYMBOL, "pop_rt_iteration_make");
    assert_ne!(
        ALLOCATE_INITIALIZED_SELF_REFERENTIAL_OBJECT_AT_SITE_SYMBOL,
        ITERATION_MAKE_SYMBOL
    );
}

#[test]
fn unsupported_operations_have_no_fallback_symbol() {
    assert_eq!(symbol(RuntimeOperation::DispatchCall), None);
    assert_eq!(symbol(RuntimeOperation::InitializeBubble), None);
}
