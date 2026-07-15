use pop_backend_mir_interp::{MirInterpreter, MirValue, ReferenceRuntimeAdapter};
use pop_driver::artifact_sha256_hex;
use pop_foundation::{BuiltinTypeId, SymbolId};
use pop_mir::{MirFfiLayout, MirFfiLayoutCatalog, MirFfiValueClass, parse_mir_dump};
use pop_runtime_interface::{
    FfiAbiLayoutId, FfiBufferOpenFailure, FfiBufferOpenRequest, ForeignAddress, RuntimeAdapter,
};
use pop_target::TargetSpec;
use pop_types::{
    FFI_BUFFER_TYPE_ID, FFI_OPTIONAL_POINTER_TYPE_ID, FFI_POINTER_TYPE_ID,
    FFI_READ_ONLY_POINTER_TYPE_ID, IntegerKind, IntegerValue, SemanticType, TypeArena,
};

fn layout(raw: u64) -> FfiAbiLayoutId {
    FfiAbiLayoutId::new(raw).expect("nonzero layout")
}

#[test]
fn reference_buffers_are_zeroed_bounded_borrowed_and_deterministically_closed() {
    let mut runtime = ReferenceRuntimeAdapter::default();
    let request = FfiBufferOpenRequest::new(2, 4, 4, layout(7)).expect("valid layout");
    let buffer = runtime.ffi_buffer_open(&request).expect("buffer");
    assert_eq!(runtime.ffi_buffer_length(buffer, layout(7)), Ok(2));

    let mut element = [9_u8; 4];
    runtime
        .ffi_buffer_read(buffer, layout(7), 1, &mut element)
        .expect("zeroed read");
    assert_eq!(element, [0; 4]);
    runtime
        .ffi_buffer_write(buffer, layout(7), 2, &[1, 2, 3, 4])
        .expect("write");
    runtime
        .ffi_buffer_read(buffer, layout(7), 2, &mut element)
        .expect("read");
    assert_eq!(element, [1, 2, 3, 4]);

    let before = element;
    assert!(
        runtime
            .ffi_buffer_read(buffer, layout(7), 0, &mut element)
            .is_err()
    );
    assert_eq!(element, before, "failed reads are output-atomic");
    assert!(
        runtime
            .ffi_buffer_read(buffer, layout(7), 3, &mut element)
            .is_err()
    );
    assert!(runtime.ffi_buffer_length(buffer, layout(8)).is_err());

    let borrow = runtime
        .ffi_buffer_borrow(buffer, layout(7))
        .expect("borrow");
    assert!(borrow.address().is_some());
    assert_eq!(borrow.address().expect("address").raw() % 4, 0);
    assert_eq!(borrow.length(), 2);
    assert!(runtime.ffi_buffer_close(buffer).is_err());
    runtime
        .ffi_buffer_end_borrow(buffer, borrow.id())
        .expect("end borrow");
    runtime.ffi_buffer_close(buffer).expect("close");
    runtime.ffi_buffer_close(buffer).expect("idempotent close");
    assert!(runtime.ffi_buffer_length(buffer, layout(7)).is_err());
}

#[test]
fn zero_length_and_allocation_failure_remain_distinct() {
    let mut runtime = ReferenceRuntimeAdapter::default();
    let zero = runtime
        .ffi_buffer_open(&FfiBufferOpenRequest::new(0, 8, 8, layout(1)).expect("zero request"))
        .expect("zero buffer");
    let borrow = runtime
        .ffi_buffer_borrow(zero, layout(1))
        .expect("zero borrow");
    assert_eq!(borrow.address(), None);
    assert_eq!(borrow.length(), 0);
    runtime
        .ffi_buffer_end_borrow(zero, borrow.id())
        .expect("end zero borrow");
    runtime.ffi_buffer_close(zero).expect("close zero");

    let huge = FfiBufferOpenRequest::new(u64::MAX, 1, 1, layout(2)).expect("valid geometry");
    assert_eq!(
        runtime.ffi_buffer_open(&huge),
        Err(FfiBufferOpenFailure::Allocation)
    );
    assert!(FfiBufferOpenRequest::new(u64::MAX, 2, 2, layout(3)).is_err());
    assert!(FfiBufferOpenRequest::new(1, 1, 3, layout(3)).is_err());
}

