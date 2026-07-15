use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolId};
use pop_source::SourceFile;
use pop_types::{Effect, ForeignAbi};

fn ffi_module() -> FrontEndModule {
    FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/native.pop",
            "namespace Native\n\
             internal function close(pointer: Ffi.Pointer<Ffi.C.Int>)\n\
             end\n",
        )
        .expect("source"),
    )
}

fn assert_balanced_foreign_root_contract(mir: &pop_mir::MirBubble, wrapper_symbol: SymbolId) {
    let wrapper = mir
        .functions()
        .iter()
        .find(|function| function.symbol() == wrapper_symbol)
        .expect("foreign wrapper MIR");
    let instructions = wrapper.blocks()[0].instructions();
    let call_index = instructions
        .iter()
        .position(|instruction| {
            matches!(
                instruction.kind(),
                pop_mir::MirInstructionKind::CallForeign { .. }
            )
        })
        .expect("canonical foreign call");
    let pop_mir::MirInstructionKind::GcSafePoint {
        safe_point: published_safe_point,
        roots: published_roots,
        ..
    } = instructions[call_index - 1].kind()
    else {
        panic!("foreign call must immediately follow its safe point");
    };
    let pop_mir::MirInstructionKind::CallForeign {
        safe_point, roots, ..
    } = instructions[call_index].kind()
    else {
        unreachable!();
    };
    assert_eq!(safe_point, published_safe_point);
    assert_eq!(roots, published_roots);
    assert_eq!(roots.len(), 1, "only the live String is a managed root");
}

#[test]
fn front_end_enables_ffi_types_only_for_the_verified_ffi_dependency() {
    let bubble = BubbleId::from_raw(10);
    let ffi = BubbleId::from_raw(20);
    let without_dependency = analyze_bubble(FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(10),
        Vec::new(),
        vec![ffi_module()],
    ));

    assert!(without_dependency.hir().is_none());
    assert!(without_dependency.diagnostic_snapshot().contains("POP1002"));

    let with_dependency = analyze_bubble(
        FrontEndBubbleInput::new(
            bubble,
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![ffi_module()],
        )
        .with_ffi_dependency(ffi),
    );

    assert!(
        with_dependency.diagnostics().is_empty(),
        "{}",
        with_dependency.diagnostic_snapshot()
    );
    assert!(with_dependency.hir().is_some());
}

#[test]
fn ffi_handle_calls_lower_to_typed_backend_neutral_operations() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/handles.pop",
            "namespace Handles\n\
             public function roundTrip(value: Array<Int>): Array<Int>\n\
             local handle = Ffi.Handle.open<<Array<Int>>>(value)\n\
                 local resolved = Ffi.Handle.get(handle)\n\
                 Ffi.Handle.close(handle)\n\
                 return resolved\n\
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
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("handle HIR"), result.types())
        .expect("handle MIR");
    let dump = mir.dump();
    assert!(dump.contains("ffiHandleOpen"), "{dump}");
    assert!(dump.contains("ffiHandleGet"), "{dump}");
    assert!(dump.contains("ffiHandleClose"), "{dump}");
}

#[test]
fn ffi_handle_calls_require_the_dependency_and_managed_payloads() {
    let source = |body: &str| {
        FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(
                FileId::from_raw(0),
                "src/invalidHandle.pop",
                format!("namespace Handles\n{body}"),
            )
            .expect("source"),
        )
    };
    let without_dependency = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(10),
        NamespaceId::from_raw(10),
        Vec::new(),
        vec![source(
            "public function invalid(value: Array<Int>)\n    Ffi.Handle.open(value)\nend\n",
        )],
    ));
    assert!(without_dependency.hir().is_none());

    let ffi = BubbleId::from_raw(20);
    let scalar = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![source(
                "public function invalid()\n    Ffi.Handle.open(1)\nend\n",
            )],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(scalar.hir().is_none());
    assert!(!scalar.diagnostics().is_empty());
}

#[test]
fn ffi_buffer_calls_lower_from_typed_source_with_one_target_layout_catalog() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/buffers.pop",
            "namespace Buffers\n\
             public function allocate(length: Ffi.C.Size): Result<Ffi.Buffer<Int>, Ffi.AllocationError>\n\
                 return Ffi.Buffer.open<<Int>>(length)\n\
             end\n\
             public function length(buffer: Ffi.Buffer<Int>): Ffi.C.Size\n\
                 return Ffi.Buffer.length(buffer)\n\
             end\n\
             public function read(buffer: Ffi.Buffer<Int>, index: Ffi.C.Size): Int\n\
                 return Ffi.Buffer.read(buffer, index)\n\
             end\n\
             public function write(buffer: Ffi.Buffer<Int>, index: Ffi.C.Size, value: Int)\n\
                 Ffi.Buffer.write(buffer, index, value)\n\
             end\n\
             public function close(buffer: Ffi.Buffer<Int>)\n\
                 Ffi.Buffer.close(buffer)\n\
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

    assert_eq!(
        pop_mir::lower_hir_bubble(result.hir().expect("buffer HIR"), result.types())
            .expect_err("source FFI layouts require artifact-owned fingerprints"),
        vec![pop_mir::MirVerificationError::MissingFfiLayoutFingerprint]
    );

    let mir = pop_mir::lower_hir_bubble_with_fingerprint(
        result.hir().expect("buffer HIR"),
        result.types(),
        pop_driver::artifact_sha256_hex,
    )
    .expect("buffer MIR");
    let [layout] = mir.ffi_layouts().entries() else {
        panic!("one canonical Int buffer layout");
    };
    assert_eq!(
        layout.element(),
        result.types().source_type("Int").expect("Int")
    );
    assert_eq!(layout.size(), 8);
    assert_eq!(layout.alignment(), 8);
    assert_eq!(layout.fingerprint().len(), 64);
    let dump = mir.dump();
    assert!(dump.contains("ffiBufferOpen"), "{dump}");
    assert!(dump.contains("ffiBufferLength"), "{dump}");
    assert!(dump.contains("ffiBufferRead"), "{dump}");
    assert!(dump.contains("ffiBufferWrite"), "{dump}");
    assert!(dump.contains("ffiBufferClose"), "{dump}");
    assert!(dump.contains(&format!("layout#{}", layout.id().raw())));
}

