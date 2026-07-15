use pop_driver::artifact_sha256_hex;
use pop_foundation::{BorrowRegionId, BuiltinTypeId, ResultCaseId, TypeId, ValueId};
use pop_mir::{
    MirFfiLayout, MirFfiLayoutCatalog, MirFfiValueClass, MirInstructionKind,
    is_managed_reference_type_id, parse_mir_dump, verify_mir_bubble,
};
use pop_runtime_interface::FfiAbiLayoutId;
use pop_target::TargetSpec;
use pop_types::{
    FFI_BUFFER_TYPE_ID, FFI_OPTIONAL_POINTER_TYPE_ID, FFI_POINTER_TYPE_ID,
    FFI_READ_ONLY_POINTER_TYPE_ID, SemanticType, TypeArena,
};

fn value(raw: u32) -> ValueId {
    ValueId::from_raw(raw)
}

fn native_target() -> TargetSpec {
    TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target")
}

fn layout(raw: u64) -> FfiAbiLayoutId {
    FfiAbiLayoutId::new(raw).expect("nonzero layout")
}

#[test]
fn buffer_operations_round_trip_with_exact_layout_and_region_text() {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
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
        .expect("Ffi.Buffer<Int>");
    let optional_pointer = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(201),
            arguments: vec![integer],
        })
        .expect("Ffi.OptionalPointer<Int>");
    let allocation_error = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(209),
            arguments: Vec::new(),
        })
        .expect("Ffi.AllocationError");
    let null_pointer_error = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(208),
            arguments: Vec::new(),
        })
        .expect("Ffi.NullPointerError");
    assert!(!is_managed_reference_type_id(
        allocation_error,
        Some(&types)
    ));
    assert!(!is_managed_reference_type_id(
        null_pointer_error,
        Some(&types)
    ));
    let open_result = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(100),
            arguments: vec![buffer, allocation_error],
        })
        .expect("buffer allocation result");
    let catalog = MirFfiLayoutCatalog::new(
        &native_target(),
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
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{size}, t{buffer}, t{integer}) -> () effects[Allocates,MayTrap,GcSafePoint,Roots]\n  b0(v0:t{size}, v1:t{buffer}, v2:t{integer}):\n    do v3 gcSafePoint sp0 roots (v1)\n    v4:t{open_result} = ffiBufferOpen v0 element t{integer} layout#{layout_id} size 8 align 8 result bt100 success resultCase#0 failure resultCase#1\n    v5:t{size} = ffiBufferLength v1 layout#{layout_id}\n    v6:t{integer} = ffiBufferRead v1 v0 layout#{layout_id}\n    do v7 ffiBufferWrite v1 v0 v2 layout#{layout_id}\n    v8:t{optional_pointer} = ffiBufferBorrow v1 v5 layout#{layout_id} region#9\n    do v9 callScopedBorrow s0 nf0 region#9 captures[] (v8,v5) effects[] unwind propagate\n    do v10 ffiBufferEndBorrow v1 region#9\n    do v11 ffiBufferClose v1\n    return ()\nnested s0 nf0 captures - params(t{optional_pointer};t{size}) results() effects[]\n  b0(v0:t{optional_pointer}, v1:t{size}):\n    return ()\n",
        size = size.raw(),
        buffer = buffer.raw(),
        integer = integer.raw(),
        optional_pointer = optional_pointer.raw(),
        open_result = open_result.raw(),
    );

    let bubble = parse_mir_dump(&text)
        .expect("buffer MIR")
        .with_ffi_layouts(catalog.clone());
    assert_eq!(verify_mir_bubble(&bubble, &types), Ok(()));
    let dump = bubble.dump();
    assert!(dump.contains("ffiBufferOpen v0 element"));
    assert!(dump.contains(&format!(
        "ffiBufferBorrow v1 v5 layout#{layout_id} region#9"
    )));
    assert!(dump.contains("ffiBufferEndBorrow v1 region#9"));
    assert_eq!(
        parse_mir_dump(&dump)
            .expect("round trip")
            .with_ffi_layouts(catalog.clone()),
        bubble
    );

    assert_invalid_buffer_variants(
        &text,
        &catalog,
        &types,
        integer,
        buffer,
        optional_pointer,
        layout_id,
    );
}

fn assert_invalid_buffer_variants(
    text: &str,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
    integer: TypeId,
    buffer: TypeId,
    optional_pointer: TypeId,
    layout_id: u64,
) {
    let layout_text = format!("layout#{layout_id}");
    let invalid_texts = [
        text.replace(&layout_text, "layout#1"),
        text.replace("ffiBufferRead v1 v0", "ffiBufferRead v1 v2"),
        text.replace(
            &format!("v6:t{} = ffiBufferRead", integer.raw()),
            &format!("v6:t{} = ffiBufferRead", buffer.raw()),
        ),
        text.replace(
            "do v7 ffiBufferWrite",
            &format!("v7:t{} = ffiBufferWrite", integer.raw()),
        ),
        text.replace("ffiBufferBorrow v1 v5", "ffiBufferBorrow v1 v0"),
        text.replace("    do v10 ffiBufferEndBorrow v1 region#9\n", ""),
        text.replace("ffiBufferEndBorrow v1 region#9", "ffiBufferEndBorrow v1 region#10"),
        text.replace(
            "    do v10 ffiBufferEndBorrow",
            "    do v12 ffiBufferClose v1\n    do v10 ffiBufferEndBorrow",
        ),
        text.replace(
            "    do v10 ffiBufferEndBorrow",
            &format!(
                "    v12:t{} = ffiBufferBorrow v1 v5 layout#{layout_id} region#10\n    do v10 ffiBufferEndBorrow",
                optional_pointer.raw()
            ),
        ),
        text.replace("-> () effects", &format!("-> (t{}) effects", optional_pointer.raw()))
            .replace("    return ()", "    return (v8)"),
    ];
    for invalid_text in invalid_texts {
        let invalid = parse_mir_dump(&invalid_text)
            .expect("structurally valid corrupt buffer MIR")
            .with_ffi_layouts(catalog.clone());
        assert!(
            verify_mir_bubble(&invalid, types).is_err(),
            "invalid FFI buffer MIR was accepted:\n{invalid_text}"
        );
    }
    assert!(
        verify_mir_bubble(&parse_mir_dump(text).expect("missing catalog MIR"), types).is_err(),
        "an FFI operation without a catalog entry was accepted"
    );
}