#[test]
fn reference_unsafe_memory_requires_active_borrow_and_preserves_overlap() {
    let mut runtime = ReferenceRuntimeAdapter::default();
    let request = FfiBufferOpenRequest::new(4, 1, 1, layout(7)).expect("valid layout");
    let buffer = runtime.ffi_buffer_open(&request).expect("buffer");
    let borrow = runtime
        .ffi_buffer_borrow(buffer, layout(7))
        .expect("borrow");
    let base = borrow.address().expect("nonempty buffer address");
    runtime
        .ffi_unsafe_write(base, &[1, 2, 3, 4])
        .expect("borrowed write");
    runtime
        .ffi_unsafe_copy(
            base,
            ForeignAddress::new(base.raw() + 1).expect("overlapping destination"),
            3,
        )
        .expect("overlapping copy");
    let mut bytes = [0; 4];
    runtime
        .ffi_unsafe_read(base, &mut bytes)
        .expect("borrowed read");
    assert_eq!(bytes, [1, 1, 2, 3]);
    assert!(
        runtime
            .ffi_unsafe_read(
                ForeignAddress::new(base.raw() + 4).expect("one-past address"),
                &mut [0],
            )
            .is_err()
    );
    runtime
        .ffi_buffer_end_borrow(buffer, borrow.id())
        .expect("end borrow");
    assert!(runtime.ffi_unsafe_read(base, &mut bytes).is_err());
}

#[test]
fn interpreter_executes_typed_buffer_storage_and_lexical_borrow_operations() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let size = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(221),
            arguments: Vec::new(),
        })
        .expect("Ffi.C.Size");
    let buffer = types
        .intern(SemanticType::Builtin {
            definition: FFI_BUFFER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("buffer");
    let optional_pointer = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(201),
            arguments: vec![integer],
        })
        .expect("optional pointer");
    let allocation_error = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(209),
            arguments: Vec::new(),
        })
        .expect("allocation error");
    let open_result = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(100),
            arguments: vec![buffer, allocation_error],
        })
        .expect("open result");
    let target = TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target");
    let catalog = MirFfiLayoutCatalog::new(
        &target,
        vec![MirFfiLayout::new(
            layout(7),
            integer,
            8,
            8,
            MirFfiValueClass::Integer,
        )],
        &types,
        artifact_sha256_hex,
    )
    .expect("catalog");
    let layout_id = catalog.entries()[0].id().raw();
    let text = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{size}, t{size}, t{integer}) -> (t{integer}) effects[Allocates,MayTrap,GcSafePoint,Roots]\n  b0(v0:t{size}, v1:t{size}, v2:t{integer}):\n    do v3 gcSafePoint sp0 roots ()\n    v4:t{open_result} = ffiBufferOpen v0 element t{integer} layout#{layout_id} size 8 align 8 result bt100 success resultCase#0 failure resultCase#1\n    v5:t{boolean} = resultIsOk bt100 v4\n    condBranch v5 b1 b2\n  b1():\n    v6:t{buffer} = resultGetOk bt100 v4\n    v7:t{size} = ffiBufferLength v6 layout#{layout_id}\n    do v8 ffiBufferWrite v6 v1 v2 layout#{layout_id}\n    v9:t{integer} = ffiBufferRead v6 v1 layout#{layout_id}\n    v10:t{optional_pointer} = ffiBufferBorrow v6 v7 layout#{layout_id} region#1\n    do v11 ffiBufferEndBorrow v6 region#1\n    do v12 ffiBufferClose v6\n    return (v9)\n  b2():\n    v13:t{allocation_error} = resultGetError bt100 v4\n    return (v2)\n",
        size = size.raw(),
        integer = integer.raw(),
        open_result = open_result.raw(),
        boolean = boolean.raw(),
        buffer = buffer.raw(),
        optional_pointer = optional_pointer.raw(),
        allocation_error = allocation_error.raw(),
    );
    let mir = parse_mir_dump(&text)
        .expect("buffer MIR")
        .with_ffi_layouts(catalog);
    let interpreter = MirInterpreter::new(&mir, &types).expect("verified buffer MIR");
    let output = interpreter
        .call(
            SymbolId::from_raw(0),
            &[
                MirValue::Integer(IntegerValue::parse_decimal("2", IntegerKind::UInt64).unwrap()),
                MirValue::Integer(IntegerValue::parse_decimal("2", IntegerKind::UInt64).unwrap()),
                MirValue::Integer(IntegerValue::parse_decimal("41", IntegerKind::Int64).unwrap()),
            ],
        )
        .expect("buffer execution");
    assert_eq!(
        output,
        vec![MirValue::Integer(
            IntegerValue::parse_decimal("41", IntegerKind::Int64).unwrap()
        )]
    );
    let allocation_failure = interpreter
        .call(
            SymbolId::from_raw(0),
            &[
                MirValue::Integer(
                    IntegerValue::parse_decimal(&(u64::MAX / 8).to_string(), IntegerKind::UInt64)
                        .unwrap(),
                ),
                MirValue::Integer(IntegerValue::parse_decimal("1", IntegerKind::UInt64).unwrap()),
                MirValue::Integer(IntegerValue::parse_decimal("41", IntegerKind::Int64).unwrap()),
            ],
        )
        .expect("allocation failure remains a typed Result branch");
    assert_eq!(allocation_failure, output);
}