#[test]
fn ffi_buffer_calls_require_the_dependency_abi_storage_and_exact_operands() {
    let module = |body: &str| {
        FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(
                FileId::from_raw(0),
                "src/invalidBuffer.pop",
                format!("namespace Buffers\n{body}"),
            )
            .expect("source"),
        )
    };
    let without_dependency = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(10),
        NamespaceId::from_raw(10),
        Vec::new(),
        vec![module(
            "public function invalid(length: Int)\n    Ffi.Buffer.open<<Int>>(length)\nend\n",
        )],
    ));
    assert!(without_dependency.hir().is_none());

    let ffi = BubbleId::from_raw(20);
    for body in [
        "public function invalid(length: Ffi.C.Size)\n    Ffi.Buffer.open<<Array<Int>>>(length)\nend\n",
        "public function invalid(buffer: Ffi.Buffer<Int>)\n    Ffi.Buffer.read(buffer, true)\nend\n",
        "public function invalid(buffer: Ffi.Buffer<Int>, index: Ffi.C.Size)\n    Ffi.Buffer.write(buffer, index, true)\nend\n",
        "public function invalid(buffer: Ffi.Buffer<Int>)\n    Ffi.Buffer.close(buffer, buffer)\nend\n",
    ] {
        let result = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(10),
                NamespaceId::from_raw(10),
                vec![ffi],
                vec![module(body)],
            )
            .with_ffi_dependency(ffi),
        );
        assert!(result.hir().is_none(), "{body}");
        assert!(!result.diagnostics().is_empty(), "{body}");
    }
}

#[test]
fn ffi_layout_records_flow_from_trusted_source_attributes_into_the_target_catalog() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/layout.pop",
            "namespace Layout\n\
             @Ffi.C.Layout\n\
             public record Pair\n\
                 left: Ffi.C.Int\n\
                 right: Ffi.C.Int\n\
             end\n\
             public function allocate(length: Ffi.C.Size): Result<Ffi.Buffer<Pair>, Ffi.AllocationError>\n\
                 return Ffi.Buffer.open<<Pair>>(length)\n\
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
        result.diagnostics().is_empty() && result.hir_build_errors().is_empty(),
        "{}\n{:?}",
        result.diagnostic_snapshot(),
        result.hir_build_errors()
    );
    let pair = result
        .hir()
        .expect("layout HIR")
        .declarations()
        .iter()
        .find_map(|declaration| match declaration.kind() {
            pop_hir::HirDeclarationKind::Record(record) if declaration.name() == "Pair" => {
                Some(record.type_id())
            }
            _ => None,
        })
        .expect("Pair");
    let mir = pop_mir::lower_hir_bubble_with_fingerprint(
        result.hir().expect("layout HIR"),
        result.types(),
        pop_driver::artifact_sha256_hex,
    )
    .expect("layout MIR");
    let layout = mir
        .ffi_layouts()
        .entries()
        .iter()
        .find(|layout| layout.element() == pair)
        .expect("record layout");
    assert_eq!(layout.size(), 8);
    assert_eq!(layout.alignment(), 4);
    let pop_mir::MirFfiValueClass::Record(fields) = layout.value_class() else {
        panic!("record value class");
    };
    assert_eq!(fields.len(), 2);
    assert_eq!(fields[0].offset(), 0);
    assert_eq!(fields[1].offset(), 4);
    assert!(layout.descriptor().contains("\"name\":\"left\""));
    assert!(layout.descriptor().contains("\"name\":\"right\""));
}

#[test]
fn ffi_layout_records_reject_missing_trust_and_managed_fields() {
    let ffi = BubbleId::from_raw(20);
    for source in [
        "namespace Layout\n\
         public record Pair\n\
             left: Ffi.C.Int\n\
         end\n\
         public function invalid(length: Ffi.C.Size)\n\
             Ffi.Buffer.open<<Pair>>(length)\n\
         end\n",
        "namespace Layout\n\
         @Ffi.C.Layout\n\
         public record Managed\n\
             value: String\n\
         end\n\
         public function invalid(length: Ffi.C.Size)\n\
             Ffi.Buffer.open<<Managed>>(length)\n\
         end\n",
    ] {
        let result = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(10),
                NamespaceId::from_raw(10),
                vec![ffi],
                vec![FrontEndModule::new(
                    ModuleId::from_raw(0),
                    SourceFile::new(FileId::from_raw(0), "src/invalidLayout.pop", source)
                        .expect("source"),
                )],
            )
            .with_ffi_dependency(ffi),
        );
        assert!(result.hir().is_none(), "{source}");
        assert!(!result.diagnostics().is_empty(), "{source}");
    }
}

