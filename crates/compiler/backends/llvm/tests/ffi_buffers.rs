use pop_backend_llvm::{LlvmLoweringError, LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_driver::artifact_sha256_hex;
use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_foundation::{BuiltinTypeId, FieldId};
use pop_mir::{
    MirFfiLayout, MirFfiLayoutCatalog, MirFfiLayoutField, MirFfiValueClass, parse_mir_dump,
};
use pop_runtime_interface::FfiAbiLayoutId;
use pop_source::SourceFile;
use pop_target::TargetSpec;
use pop_types::{FFI_BUFFER_TYPE_ID, SemanticType, TypeArena};

fn layout(raw: u64) -> FfiAbiLayoutId {
    FfiAbiLayoutId::new(raw).expect("nonzero layout")
}

fn target() -> TargetSpec {
    TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("native target")
}

#[test]
fn scoped_buffer_bodies_pass_captures_directly_without_a_closure_environment() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/scopedCapture.pop",
            "namespace Memory\n\
             public function enough(buffer: Ffi.Buffer<Int>, allowed: Boolean): Boolean\n\
                 return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Boolean\n\
                     return allowed\n\
                 end)\n\
             end\n",
        )
        .expect("source"),
    );
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![module],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let mir = pop_mir::lower_hir_bubble_with_fingerprint(
        result.hir().expect("scoped capture HIR"),
        result.types(),
        pop_driver::artifact_sha256_hex,
    )
    .expect("scoped capture MIR");
    assert!(!mir.dump().contains("closureEnvironment.allocate"));
    let llvm = lower_mir_to_llvm_ir(
        &mir,
        result.types(),
        &target(),
        LlvmLoweringOptions::default(),
    )
    .expect("scoped capture LLVM");
    let text = llvm.to_string();
    assert!(text.contains("@pop_b10_nested_0_0"), "{text}");
    assert!(text.contains("%capture0"), "{text}");
    llvm.verify().expect("valid scoped capture LLVM");
}

fn scalar_buffer_mir() -> (pop_mir::MirBubble, TypeArena) {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let boolean = types.source_type("Boolean").expect("Boolean");
    let size = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(221),
            arguments: Vec::new(),
        })
        .expect("size");
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
    let result = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(100),
            arguments: vec![buffer, allocation_error],
        })
        .expect("result");
    let catalog = MirFfiLayoutCatalog::new(
        &target(),
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
        "mir bubble b0 namespace n0\ndependencies\nfunction s0 f0(t{size}, t{size}, t{integer}) -> (t{integer}) effects[Allocates,MayTrap,GcSafePoint,Roots]\n  b0(v0:t{size}, v1:t{size}, v2:t{integer}):\n    do v3 gcSafePoint sp0 roots ()\n    v4:t{result} = ffiBufferOpen v0 element t{integer} layout#{layout_id} size 8 align 8 result bt100 success resultCase#0 failure resultCase#1\n    v5:t{boolean} = resultIsOk bt100 v4\n    condBranch v5 b1 b2\n  b1():\n    v6:t{buffer} = resultGetOk bt100 v4\n    v7:t{size} = ffiBufferLength v6 layout#{layout_id}\n    do v8 ffiBufferWrite v6 v1 v2 layout#{layout_id}\n    v9:t{integer} = ffiBufferRead v6 v1 layout#{layout_id}\n    v10:t{optional_pointer} = ffiBufferBorrow v6 v7 layout#{layout_id} region#1\n    v11:t{integer} = callScopedBorrow s0 nf0 region#1 captures[] (v10, v7) effects[] unwind propagate\n    do v12 ffiBufferEndBorrow v6 region#1\n    do v13 ffiBufferClose v6\n    return (v9)\n  b2():\n    v14:t{allocation_error} = resultGetError bt100 v4\n    return (v2)\nnested s0 nf0 captures - params(t{optional_pointer};t{size}) results(t{integer}) effects[]\n  b0(v0:t{optional_pointer}, v1:t{size}):\n    v2:t{integer} = const.integer Int64 0\n    return (v2)\n",
        size = size.raw(),
        integer = integer.raw(),
        result = result.raw(),
        boolean = boolean.raw(),
        buffer = buffer.raw(),
        optional_pointer = optional_pointer.raw(),
        allocation_error = allocation_error.raw(),
    );
    (
        parse_mir_dump(&text)
            .expect("buffer MIR")
            .with_ffi_layouts(catalog),
        types,
    )
}

