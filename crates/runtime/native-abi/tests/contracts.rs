use std::collections::BTreeSet;

use pop_runtime_interface::RuntimeOperation;
use pop_runtime_native_abi::{INVALID_HANDLE, NATIVE_ABI_VERSION, symbol};

#[test]
fn abi_version_and_invalid_handle_are_explicit() {
    assert_eq!(NATIVE_ABI_VERSION.major(), 1);
    assert_eq!(NATIVE_ABI_VERSION.minor(), 7);
    assert_eq!(INVALID_HANDLE, 0);
}

#[test]
fn supported_symbols_are_unique_and_native() {
    let operations = [
        RuntimeOperation::AllocateObject,
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
        RuntimeOperation::FieldGet,
        RuntimeOperation::FieldSet,
        RuntimeOperation::StringConcat,
        RuntimeOperation::StringFormat,
        RuntimeOperation::RetainRoot,
        RuntimeOperation::ReleaseRoot,
        RuntimeOperation::Pin,
        RuntimeOperation::Unpin,
        RuntimeOperation::GcSafePoint,
        RuntimeOperation::SatbWriteBarrier,
        RuntimeOperation::Trap,
        RuntimeOperation::ContinueUnwind,
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