#[test]
fn ffi_layout_records_reject_defaults_and_invalid_nested_abi_storage() {
    let ffi = BubbleId::from_raw(20);
    for (name, field) in [
        ("defaulted", "value: Int32 = 0"),
        ("managedPointer", "value: Ffi.Pointer<String>"),
        ("invalidCallback", "value: Ffi.Function<InvalidCallback>"),
        ("scalarHandle", "value: Ffi.Handle<Int32>"),
    ] {
        let source = format!(
            "namespace Layout\n\
             private type InvalidCallback = function(input: String): Int32\n\
             @Ffi.C.Layout\n\
             public record Invalid\n\
                 {field}\n\
             end\n\
             public function allocate(length: Ffi.C.Size)\n\
                 Ffi.Buffer.open<<Invalid>>(length)\n\
             end\n"
        );
        let result = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(10),
                NamespaceId::from_raw(10),
                vec![ffi],
                vec![FrontEndModule::new(
                    ModuleId::from_raw(0),
                    SourceFile::new(FileId::from_raw(0), format!("src/{name}.pop"), source)
                        .expect("source"),
                )],
            )
            .with_ffi_dependency(ffi),
        );
        assert!(result.hir().is_none(), "{name} must not reach HIR");
        assert!(
            result.diagnostic_snapshot().contains("POP5000"),
            "{name}: {}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn user_attributes_cannot_spoof_the_trusted_ffi_layout_identity() {
    let ffi = BubbleId::from_raw(20);
    let attribute = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/userLayout.pop",
            "namespace Ffi.C\n\
             @AttributeUsage(targets = { AttributeTarget.Record })\n\
             public attribute Layout()\n",
        )
        .expect("source"),
    );
    let consumer = FrontEndModule::new(
        ModuleId::from_raw(1),
        SourceFile::new(
            FileId::from_raw(1),
            "src/consumer.pop",
            "namespace Consumer\n\
             @Ffi.C.Layout\n\
             public record Pair\n\
                 value: Ffi.C.Int\n\
             end\n\
             public function invalid(length: Ffi.C.Size)\n\
                 Ffi.Buffer.open<<Pair>>(length)\n\
             end\n",
        )
        .expect("source"),
    );
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![attribute, consumer],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(result.hir().is_none());
    assert!(!result.diagnostics().is_empty());
}

#[test]
fn safe_ffi_pointer_construction_and_presence_lower_to_typed_mir() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/pointers.pop",
            "namespace Pointers\n\
             public function inspect(pointer: Ffi.Pointer<Int>): Boolean\n\
                 local optional = Ffi.OptionalPointer.fromPointer(pointer)\n\
                 local readOnly = Ffi.Pointer.readOnly(pointer)\n\
                 local optionalReadOnly = Ffi.OptionalReadOnlyPointer.fromPointer(readOnly)\n\
                 local absent = Ffi.OptionalReadOnlyPointer.none<<Int>>()\n\
                 return Ffi.OptionalPointer.isPresent(optional) and Ffi.OptionalReadOnlyPointer.isPresent(optionalReadOnly) and not Ffi.OptionalReadOnlyPointer.isPresent(absent)\n\
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
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("pointer HIR"), result.types())
        .expect("pointer MIR");
    let dump = mir.dump();
    assert!(dump.contains("ffiPointerToOptional"), "{dump}");
    assert!(dump.contains("ffiPointerReadOnly"), "{dump}");
    assert!(dump.contains("ffiPointerNone"), "{dump}");
    assert!(dump.contains("ffiPointerIsPresent"), "{dump}");
}

#[test]
fn safe_ffi_pointer_operations_require_dependency_direction_and_exact_arity() {
    let module = |body: &str| {
        FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(
                FileId::from_raw(0),
                "src/invalidPointer.pop",
                format!("namespace Pointers\n{body}"),
            )
            .expect("source"),
        )
    };
    let without_dependency = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(10),
        NamespaceId::from_raw(10),
        Vec::new(),
        vec![module(
            "public function invalid(): Ffi.OptionalPointer<Int>\n    return Ffi.OptionalPointer.none<<Int>>()\nend\n",
        )],
    ));
    assert!(without_dependency.hir().is_none());
    assert!(!without_dependency.diagnostics().is_empty());

    let ffi = BubbleId::from_raw(20);
    for body in [
        "public function invalid(pointer: Ffi.ReadOnlyPointer<Int>)\n    Ffi.OptionalPointer.fromPointer(pointer)\nend\n",
        "public function invalid(pointer: Ffi.Pointer<Int>)\n    Ffi.OptionalReadOnlyPointer.fromPointer(pointer)\nend\n",
        "public function invalid(pointer: Ffi.OptionalPointer<Int>)\n    Ffi.Pointer.readOnly(pointer)\nend\n",
        "public function invalid()\n    Ffi.OptionalPointer.none<<Int, Int>>()\nend\n",
        "public function invalid(pointer: Ffi.Pointer<Int>)\n    Ffi.OptionalPointer.fromPointer(pointer, pointer)\nend\n",
    ] {
        let result = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(10),
                NamespaceId::from_raw(10),
                vec![ffi],
                vec![module(body)],
            )
            .with_ffi_dependency(ffi),
        );
        assert!(result.hir().is_none(), "{body}");
        assert!(!result.diagnostics().is_empty(), "{body}");
    }
}

#[test]
fn checked_ffi_pointer_require_lowers_to_one_typed_result_operation() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/requirePointer.pop",
            "namespace Pointers\n\
             public function requireMutable(pointer: Ffi.OptionalPointer<Int>): Result<Ffi.Pointer<Int>, Ffi.NullPointerError>\n\
                 return Ffi.OptionalPointer.require(pointer)\n\
             end\n\
             public function requireReadOnly(pointer: Ffi.OptionalReadOnlyPointer<Int>): Result<Ffi.ReadOnlyPointer<Int>, Ffi.NullPointerError>\n\
                 return Ffi.OptionalReadOnlyPointer.require(pointer)\n\
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
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("pointer HIR"), result.types())
        .expect("pointer MIR");
    let dump = mir.dump();
    assert_eq!(dump.matches("ffiPointerRequire").count(), 2, "{dump}");
    assert!(dump.contains("success resultCase#0 failure resultCase#1"));
    let reparsed = pop_mir::parse_mir_dump(&dump).expect("pointer require MIR text round trip");
    assert_eq!(reparsed.dump(), dump);
}