fn record_buffer_mir() -> (pop_mir::MirBubble, TypeArena) {
    let mut types = TypeArena::new();
    let integer = types.source_type("Int").expect("Int");
    let byte = types.source_type("UInt8").expect("UInt8");
    let size = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(221),
            arguments: Vec::new(),
        })
        .expect("size");
    let record = types
        .intern(SemanticType::Record(vec![
            ("count".to_owned(), integer),
            ("tag".to_owned(), byte),
        ]))
        .expect("record");
    let buffer = types
        .intern(SemanticType::Builtin {
            definition: FFI_BUFFER_TYPE_ID,
            arguments: vec![record],
        })
        .expect("buffer");
    let allocation_error = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(209),
            arguments: Vec::new(),
        })
        .expect("allocation error");
    let result = types
        .intern(SemanticType::Builtin {
            definition: BuiltinTypeId::from_raw(100),
            arguments: vec![buffer, allocation_error],
        })
        .expect("result");
    let catalog = MirFfiLayoutCatalog::new(
        &target(),
        vec![
            MirFfiLayout::new(layout(1), integer, 8, 8, MirFfiValueClass::Integer),
            MirFfiLayout::new(layout(2), byte, 1, 1, MirFfiValueClass::Integer),
            MirFfiLayout::new(
                layout(3),
                record,
                16,
                8,
                MirFfiValueClass::Record(vec![
                    MirFfiLayoutField::new(FieldId::from_raw(1), 0, layout(1), 0),
                    MirFfiLayoutField::new(FieldId::from_raw(2), 1, layout(2), 8),
                ]),
            ),
        ],
        &types,
        artifact_sha256_hex,
    )
    .expect("catalog");
    let layout_id = catalog
        .entries()
        .iter()
        .find(|entry| entry.element() == record)
        .expect("record layout")
        .id()
        .raw();
    let text = format!(
        "mir bubble b0 namespace n0\ndependencies\ntype.record s1 t{record} fields field#1:t{integer},field#2:t{byte}\nfunction s0 f0(t{size}, t{size}, t{record}) -> (t{record}) effects[Allocates,MayTrap,GcSafePoint,Roots]\n  b0(v0:t{size}, v1:t{size}, v2:t{record}):\n    do v3 gcSafePoint sp0 roots ()\n    v4:t{result} = ffiBufferOpen v0 element t{record} layout#{layout_id} size 16 align 8 result bt100 success resultCase#0 failure resultCase#1\n    v5:t{buffer} = resultGetOk bt100 v4\n    do v6 ffiBufferWrite v5 v1 v2 layout#{layout_id}\n    v7:t{record} = ffiBufferRead v5 v1 layout#{layout_id}\n    do v8 ffiBufferClose v5\n    return (v7)\n",
        size = size.raw(),
        record = record.raw(),
        integer = integer.raw(),
        byte = byte.raw(),
        result = result.raw(),
        buffer = buffer.raw(),
    );
    (
        parse_mir_dump(&text)
            .expect("record buffer MIR")
            .with_ffi_layouts(catalog),
        types,
    )
}

#[test]
fn lowers_checked_buffer_statuses_marshalling_and_private_borrow_generation() {
    let (mir, types) = scalar_buffer_mir();
    let module = lower_mir_to_llvm_ir(&mir, &types, &target(), LlvmLoweringOptions::default())
        .expect("buffer LLVM lowering");
    let text = module.to_string();
    assert!(text.contains("call i8 @pop_rt_ffi_buffer_open"));
    assert!(text.contains("switch i8") && text.contains("i8 0") && text.contains("i8 2"));
    assert!(text.contains("call i8 @pop_rt_ffi_buffer_length"));
    assert!(text.contains("call i8 @pop_rt_ffi_buffer_write"));
    assert!(text.contains("call i8 @pop_rt_ffi_buffer_read"));
    assert!(text.contains("call i8 @pop_rt_ffi_buffer_borrow"));
    assert!(text.contains("ffi_buffer_region_1_generation"));
    assert!(text.contains("icmp eq i64") && text.contains("%v7"));
    assert!(text.contains("call i8 @pop_rt_ffi_buffer_end_borrow"));
    assert!(text.contains("store i64 0, ptr %ffi_buffer_region_1_generation"));
    assert!(text.contains("call i8 @pop_rt_ffi_buffer_close"));
    assert!(!text.contains("memcpy"));
    module.verify().expect("valid LLVM buffer module");
}

#[test]
fn rejects_a_backend_target_that_differs_from_the_verified_catalog() {
    let (mir, types) = scalar_buffer_mir();
    let bpf = TargetSpec::for_triple("bpfel-unknown-none").expect("BPF target");
    assert!(matches!(
        lower_mir_to_llvm_ir(&mir, &types, &bpf, LlvmLoweringOptions::default()),
        Err(LlvmLoweringError::FfiLayoutTargetMismatch { .. })
    ));
}

#[test]
fn marshals_records_field_by_field_and_zeroes_abi_padding() {
    let (mir, types) = record_buffer_mir();
    let module = lower_mir_to_llvm_ir(&mir, &types, &target(), LlvmLoweringOptions::default())
        .expect("record buffer LLVM lowering");
    let text = module.to_string();
    assert!(text.contains("store [16 x i8] zeroinitializer"));
    assert!(text.contains("getelementptr i8") && text.contains("i64 8"));
    assert!(text.contains("call i64 @pop_rt_field_get"));
    assert!(text.contains("call i8 @pop_rt_field_set"));
    assert!(!text.contains("memcpy"));
    module.verify().expect("valid record buffer module");
}