#[test]
fn interpreter_executes_checked_unsafe_memory_operations() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let size = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(221),
            arguments: Vec::new(),
        })
        .expect("Ffi.C.Size");
    let difference = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(222),
            arguments: Vec::new(),
        })
        .expect("Ffi.C.PointerDifference");
    let pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("pointer");
    let read_only_pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_READ_ONLY_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("read-only pointer");
    let optional_pointer = types
        .intern(SemanticType::Builtin {
            definition: FFI_OPTIONAL_POINTER_TYPE_ID,
            arguments: vec![integer],
        })
        .expect("optional pointer");
    let target = TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target");
    let catalog = MirFfiLayoutCatalog::new(
        &target,
        vec![MirFfiLayout::new(
            layout(7),
            integer,
            8,
            8,
            MirFfiValueClass::Integer,
        )],
        &types,
        artifact_sha256_hex,
    )
    .expect("catalog");
    let layout_id = catalog.entries()[0].id().raw();
    let text = format!(
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{pointer}, t{read_only_pointer}, t{pointer}, t{read_only_pointer}, t{difference}, t{size}, t{integer}) -> (t{integer}, t{pointer}, t{size}, t{optional_pointer}) effects[UnsafeMemory,MayTrap]\n  b0(v0:t{pointer}, v1:t{read_only_pointer}, v2:t{pointer}, v3:t{read_only_pointer}, v4:t{difference}, v5:t{size}, v6:t{integer}):\n    do v7 ffiUnsafeStore v0 v6 layout#{layout_id}\n    do v8 ffiUnsafeCopy v1 v2 v5 layout#{layout_id}\n    v9:t{integer} = ffiUnsafeLoad v3 layout#{layout_id}\n    v10:t{pointer} = ffiUnsafeAdvance v0 v4 layout#{layout_id} readOnly false\n    v11:t{size} = ffiUnsafeAddress v1 layout#{layout_id}\n    v12:t{optional_pointer} = ffiUnsafePointerFromAddress v11 layout#{layout_id}\n    return (v9, v10, v11, v12)\n",
        pointer = pointer.raw(),
        read_only_pointer = read_only_pointer.raw(),
        difference = difference.raw(),
        size = size.raw(),
        integer = integer.raw(),
        optional_pointer = optional_pointer.raw(),
    );
    let mir = parse_mir_dump(&text)
        .expect("unsafe-memory MIR")
        .with_ffi_layouts(catalog);
    let mut runtime = ReferenceRuntimeAdapter::default();
    let buffer = runtime
        .ffi_buffer_open(&FfiBufferOpenRequest::new(3, 8, 8, layout(layout_id)).unwrap())
        .expect("buffer");
    let borrow = runtime
        .ffi_buffer_borrow(buffer, layout(layout_id))
        .expect("borrow");
    let source = borrow.address().expect("source address");
    let destination = ForeignAddress::new(source.raw() + 8).expect("destination address");
    let interpreter =
        MirInterpreter::with_runtime(&mir, &types, runtime).expect("verified unsafe-memory MIR");
    let output = interpreter
        .call(
            SymbolId::from_raw(0),
            &[
                MirValue::FfiPointer(source),
                MirValue::FfiPointer(source),
                MirValue::FfiPointer(destination),
                MirValue::FfiPointer(destination),
                MirValue::Integer(IntegerValue::parse_decimal("1", IntegerKind::Int64).unwrap()),
                MirValue::Integer(IntegerValue::parse_decimal("1", IntegerKind::UInt64).unwrap()),
                MirValue::Integer(IntegerValue::parse_decimal("41", IntegerKind::Int64).unwrap()),
            ],
        )
        .expect("unsafe-memory execution");
    assert_eq!(
        output,
        vec![
            MirValue::Integer(IntegerValue::parse_decimal("41", IntegerKind::Int64).unwrap()),
            MirValue::FfiPointer(destination),
            MirValue::Integer(
                IntegerValue::parse_decimal(&source.raw().to_string(), IntegerKind::UInt64)
                    .unwrap()
            ),
            MirValue::FfiPointer(source),
        ]
    );
}
