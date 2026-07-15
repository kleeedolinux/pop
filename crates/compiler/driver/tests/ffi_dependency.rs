use pop_driver::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
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
             internal function close(pointer: Ffi.Pointer<Byte>)\n\
             end\n\
             @Ffi.Foreign(\"native_poll\")\n\
             @Ffi.Nonblocking\n\
             internal function poll(): Ffi.C.Int\n\
             end\n\
             internal function pollWrapper(): Ffi.C.Int\n\
                 return poll()\n\
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
    let reparsed = pop_mir::parse_mir_dump(&mir_dump).expect("foreign MIR text round trip");
    assert_eq!(reparsed.dump(), mir_dump);
    pop_mir::verify_mir_bubble(&reparsed, result.types()).expect("reparsed foreign MIR verifies");
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