#[test]
fn checked_ffi_pointer_require_rejects_wrong_direction_and_arity() {
    let ffi = BubbleId::from_raw(20);
    for body in [
        "public function invalid(pointer: Ffi.OptionalReadOnlyPointer<Int>)\n    Ffi.OptionalPointer.require(pointer)\nend\n",
        "public function invalid(pointer: Ffi.OptionalPointer<Int>)\n    Ffi.OptionalReadOnlyPointer.require(pointer)\nend\n",
        "public function invalid(pointer: Ffi.OptionalPointer<Int>)\n    Ffi.OptionalPointer.require(pointer, pointer)\nend\n",
    ] {
        let module = FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(
                FileId::from_raw(0),
                "src/invalidRequirePointer.pop",
                format!("namespace Pointers\n{body}"),
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
        assert!(result.hir().is_none(), "{body}");
        assert!(!result.diagnostics().is_empty(), "{body}");
    }
}

#[test]
fn ffi_unsafe_memory_calls_lower_to_closed_typed_operations() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/unsafeMemory.pop",
            "namespace Memory\n\
             public function load(pointer: Ffi.ReadOnlyPointer<Int>): Int\n\
                 return Ffi.Unsafe.load(pointer)\n\
             end\n\
             public function store(pointer: Ffi.Pointer<Int>, value: Int)\n\
                 Ffi.Unsafe.store(pointer, value)\n\
             end\n\
             public function advance(pointer: Ffi.Pointer<Int>, elements: Ffi.C.PointerDifference): Ffi.Pointer<Int>\n\
                 return Ffi.Unsafe.advance(pointer, elements)\n\
             end\n\
             public function advanceReadOnly(pointer: Ffi.ReadOnlyPointer<Int>, elements: Ffi.C.PointerDifference): Ffi.ReadOnlyPointer<Int>\n\
                 return Ffi.Unsafe.advanceReadOnly(pointer, elements)\n\
             end\n\
             public function copy(source: Ffi.ReadOnlyPointer<Int>, destination: Ffi.Pointer<Int>, count: Ffi.C.Size)\n\
                 Ffi.Unsafe.copy(source, destination, count)\n\
             end\n\
             public function address(pointer: Ffi.ReadOnlyPointer<Int>): Ffi.C.Size\n\
                 return Ffi.Unsafe.address(pointer)\n\
             end\n\
             public function fromAddress(address: Ffi.C.Size): Ffi.OptionalPointer<Int>\n\
                 return Ffi.Unsafe.pointerFromAddress<<Int>>(address)\n\
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
    assert_eq!(
        pop_mir::lower_hir_bubble(result.hir().expect("unsafe memory HIR"), result.types())
            .expect_err("unsafe ABI layouts require canonical fingerprints"),
        vec![pop_mir::MirVerificationError::MissingFfiLayoutFingerprint]
    );
    let mir = pop_mir::lower_hir_bubble_with_fingerprint(
        result.hir().expect("unsafe memory HIR"),
        result.types(),
        pop_driver::artifact_sha256_hex,
    )
    .expect("unsafe memory MIR");
    let dump = mir.dump();
    for operation in [
        "ffiUnsafeLoad",
        "ffiUnsafeStore",
        "ffiUnsafeAdvance",
        "ffiUnsafeCopy",
        "ffiUnsafeAddress",
        "ffiUnsafePointerFromAddress",
    ] {
        assert!(dump.contains(operation), "missing {operation}\n{dump}");
    }
    assert!(dump.contains("effects[MayTrap,UnsafeMemory]"), "{dump}");
    assert!(dump.contains("effects[UnsafeMemory]"), "{dump}");
}

#[test]
fn ffi_unsafe_memory_calls_reject_safe_namespace_and_type_drift() {
    let ffi = BubbleId::from_raw(20);
    for body in [
        "public function invalid(pointer: Ffi.Pointer<Int>)\n    Ffi.Unsafe.load(pointer)\nend\n",
        "public function invalid(pointer: Ffi.ReadOnlyPointer<Int>, value: Int)\n    Ffi.Unsafe.store(pointer, value)\nend\n",
        "public function invalid(pointer: Ffi.Pointer<Int>, elements: Int)\n    Ffi.Unsafe.advance(pointer, elements)\nend\n",
        "public function invalid(source: Ffi.ReadOnlyPointer<Int>, destination: Ffi.Pointer<Byte>, count: Ffi.C.Size)\n    Ffi.Unsafe.copy(source, destination, count)\nend\n",
        "public function invalid(address: Ffi.C.Size)\n    Ffi.Unsafe.pointerFromAddress<<Array<Int>>>(address)\nend\n",
    ] {
        let module = FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(
                FileId::from_raw(0),
                "src/invalidUnsafeMemory.pop",
                format!("namespace Memory\n{body}"),
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
        assert!(result.hir().is_none(), "{body}");
        assert!(!result.diagnostics().is_empty(), "{body}");
    }
}

#[test]
fn ffi_buffer_with_pointer_requires_one_non_escaping_inline_body() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/withPointer.pop",
            "namespace Memory\n\
             public function inspect(buffer: Ffi.Buffer<Int>): Boolean\n\
                 return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Boolean\n\
                     return Ffi.OptionalPointer.isPresent(pointer)\n\
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
        result.hir().expect("scoped buffer HIR"),
        result.types(),
        pop_driver::artifact_sha256_hex,
    )
    .expect("scoped buffer MIR");
    let dump = mir.dump();
    for operation in [
        "ffiBufferLength",
        "ffiBufferBorrow",
        "callScopedBorrow",
        "ffiBufferEndBorrow",
    ] {
        assert!(dump.contains(operation), "missing {operation}\n{dump}");
    }
    let reparsed = pop_mir::parse_mir_dump(&dump)
        .unwrap_or_else(|error| panic!("scoped FFI MIR text round trip: {error:?}\n{dump}"))
        .with_ffi_layouts(mir.ffi_layouts().clone());
    assert_eq!(reparsed.dump(), dump);
    pop_mir::verify_mir_bubble(&reparsed, result.types())
        .expect("reparsed scoped FFI MIR verifies");
    let corrupted_dump = dump.replacen("region#0 captures[", "region#1 captures[", 1);
    assert_ne!(corrupted_dump, dump, "scoped call region must be present");
    let corrupted = pop_mir::parse_mir_dump(&corrupted_dump)
        .expect("corrupt scoped FFI MIR remains syntactically valid")
        .with_ffi_layouts(mir.ffi_layouts().clone());
    assert!(
        pop_mir::verify_mir_bubble(&corrupted, result.types()).is_err(),
        "corrupt scoped call region must fail independent MIR verification"
    );
    let optimized =
        pop_mir::optimize_mir(reparsed, result.types()).expect("optimized scoped FFI MIR");
    assert!(optimized.dump().contains("callScopedBorrow"));
}