#[test]
fn canonical_buffer_operations_keep_layout_and_borrow_identities_explicit() {
    let region = BorrowRegionId::from_raw(9);
    let operations = [
        MirInstructionKind::FfiBufferOpen {
            length: value(0),
            element: TypeId::from_raw(12),
            layout: layout(7),
            element_size: 8,
            alignment: 8,
            result: BuiltinTypeId::from_raw(100),
            success: ResultCaseId::from_raw(0),
            failure: ResultCaseId::from_raw(1),
        },
        MirInstructionKind::FfiBufferLength {
            buffer: value(1),
            layout: layout(7),
        },
        MirInstructionKind::FfiBufferRead {
            buffer: value(1),
            index: value(2),
            layout: layout(7),
        },
        MirInstructionKind::FfiBufferWrite {
            buffer: value(1),
            index: value(2),
            value: value(3),
            layout: layout(7),
        },
        MirInstructionKind::FfiBufferBorrow {
            buffer: value(1),
            expected_length: value(4),
            layout: layout(7),
            region,
        },
        MirInstructionKind::FfiBufferEndBorrow {
            buffer: value(1),
            region,
        },
        MirInstructionKind::FfiBufferClose { buffer: value(1) },
    ];

    assert_eq!(operations.len(), 7);
    assert!(matches!(
        operations[4],
        MirInstructionKind::FfiBufferBorrow {
            region: found,
            ..
        } if found == region
    ));
    assert!(matches!(
        operations[5],
        MirInstructionKind::FfiBufferEndBorrow {
            region: found,
            ..
        } if found == region
    ));
}

#[test]
fn unsafe_memory_operations_round_trip_and_reject_type_drift() {
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
    let catalog = MirFfiLayoutCatalog::new(
        &native_target(),
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
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{pointer}, t{read_only_pointer}, t{difference}, t{size}, t{integer}) -> (t{integer}) effects[UnsafeMemory,MayTrap]\n  b0(v0:t{pointer}, v1:t{read_only_pointer}, v2:t{difference}, v3:t{size}, v4:t{integer}):\n    do v5 ffiUnsafeStore v0 v4 layout#{layout_id}\n    v6:t{integer} = ffiUnsafeLoad v1 layout#{layout_id}\n    v7:t{pointer} = ffiUnsafeAdvance v0 v2 layout#{layout_id} readOnly false\n    do v8 ffiUnsafeCopy v1 v0 v3 layout#{layout_id}\n    v9:t{size} = ffiUnsafeAddress v1 layout#{layout_id}\n    v10:t{optional_pointer} = ffiUnsafePointerFromAddress v9 layout#{layout_id}\n    return (v6)\n",
        pointer = pointer.raw(),
        read_only_pointer = read_only_pointer.raw(),
        difference = difference.raw(),
        size = size.raw(),
        integer = integer.raw(),
        optional_pointer = optional_pointer.raw(),
    );
    let bubble = parse_mir_dump(&text)
        .expect("unsafe-memory MIR")
        .with_ffi_layouts(catalog.clone());
    assert_eq!(verify_mir_bubble(&bubble, &types), Ok(()));
    let dump = bubble.dump();
    assert_eq!(
        parse_mir_dump(&dump)
            .expect("unsafe-memory round trip")
            .with_ffi_layouts(catalog.clone()),
        bubble
    );

    for invalid_text in [
        text.replace("ffiUnsafeLoad v1", "ffiUnsafeLoad v0"),
        text.replace("ffiUnsafeStore v0", "ffiUnsafeStore v1"),
        text.replace("ffiUnsafeCopy v1 v0 v3", "ffiUnsafeCopy v1 v0 v2"),
        text.replace("readOnly false", "readOnly true"),
        text.replace(
            "do v5 ffiUnsafeStore",
            &format!("v5:t{} = ffiUnsafeStore", integer.raw()),
        ),
    ] {
        let invalid = parse_mir_dump(&invalid_text)
            .expect("structurally valid corrupt unsafe-memory MIR")
            .with_ffi_layouts(catalog.clone());
        assert!(
            verify_mir_bubble(&invalid, &types).is_err(),
            "invalid unsafe-memory MIR was accepted:\n{invalid_text}"
        );
    }
    assert!(
        verify_mir_bubble(&parse_mir_dump(&text).expect("missing catalog MIR"), &types).is_err()
    );
}
