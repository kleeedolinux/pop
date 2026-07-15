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