#[test]
fn ffi_buffer_with_pointer_rejects_function_values_async_and_escape() {
    let ffi = BubbleId::from_raw(20);
    for source in [
        "namespace Memory\n\
         function inspectBody(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Boolean\n\
             return Ffi.OptionalPointer.isPresent(pointer)\n\
         end\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Boolean\n\
             return Ffi.Buffer.withPointer(buffer, inspectBody)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Boolean\n\
             return Ffi.Buffer.withPointer(buffer, async function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Boolean\n\
                 return Ffi.OptionalPointer.isPresent(pointer)\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Ffi.OptionalPointer<Int>\n\
             return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Ffi.OptionalPointer<Int>\n\
                 return pointer\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         function retain(pointer: Ffi.OptionalPointer<Int>): Boolean\n\
             return Ffi.OptionalPointer.isPresent(pointer)\n\
         end\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Boolean\n\
             return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Boolean\n\
                 return retain(pointer)\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Ffi.C.Size\n\
             return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Ffi.C.Size\n\
                 return Ffi.Unsafe.address(Ffi.OptionalPointer.require(pointer)?)\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Boolean\n\
             return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Boolean\n\
                 return Ffi.Buffer.withPointer(buffer, function(nestedPointer: Ffi.OptionalPointer<Int>, nestedLength: Ffi.C.Size): Boolean\n\
                     return Ffi.OptionalPointer.isPresent(nestedPointer)\n\
                 end)\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Array<Ffi.OptionalPointer<Int>>\n\
             return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Array<Ffi.OptionalPointer<Int>>\n\
                 return { pointer }\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(buffer: Ffi.Buffer<Int>): Result<Ffi.Pointer<Int>, Ffi.NullPointerError>\n\
             return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Result<Ffi.Pointer<Int>, Ffi.NullPointerError>\n\
                 return Ffi.OptionalPointer.require(pointer)\n\
             end)\n\
         end\n",
    ] {
        let module = FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(FileId::from_raw(0), "src/invalidBorrow.pop", source).expect("source"),
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
        assert!(result.hir().is_none(), "{source}");
        assert!(!result.diagnostics().is_empty(), "{source}");
    }
}

#[test]
fn ffi_buffer_with_pointer_allows_exact_foreign_calls_and_balances_unwind() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/scopedForeign.pop",
            "namespace Memory\n\
             @Ffi.Foreign(\"inspect_pointer\", abi = \"CUnwind\")\n\
             internal function inspectForeign(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Ffi.C.Int\n\
             end\n\
             public function inspect(buffer: Ffi.Buffer<Int>): Ffi.C.Int\n\
                 return Ffi.Buffer.withPointer(buffer, function(pointer: Ffi.OptionalPointer<Int>, length: Ffi.C.Size): Ffi.C.Int\n\
                     return inspectForeign(pointer, length)\n\
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
        result.hir().expect("scoped foreign HIR"),
        result.types(),
        pop_driver::artifact_sha256_hex,
    )
    .expect("scoped foreign MIR");
    let dump = mir.dump();
    assert!(dump.contains("callForeign"), "{dump}");
    assert!(dump.contains("callScopedBorrow"), "{dump}");
    assert!(dump.contains("unwind cleanup:b"), "{dump}");
    assert_eq!(dump.matches("ffiBufferEndBorrow").count(), 2, "{dump}");
}

#[test]
fn ffi_with_pin_lowers_one_inline_immutable_bytes_payload_borrow() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/withPin.pop",
            "namespace Memory\n\
             public function inspect(bytes: Bytes): Boolean\n\
                 return Ffi.withPin(bytes, function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Boolean\n\
                     return Ffi.OptionalReadOnlyPointer.isPresent(pointer)\n\
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
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("byte pin HIR"), result.types())
        .expect("byte pin MIR");
    let dump = mir.dump();
    for operation in [
        "ffiBytesBorrow",
        "ffiBytesBorrowLength",
        "callScopedBorrow",
        "ffiBytesEndBorrow",
    ] {
        assert!(dump.contains(operation), "missing {operation}\n{dump}");
    }
}

#[test]
fn ffi_with_pin_rejects_non_bytes_non_inline_async_and_escaping_bodies() {
    let ffi = BubbleId::from_raw(20);
    for source in [
        "namespace Memory\n\
         function inspectBody(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Boolean\n\
             return Ffi.OptionalReadOnlyPointer.isPresent(pointer)\n\
         end\n\
         public function invalid(bytes: Bytes): Boolean\n\
             return Ffi.withPin(bytes, inspectBody)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(value: String): Boolean\n\
             return Ffi.withPin(value, function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Boolean\n\
                 return Ffi.OptionalReadOnlyPointer.isPresent(pointer)\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(bytes: Bytes): Boolean\n\
             return Ffi.withPin(bytes, async function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Boolean\n\
                 return Ffi.OptionalReadOnlyPointer.isPresent(pointer)\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(bytes: Bytes): Boolean\n\
             return Ffi.withPin(bytes, function(pointer: Ffi.OptionalPointer<Byte>, length: Ffi.C.Size): Boolean\n\
                 return Ffi.OptionalPointer.isPresent(pointer)\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         public function invalid(bytes: Bytes): Ffi.OptionalReadOnlyPointer<Byte>\n\
             return Ffi.withPin(bytes, function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Ffi.OptionalReadOnlyPointer<Byte>\n\
                 return pointer\n\
             end)\n\
         end\n",
        "namespace Memory\n\
         function retain(pointer: Ffi.OptionalReadOnlyPointer<Byte>): Boolean\n\
             return Ffi.OptionalReadOnlyPointer.isPresent(pointer)\n\
         end\n\
         public function invalid(bytes: Bytes): Boolean\n\
             return Ffi.withPin(bytes, function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Boolean\n\
                 return retain(pointer)\n\
             end)\n\
         end\n",
    ] {
        let module = FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(FileId::from_raw(0), "src/invalidPin.pop", source).expect("source"),
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
        assert!(result.hir().is_none(), "{source}");
        assert!(!result.diagnostics().is_empty(), "{source}");
    }
}

