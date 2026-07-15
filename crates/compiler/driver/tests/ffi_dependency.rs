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
        let ffi = BubbleId::from_raw(20);
        let source = format!("namespace Native\n{declaration}");
        let module = FrontEndModule::new(
            ModuleId::from_raw(0),
            SourceFile::new(FileId::from_raw(0), format!("src/{name}.pop"), source)
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

        assert!(result.hir().is_none(), "{name} must not reach HIR");
        assert!(
            result.diagnostic_snapshot().contains("POP5000"),
            "{name}: {}",
            result.diagnostic_snapshot()
        );
        assert!(result.foreign_declarations().is_empty());
    }
}