#[test]
fn ffi_with_pin_allows_an_exact_read_only_foreign_call_and_unwind_cleanup() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/pinnedForeign.pop",
            "namespace Memory\n\
             @Ffi.Foreign(\"inspect_bytes\", abi = \"CUnwind\")\n\
             internal function inspectForeign(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Ffi.C.Int\n\
             end\n\
             public function inspect(bytes: Bytes): Ffi.C.Int\n\
                 return Ffi.withPin(bytes, function(pointer: Ffi.OptionalReadOnlyPointer<Byte>, length: Ffi.C.Size): Ffi.C.Int\n\
                     return inspectForeign(pointer, length)\n\
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
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("byte pin HIR"), result.types())
        .expect("byte pin MIR");
    let dump = mir.dump();
    assert!(dump.contains("callForeign"), "{dump}");
    assert!(dump.contains("unwind cleanup:b"), "{dump}");
    assert_eq!(dump.matches("ffiBytesEndBorrow").count(), 2, "{dump}");
}

#[test]
fn resolved_user_calls_are_not_hijacked_by_ffi_pointer_spelling() {
    let ffi = BubbleId::from_raw(20);
    let declaration = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/userPointer.pop",
            "namespace Ffi.Pointer\npublic function readOnly(value: Int): Int\n    return value\nend\n",
        )
        .expect("source"),
    );
    let caller = FrontEndModule::new(
        ModuleId::from_raw(1),
        SourceFile::new(
            FileId::from_raw(1),
            "src/caller.pop",
            "namespace Caller\npublic function run(): Int\n    return Ffi.Pointer.readOnly(7)\nend\n",
        )
        .expect("source"),
    );
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![declaration, caller],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("user call HIR"), result.types())
        .expect("user call MIR");
    let dump = mir.dump();
    assert!(dump.contains("callDirect"), "{dump}");
    assert!(!dump.contains("ffiPointerReadOnly"), "{dump}");
}

#[test]
fn resolved_user_calls_are_not_hijacked_by_ffi_buffer_spelling() {
    let ffi = BubbleId::from_raw(20);
    let declaration = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/userBuffer.pop",
            "namespace Ffi.Buffer\npublic function length(value: Int): Int\n    return value\nend\n",
        )
        .expect("source"),
    );
    let caller = FrontEndModule::new(
        ModuleId::from_raw(1),
        SourceFile::new(
            FileId::from_raw(1),
            "src/caller.pop",
            "namespace Caller\npublic function run(): Int\n    return Ffi.Buffer.length(7)\nend\n",
        )
        .expect("source"),
    );
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![declaration, caller],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("user call HIR"), result.types())
        .expect("user call MIR");
    let dump = mir.dump();
    assert!(dump.contains("callDirect"), "{dump}");
    assert!(!dump.contains("ffiBufferLength"), "{dump}");
}

#[test]
fn resolved_user_calls_are_not_hijacked_by_ffi_handle_spelling() {
    let ffi = BubbleId::from_raw(20);
    let declaration = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/userHandle.pop",
            "namespace Ffi.Handle\npublic function get(value: Int): Int\n    return value\nend\n",
        )
        .expect("source"),
    );
    let caller = FrontEndModule::new(
        ModuleId::from_raw(1),
        SourceFile::new(
            FileId::from_raw(1),
            "src/caller.pop",
            "namespace Caller\npublic function run(): Int\n    return Ffi.Handle.get(7)\nend\n",
        )
        .expect("source"),
    );
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![declaration, caller],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let mir = pop_mir::lower_hir_bubble(result.hir().expect("user call HIR"), result.types())
        .expect("user call MIR");
    let dump = mir.dump();
    assert!(dump.contains("callDirect"), "{dump}");
    assert!(!dump.contains("ffiHandleGet"), "{dump}");
}

#[test]
#[should_panic(expected = "Pop.Ffi must be a direct Bubble dependency")]
fn front_end_rejects_an_unverified_ffi_bubble() {
    let _ = FrontEndBubbleInput::new(
        BubbleId::from_raw(10),
        NamespaceId::from_raw(10),
        Vec::new(),
        vec![ffi_module()],
    )
    .with_ffi_dependency(BubbleId::from_raw(20));
}

#[test]
fn front_end_resolves_foreign_attributes_to_one_closed_typed_contract() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/native.pop",
            "@Ffi.Link(\"SystemC\")\n\
             namespace Native\n\
             @Ffi.Foreign(\"native_close\", abi = \"System\")\n\
             internal function close(pointer: Ffi.ReadOnlyPointer<Byte>)\n\
             end\n\
             @Ffi.Foreign(\"native_poll\")\n\
             @Ffi.Nonblocking\n\
             internal function poll(value: Ffi.C.Int): Ffi.C.Int\n\
             end\n\
             internal function pollWrapper(value: Ffi.C.Int, retained: String): Ffi.C.Int\n\
                 local result = poll(value)\n\
                 print(retained)\n\
                 return result\n\
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
    let [close, poll] = result.foreign_declarations() else {
        panic!("two typed foreign declarations");
    };
    assert_eq!(close.external_symbol(), "native_close");
    assert_eq!(close.abi(), ForeignAbi::System);
    assert_eq!(close.link_aliases(), ["SystemC"]);
    assert!(close.effects().contains(Effect::ForeignFunction));
    assert!(close.effects().contains(Effect::UnsafeMemory));
    assert!(close.effects().contains(Effect::GcSafePoint));
    assert!(close.effects().contains(Effect::Blocks));
    assert!(!close.effects().contains(Effect::MayUnwind));

    assert_eq!(poll.external_symbol(), "native_poll");
    assert_eq!(poll.abi(), ForeignAbi::C);
    assert_eq!(poll.link_aliases(), ["SystemC"]);
    assert!(!poll.effects().contains(Effect::Blocks));

    let hir = result.hir().expect("verified HIR");
    assert_eq!(hir.foreign_functions().len(), 2);
    assert!(
        hir.functions()
            .iter()
            .any(|function| function.name() == "pollWrapper")
    );
    let wrapper_symbol = hir
        .functions()
        .iter()
        .find(|function| function.name() == "pollWrapper")
        .expect("foreign wrapper")
        .symbol();
    assert!(hir.verify(result.types()).is_ok());
    let dump = hir.dump(result.types());
    assert!(dump.contains("foreign s0"));
    assert!(dump.contains("symbol=\"native_close\""));

    let mir = pop_mir::lower_hir_bubble(hir, result.types()).expect("verified canonical MIR");
    assert_eq!(mir.foreign_functions().len(), 2);
    let mir_close = &mir.foreign_functions()[0];
    assert!(
        mir_close
            .effects()
            .contains(pop_mir::MirEffect::ForeignFunction)
    );
    assert!(
        mir_close
            .effects()
            .contains(pop_mir::MirEffect::UnsafeMemory)
    );
    assert!(
        mir_close
            .effects()
            .contains(pop_mir::MirEffect::GcSafePoint)
    );
    assert!(mir_close.effects().contains(pop_mir::MirEffect::Blocks));
    let mir_dump = mir.dump();
    assert!(mir_dump.contains("foreign s0"));
    assert!(mir_dump.contains("callForeign s1"));
    assert!(!mir_dump.contains("callDirect s1"));
    assert_balanced_foreign_root_contract(&mir, wrapper_symbol);
    let reparsed = pop_mir::parse_mir_dump(&mir_dump)
        .unwrap_or_else(|error| panic!("foreign MIR text round trip: {error:?}\n{mir_dump}"));
    assert_eq!(reparsed.dump(), mir_dump);
    pop_mir::verify_mir_bubble(&reparsed, result.types()).expect("reparsed foreign MIR verifies");
    let optimized = pop_mir::optimize_mir(reparsed, result.types()).expect("optimized foreign MIR");
    assert!(optimized.dump().contains("callForeign s1"));
    assert!(!optimized.dump().contains("callDirect s1"));
}

#[test]
fn foreign_declarations_accept_only_closed_layout_pointer_callback_and_handle_types() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/closedAbi.pop",
            "namespace Native.Unsafe\n\
             @Ffi.C.Layout\n\
             public record Pair\n\
                 left: Int32\n\
                 right: Ffi.C.Int\n\
             end\n\
             private type Callback = function(input: Int32): Int32\n\
             @Ffi.Foreign(\"accept_pair\")\n\
             internal function acceptPair(value: Pair): Pair\n\
             end\n\
             @Ffi.Foreign(\"accept_pair_pointer\")\n\
             internal function acceptPairPointer(value: Ffi.Pointer<Pair>)\n\
             end\n\
             @Ffi.Foreign(\"accept_handle\")\n\
             internal function acceptHandle(value: Ffi.Handle<String>)\n\
             end\n\
             @Ffi.Foreign(\"accept_callback\")\n\
             internal function acceptCallback(value: Ffi.Function<Callback>)\n\
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
    assert_eq!(result.foreign_declarations().len(), 4);
    assert!(result.hir().is_some());
}

#[test]
fn trusted_ffi_attributes_cannot_be_spoofed_by_user_source_names() {
    let ffi = BubbleId::from_raw(20);
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/spoofedForeign.pop",
            "@Ffi.Link(\"UserLibrary\")\n\
             namespace Ffi\n\
             @AttributeUsage(targets = { AttributeTarget.Namespace }, repeatable = false)\n\
             public attribute Link(alias: String)\n\
             @AttributeUsage(targets = { AttributeTarget.Function }, repeatable = false)\n\
             public attribute Foreign(symbol: String)\n\
             @AttributeUsage(targets = { AttributeTarget.Function }, repeatable = false)\n\
             public attribute Nonblocking()\n\
             @Ffi.Foreign(\"user_function\")\n\
             @Ffi.Nonblocking\n\
             internal function userFunction(value: Int32): Int32\n\
                 return value\n\
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
    assert!(result.foreign_declarations().is_empty());
    let [namespace] = result.namespace_attributes() else {
        panic!("one user namespace attribute");
    };
    assert_eq!(namespace.attributes().len(), 1);
}

#[test]
fn inaccessible_user_ffi_attributes_do_not_fall_back_to_trusted_identities() {
    let ffi = BubbleId::from_raw(20);
    let owner = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(
            FileId::from_raw(0),
            "src/userForeign.pop",
            "namespace Ffi\n\
             @AttributeUsage(targets = { AttributeTarget.Function }, repeatable = false)\n\
             private attribute Foreign(symbol: String)\n",
        )
        .expect("source"),
    );
    let consumer = FrontEndModule::new(
        ModuleId::from_raw(1),
        SourceFile::new(
            FileId::from_raw(1),
            "src/consumer.pop",
            "namespace Consumer\n\
             @Ffi.Foreign(\"user_function\")\n\
             internal function userFunction(value: Int32): Int32\n\
                 return value\n\
             end\n",
        )
        .expect("source"),
    );
    let result = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![owner, consumer],
        )
        .with_ffi_dependency(ffi),
    );

    assert!(result.hir().is_none());
    assert!(result.foreign_declarations().is_empty());
    assert!(!result.diagnostic_snapshot().contains("POP5000"));
}

#[test]
fn public_foreign_declarations_require_a_final_unsafe_namespace() {
    let ffi = BubbleId::from_raw(20);
    let accepted = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(10),
            NamespaceId::from_raw(10),
            vec![ffi],
            vec![FrontEndModule::new(
                ModuleId::from_raw(0),
                SourceFile::new(
                    FileId::from_raw(0),
                    "src/publicUnsafe.pop",
                    "namespace Example.Native.Unsafe\n\
                     @Ffi.Foreign(\"native_public\")\n\
                     public function nativePublic(value: Int32): Int32\n\
                     end\n",
                )
                .expect("source"),
            )],
        )
        .with_ffi_dependency(ffi),
    );
    assert!(
        accepted.diagnostics().is_empty(),
        "{}",
        accepted.diagnostic_snapshot()
    );
    assert_eq!(accepted.foreign_declarations().len(), 1);

    for (name, namespace) in [
        ("safe", "Example.Native"),
        ("unsafeNotFinal", "Example.Unsafe.Native"),
        ("wrongCase", "Example.Native.unsafe"),
    ] {
        let source = format!(
            "namespace {namespace}\n\
             @Ffi.Foreign(\"native_public\")\n\
             public function nativePublic(value: Int32): Int32\n\
             end\n"
        );
        let result = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(10),
                NamespaceId::from_raw(10),
                vec![ffi],
                vec![FrontEndModule::new(
                    ModuleId::from_raw(0),
                    SourceFile::new(FileId::from_raw(0), format!("src/{name}.pop"), source)
                        .expect("source"),
                )],
            )
            .with_ffi_dependency(ffi),
        );

        assert!(result.hir().is_none(), "{name} must not reach HIR");
        assert!(
            result.diagnostic_snapshot().contains("POP5000"),
            "{name}: {}",
            result.diagnostic_snapshot()
        );
        assert!(result.foreign_declarations().is_empty());
    }
}

#[test]
fn front_end_rejects_invalid_foreign_declaration_contracts() {
    for (name, declaration) in [
        (
            "body",
            "@Ffi.Foreign(\"native_body\")\n\
             internal function invalid(value: Int32)\n\
                 print(1)\n\
             end\n",
        ),
        (
            "generic",
            "@Ffi.Foreign(\"native_generic\")\n\
             internal function invalid<T>(value: Ffi.Pointer<T>)\n\
             end\n",
        ),
        (
            "managedString",
            "@Ffi.Foreign(\"native_string\")\n\
             internal function invalid(value: String)\n\
             end\n",
        ),
        (
            "boolean",
            "@Ffi.Foreign(\"native_boolean\")\n\
             internal function invalid(value: Boolean)\n\
             end\n",
        ),
        (
            "unannotatedRecord",
            "private record Plain\n\
                 value: Int32\n\
             end\n\
             @Ffi.Foreign(\"native_record\")\n\
             internal function invalid(value: Plain)\n\
             end\n",
        ),
        (
            "managedStringPointer",
            "@Ffi.Foreign(\"native_string_pointer\")\n\
             internal function invalid(value: Ffi.Pointer<String>)\n\
             end\n",
        ),
        (
            "managedStringReadOnlyPointer",
            "@Ffi.Foreign(\"native_string_pointer\")\n\
             internal function invalid(value: Ffi.ReadOnlyPointer<String>)\n\
             end\n",
        ),
        (
            "ownedBuffer",
            "@Ffi.Foreign(\"native_buffer\")\n\
             internal function invalid(value: Ffi.Buffer<Byte>)\n\
             end\n",
        ),
        (
            "scalarHandle",
            "@Ffi.Foreign(\"native_handle\")\n\
             internal function invalid(value: Ffi.Handle<Int32>)\n\
             end\n",
        ),
        (
            "singletonErrorHandle",
            "@Ffi.Foreign(\"native_handle\")\n\
             internal function invalid(value: Ffi.Handle<Ffi.NullPointerError>)\n\
             end\n",
        ),
        (
            "managedCallbackParameter",
            "private type InvalidCallback = function(input: String): Int32\n\
             @Ffi.Foreign(\"native_callback\")\n\
             internal function invalid(value: Ffi.Function<InvalidCallback>)\n\
             end\n",
        ),
        (
            "asyncCallback",
            "private type AsyncCallback = async function(): Int32\n\
             @Ffi.Foreign(\"native_callback\")\n\
             internal function invalid(value: Ffi.Function<AsyncCallback>)\n\
             end\n",
        ),
        (
            "abi",
            "@Ffi.Foreign(\"native_abi\", abi = \"Lua\")\n\
             internal function invalid(value: Int32)\n\
             end\n",
        ),
        (
            "nonblocking",
            "@Ffi.Nonblocking\n\
             internal function invalid(value: Int32)\n\
             end\n",
        ),
    ] {
        assert_invalid_foreign_contract(name, declaration);
    }
}

fn assert_invalid_foreign_contract(name: &str, declaration: &str) {
    let ffi = BubbleId::from_raw(20);
    let source = format!("namespace Native\n{declaration}");
    let module = FrontEndModule::new(
        ModuleId::from_raw(0),
        SourceFile::new(FileId::from_raw(0), format!("src/{name}.pop"), source).expect("source"),
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

    assert!(result.hir().is_none(), "{name} must not reach HIR");
    assert!(
        result.diagnostic_snapshot().contains("POP5000"),
        "{name}: {}",
        result.diagnostic_snapshot()
    );
    assert!(result.foreign_declarations().is_empty());
}
