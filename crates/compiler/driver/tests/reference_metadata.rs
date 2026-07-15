#![allow(clippy::too_many_lines)]

use pop_backend_llvm::{LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_driver::{
    FrontEndBubbleInput, FrontEndModule, ReferenceMetadataDecodeError, ReferenceMetadataError,
    analyze_bubble, decode_reference_metadata, encode_reference_metadata,
};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolIdentity};
use pop_mir::{lower_hir_bubble, parse_mir_dump, verify_mir_bubble};
use pop_source::SourceFile;
use pop_target::TargetSpec;
use pop_types::{Effect, ForeignAbi};

fn module(raw: u32, path: &str, text: &str) -> FrontEndModule {
    FrontEndModule::new(
        ModuleId::from_raw(raw),
        SourceFile::new(FileId::from_raw(raw), path, text).expect("test source"),
    )
}

#[test]
fn foreign_contract_and_transitive_effects_survive_public_reference_metadata() {
    let ffi_bubble = BubbleId::from_raw(20);
    let producer_bubble = BubbleId::from_raw(21);
    let producer = analyze_bubble(
        FrontEndBubbleInput::new(
            producer_bubble,
            NamespaceId::from_raw(21),
            vec![ffi_bubble],
            vec![module(
                0,
                "src/native.pop",
                "@Ffi.Link(\"SystemC\")\n\
                 namespace Native.Unsafe\n\
                 @Ffi.Foreign(\"native_poll\", abi = \"CUnwind\")\n\
                 @Ffi.Nonblocking\n\
                 public function poll(value: Ffi.C.Int): Ffi.C.Int\n\
                 end\n\
                 @Ffi.Foreign(\"native_hidden\")\n\
                 internal function hidden(value: Ffi.C.Int): Ffi.C.Int\n\
                 end\n\
                 public function checkedPoll(value: Ffi.C.Int): Ffi.C.Int\n\
                     return poll(value)\n\
                 end\n\
                 public function checkedPollTwice(value: Ffi.C.Int): Ffi.C.Int\n\
                     return checkedPoll(value)\n\
                 end\n",
            )],
        )
        .with_ffi_dependency(ffi_bubble),
    );
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );

    let producer_hir = producer.hir().expect("producer HIR");
    for name in ["checkedPoll", "checkedPollTwice"] {
        let effects = producer_hir
            .functions()
            .iter()
            .find(|function| function.name() == name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .effects();
        assert!(effects.contains(Effect::ForeignFunction), "{name}");
        assert!(effects.contains(Effect::UnsafeMemory), "{name}");
        assert!(effects.contains(Effect::GcSafePoint), "{name}");
        assert!(effects.contains(Effect::MayUnwind), "{name}");
        assert!(!effects.contains(Effect::Blocks), "{name}");
    }

    let metadata = producer.reference_metadata().expect("public FFI metadata");
    assert_eq!(metadata.functions().len(), 3, "internal foreign is omitted");
    let foreign = metadata
        .functions()
        .iter()
        .find(|function| function.name() == "poll")
        .expect("public foreign declaration");
    let declaration = foreign
        .foreign_declaration()
        .expect("exact foreign contract enters reference metadata");
    assert_eq!(declaration.external_symbol(), "native_poll");
    assert_eq!(declaration.abi(), ForeignAbi::CUnwind);
    assert_eq!(declaration.link_aliases(), ["SystemC"]);
    assert!(declaration.is_nonblocking());
    assert_eq!(foreign.effects(), declaration.effects());

    let encoded = encode_reference_metadata(metadata).expect("encode FFI reference metadata");
    let corrupted = String::from_utf8(encoded.clone())
        .expect("canonical metadata is UTF-8")
        .replacen("\"nonblocking\":true", "\"nonblocking\":false", 1)
        .into_bytes();
    assert_eq!(
        decode_reference_metadata(&corrupted),
        Err(ReferenceMetadataDecodeError::InvalidForeignDeclaration(
            foreign.identity()
        ))
    );
    let decoded = decode_reference_metadata(&encoded).expect("decode FFI reference metadata");
    let decoded_foreign = decoded
        .functions()
        .iter()
        .find(|function| function.name() == "poll")
        .and_then(|function| function.foreign_declaration())
        .expect("foreign contract survives canonical round trip");
    assert_eq!(decoded_foreign, declaration);

    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(22),
            NamespaceId::from_raw(22),
            vec![ffi_bubble, producer_bubble],
            vec![module(
                0,
                "src/main.pop",
                "namespace Application\n\
                 public function callForeign(value: Ffi.C.Int): Ffi.C.Int\n\
                     return Native.Unsafe.poll(value)\n\
                 end\n\
                 public function callWrapper(value: Ffi.C.Int): Ffi.C.Int\n\
                     return Native.Unsafe.checkedPollTwice(value)\n\
                 end\n",
            )],
        )
        .with_ffi_dependency(ffi_bubble)
        .with_reference_metadata(vec![decoded]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let consumer_hir = consumer.hir().expect("consumer HIR");
    let foreign_reference = consumer_hir
        .function_references()
        .iter()
        .find(|reference| reference.identity() == foreign.identity())
        .expect("foreign HIR reference");
    assert_eq!(
        foreign_reference
            .foreign_declaration()
            .expect("HIR keeps foreign contract"),
        declaration
    );
    for function in consumer_hir.functions() {
        assert!(function.effects().contains(Effect::ForeignFunction));
        assert!(function.effects().contains(Effect::UnsafeMemory));
        assert!(function.effects().contains(Effect::GcSafePoint));
        assert!(function.effects().contains(Effect::MayUnwind));
        assert!(!function.effects().contains(Effect::Blocks));
    }

    let mir = lower_hir_bubble(consumer_hir, consumer.types()).expect("consumer MIR");
    let referenced_foreign = mir
        .foreign_functions()
        .iter()
        .find(|function| function.reference_identity() == Some(foreign.identity()))
        .expect("referenced foreign declaration becomes canonical MIR foreign identity");
    assert_eq!(referenced_foreign.declaration().abi(), ForeignAbi::CUnwind);
    assert_eq!(
        referenced_foreign.declaration().external_symbol(),
        "native_poll"
    );
    assert_eq!(referenced_foreign.declaration().link_aliases(), ["SystemC"]);
    let mir_dump = mir.dump();
    assert!(mir_dump.contains("callForeign"), "{mir_dump}");
    assert!(!mir_dump.contains("callReference b21:s0"), "{mir_dump}");
    let reparsed = parse_mir_dump(&mir_dump).expect("referenced foreign MIR round trip");
    verify_mir_bubble(&reparsed, consumer.types()).expect("reparsed referenced foreign MIR");
    assert!(reparsed.foreign_functions().iter().any(|function| {
        function.reference_identity() == Some(foreign.identity())
            && function.declaration().abi() == ForeignAbi::CUnwind
    }));
    let foreign_caller = mir
        .functions()
        .iter()
        .find(|function| {
            function.blocks().iter().any(|block| {
                block.instructions().iter().any(|instruction| {
                    matches!(
                        instruction.kind(),
                        pop_mir::MirInstructionKind::CallForeign { .. }
                    )
                })
            })
        })
        .expect("consumer foreign caller");
    let instructions = foreign_caller.blocks()[0].instructions();
    let call_index = instructions
        .iter()
        .position(|instruction| {
            matches!(
                instruction.kind(),
                pop_mir::MirInstructionKind::CallForeign { .. }
            )
        })
        .expect("consumer canonical foreign call");
    let pop_mir::MirInstructionKind::GcSafePoint {
        safe_point: published_safe_point,
        roots: published_roots,
        ..
    } = instructions[call_index - 1].kind()
    else {
        panic!("referenced foreign call must immediately follow its safe point");
    };
    let pop_mir::MirInstructionKind::CallForeign {
        safe_point,
        roots,
        declared_effects,
        ..
    } = instructions[call_index].kind()
    else {
        unreachable!();
    };
    assert_eq!(safe_point, published_safe_point);
    assert_eq!(roots, published_roots);
    assert!(declared_effects.contains(pop_mir::MirEffect::ForeignFunction));
    assert!(declared_effects.contains(pop_mir::MirEffect::MayUnwind));
    let target = TargetSpec::for_triple(mir.ffi_layouts().target()).expect("MIR target");
    let llvm = lower_mir_to_llvm_ir(
        &mir,
        consumer.types(),
        &target,
        LlvmLoweringOptions::default(),
    )
    .expect("referenced CUnwind LLVM lowering")
    .to_string();
    assert!(
        llvm.contains("personality ptr @__gcc_personality_v0"),
        "{llvm}"
    );
    for function in mir.functions() {
        assert!(
            function
                .effects()
                .contains(pop_mir::MirEffect::ForeignFunction)
        );
        assert!(
            function
                .effects()
                .contains(pop_mir::MirEffect::UnsafeMemory)
        );
        assert!(function.effects().contains(pop_mir::MirEffect::GcSafePoint));
        assert!(function.effects().contains(pop_mir::MirEffect::MayUnwind));
        assert!(!function.effects().contains(pop_mir::MirEffect::Blocks));
    }

    let hidden_consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(23),
            NamespaceId::from_raw(23),
            vec![ffi_bubble, producer_bubble],
            vec![module(
                0,
                "src/main.pop",
                "namespace Application\n\
                 internal function invalid(value: Ffi.C.Int): Ffi.C.Int\n\
                     return Native.Unsafe.hidden(value)\n\
                 end\n",
            )],
        )
        .with_ffi_dependency(ffi_bubble)
        .with_reference_metadata(vec![metadata.clone()]),
    );
    assert!(hidden_consumer.hir().is_none());
    assert!(hidden_consumer.diagnostic_snapshot().contains("POP1002"));
}

#[test]
fn public_function_metadata_resolves_in_a_dependent_bubble_by_typed_identity() {
    let standard_bubble = BubbleId::from_raw(2);
    let standard = analyze_bubble(FrontEndBubbleInput::new(
        standard_bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![
            module(
                0,
                "src/contribution.pop",
                "namespace Pop.Math\n\
                 --- <summary>\n\
                 --- Returns the supplied value.\n\
                 --- </summary>\n\
                 ---\n\
                 --- <param name=\"value\">\n\
                 --- The value to return.\n\
                 --- </param>\n\
                 ---\n\
                 --- <returns>\n\
                 --- The unchanged value.\n\
                 --- </returns>\n\
                 public function contributorIdentity(value: Int): Int\n\
                     return value\n\
                 end\n",
            ),
            module(
                1,
                "src/internal.pop",
                "namespace Pop.Math\n\
                 internal function hiddenIdentity(value: Int): Int\n\
                     return value\n\
                 end\n",
            ),
            module(
                2,
                "src/private.pop",
                "namespace Pop.Math\n\
                 private function privateIdentity(value: Int): Int\n\
                     return value\n\
                 end\n",
            ),
        ],
    ));
    assert!(
        standard.diagnostics().is_empty(),
        "{}",
        standard.diagnostic_snapshot()
    );
    let metadata = standard
        .reference_metadata()
        .expect("primitive public metadata");
    assert_eq!(metadata.bubble(), standard_bubble);
    let [function] = metadata.functions() else {
        panic!("only the public function enters reference metadata");
    };
    assert_eq!(
        function.identity(),
        SymbolIdentity::new(standard_bubble, pop_foundation::SymbolId::from_raw(0))
    );
    assert_eq!(function.namespace(), "Pop.Math");
    assert_eq!(function.name(), "contributorIdentity");
    assert_eq!(function.parameters().len(), 1);
    assert_eq!(function.results().len(), 1);
    let [documentation] = standard.checked_documentation() else {
        panic!("one public checked documentation member");
    };
    assert_eq!(documentation.identity(), function.identity());
    assert_eq!(documentation.fragment().children().len(), 3);

    let application_bubble = BubbleId::from_raw(7);
    let application = analyze_bubble(
        FrontEndBubbleInput::new(
            application_bubble,
            NamespaceId::from_raw(7),
            vec![standard_bubble],
            vec![
                module(
                    0,
                    "src/local.pop",
                    "namespace Application\n\
                     internal function localIdentity(value: Int): Int\n\
                         return value\n\
                     end\n",
                ),
                module(
                    1,
                    "src/main.pop",
                    "namespace Application\n\
                     using Pop.Math\n\
                     public function useContribution(value: Int): Int\n\
                         return contributorIdentity(value)\n\
                     end\n",
                ),
            ],
        )
        .with_reference_metadata(vec![metadata.clone()]),
    );
    assert!(
        application.diagnostics().is_empty(),
        "{}",
        application.diagnostic_snapshot()
    );
    let hir = application.hir().expect("consumer HIR");
    assert!(
        hir.dump(application.types())
            .contains("call.reference b2:s0")
    );
    let mir = lower_hir_bubble(hir, application.types()).expect("consumer MIR");
    let dump = mir.dump();
    assert!(dump.contains("callReference b2:s0"));
    let reparsed = parse_mir_dump(&dump)
        .unwrap_or_else(|error| panic!("referenced-call MIR round trip: {error:?}\n{dump}"));
    verify_mir_bubble(&reparsed, application.types()).expect("reparsed referenced-call MIR");
}

#[test]
fn async_identity_survives_reference_metadata_and_dependent_mir_lowering() {
    let producer_bubble = BubbleId::from_raw(12);
    let producer = analyze_bubble(FrontEndBubbleInput::new(
        producer_bubble,
        NamespaceId::from_raw(12),
        Vec::new(),
        vec![module(
            0,
            "src/load.pop",
            "namespace Service\n\
             public async function load(value: Int): Int\n\
                 return value\n\
             end\n",
        )],
    ));
    assert!(
        producer.diagnostics().is_empty(),
        "{}",
        producer.diagnostic_snapshot()
    );
    let metadata = producer
        .reference_metadata()
        .expect("async public metadata");
    let [function] = metadata.functions() else {
        panic!("one async public function");
    };
    assert!(function.is_async());
    let encoded = encode_reference_metadata(metadata).expect("encode async metadata");
    let decoded = decode_reference_metadata(&encoded).expect("decode async metadata");
    assert!(decoded.functions()[0].is_async());

    let consumer = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(13),
            NamespaceId::from_raw(13),
            vec![producer_bubble],
            vec![module(
                0,
                "src/main.pop",
                "namespace Application\n\
                 using Service\n\
                 public async function consume(): Int\n\
                     return await load(42)\n\
                 end\n",
            )],
        )
        .with_reference_metadata(vec![decoded]),
    );
    assert!(
        consumer.diagnostics().is_empty(),
        "{}",
        consumer.diagnostic_snapshot()
    );
    let hir = consumer.hir().expect("async consumer HIR");
    let [reference] = hir.function_references() else {
        panic!("one async HIR reference");
    };
    assert!(reference.is_async());
    let mir = lower_hir_bubble(hir, consumer.types()).expect("async consumer MIR");
    assert!(mir.dump().contains("task.create reference:b12:s0"));
    assert!(mir.function_references()[0].is_async());
}

#[test]
fn dependency_metadata_keeps_visibility_types_and_edges_closed() {
    let standard_bubble = BubbleId::from_raw(2);
    let producer = analyze_bubble(FrontEndBubbleInput::new(
        standard_bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![
            module(
                0,
                "src/contribution.pop",
                "namespace Pop.Math\n\
                 public function acceptsInt(value: Int): Int\n\
                     return value\n\
                 end\n",
            ),
            module(
                1,
                "src/internal.pop",
                "namespace Pop.Math\n\
                 internal function hidden(value: Int): Int\n\
                     return value\n\
                 end\n",
            ),
        ],
    ));
    let metadata = producer.reference_metadata().expect("public metadata");
    assert_eq!(metadata.functions().len(), 1);

    for source in [
        "namespace Application\nusing Pop.Math\npublic function wrong(): Int\n    return acceptsInt(true)\nend\n",
        "namespace Application\nusing Pop.Math\npublic function hiddenCall(): Int\n    return hidden(1)\nend\n",
    ] {
        let result = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(8),
                NamespaceId::from_raw(8),
                vec![standard_bubble],
                vec![module(1, "src/main.pop", source)],
            )
            .with_reference_metadata(vec![metadata.clone()]),
        );
        assert!(!result.diagnostics().is_empty());
        assert!(result.hir().is_none());
    }

    let no_dependency = analyze_bubble(FrontEndBubbleInput::new(
        BubbleId::from_raw(9),
        NamespaceId::from_raw(9),
        Vec::new(),
        vec![module(
            2,
            "src/main.pop",
            "namespace Application\n\
             using Pop.Math\n\
             public function unavailable(value: Int): Int\n\
                 return acceptsInt(value)\n\
             end\n",
        )],
    ));
    assert!(!no_dependency.diagnostics().is_empty());
    assert!(no_dependency.hir().is_none());
}

#[test]
fn unsupported_nominal_public_signature_types_fail_reference_emission() {
    let bubble = BubbleId::from_raw(2);
    let result = analyze_bubble(FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            0,
            "src/unsupported.pop",
            "namespace Pop.Sequence\n\
             public record Token\n\
                 value: Int\n\
             end\n\
             public function identity(value: Token): Token\n\
                 return value\n\
             end\n",
        )],
    ));
    assert!(result.diagnostics().is_empty());
    assert!(matches!(
        result.reference_metadata(),
        Err(ReferenceMetadataError::UnsupportedPublicType { function, .. })
            if function == SymbolIdentity::new(bubble, pop_foundation::SymbolId::from_raw(1))
    ));
}

#[test]
fn generic_reference_metadata_preserves_bounds_and_infers_consumer_calls() {
    let library_bubble = BubbleId::from_raw(2);
    let library = analyze_bubble(FrontEndBubbleInput::new(
        library_bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            0,
            "src/generics.pop",
            "namespace Pop.Sequence\n\
             public function identity<T>(value: T): T\n\
                 return value\n\
             end\n\
             public function select<T, TSource: Iterable<T>>(source: TSource, value: T): T\n\
                 return value\n\
             end\n",
        )],
    ));
    assert!(
        library.diagnostics().is_empty(),
        "{}",
        library.diagnostic_snapshot()
    );
    let metadata = library.reference_metadata().expect("generic metadata");
    assert_eq!(metadata.functions().len(), 2);
    let identity = metadata
        .functions()
        .iter()
        .find(|function| function.name() == "identity")
        .expect("identity metadata");
    assert_eq!(identity.type_parameters().len(), 1);
    assert!(identity.type_parameters()[0].bound().is_none());
    let select = metadata
        .functions()
        .iter()
        .find(|function| function.name() == "select")
        .expect("select metadata");
    assert_eq!(select.type_parameters().len(), 2);
    assert_eq!(select.type_parameters()[1].name(), "TSource");
    assert!(select.type_parameters()[1].bound().is_some());

    let application = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(7),
            NamespaceId::from_raw(7),
            vec![library_bubble],
            vec![module(
                0,
                "src/main.pop",
                "namespace Application\n\
                 using Pop.Sequence\n\
                 public function run(): Int\n\
                     local values: {Int} = {1, 2}\n\
                     return identity(select(values, 7))\n\
                 end\n",
            )],
        )
        .with_reference_metadata(vec![metadata.clone()]),
    );
    assert!(
        application.diagnostics().is_empty(),
        "{}",
        application.diagnostic_snapshot()
    );
    let dump = application
        .hir()
        .expect("generic consumer HIR")
        .dump(application.types());
    assert_eq!(dump.matches("call.reference b2:").count(), 2);
    assert!(dump.contains("<<t"), "{dump}");
}

#[test]
fn portable_generic_capsules_specialize_private_helpers_without_widening_visibility() {
    let library_bubble = BubbleId::from_raw(2);
    let library = analyze_bubble(FrontEndBubbleInput::new(
        library_bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            0,
            "src/generics.pop",
            "namespace Pop.Sequence\n\
             private class UnusedBox<T>\n\
                 private value: T\n\
             end\n\
             private function privateIdentity<T>(value: T): T\n\
                 return value\n\
             end\n\
             public function portableIdentity<T>(value: T): T\n\
                 return privateIdentity(value)\n\
             end\n",
        )],
    ));
    assert!(
        library.diagnostics().is_empty(),
        "{}",
        library.diagnostic_snapshot()
    );
    let metadata = library.reference_metadata().expect("generic metadata");
    let [function] = metadata.functions() else {
        panic!("private capsule helpers must not enter public metadata");
    };
    let capsule = function
        .specialization_capsule()
        .expect("public generic body requires a portable capsule");
    assert_eq!(capsule.schema_version(), 1);
    assert_eq!(capsule.content_sha256().len(), 64);
    assert!(
        capsule
            .content_sha256()
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    );
    assert_eq!(capsule.function_count(), 2);
    assert!(
        !format!("{capsule:?}").contains("UnusedBox"),
        "portable capsules must exclude unrelated private declarations"
    );

    let application = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(7),
            NamespaceId::from_raw(7),
            vec![library_bubble],
            vec![module(
                0,
                "src/main.pop",
                "namespace Application\n\
                 using Pop.Sequence\n\
                 public function run(): Int\n\
                     return portableIdentity(42)\n\
                 end\n",
            )],
        )
        .with_reference_metadata(vec![metadata.clone()]),
    );
    assert!(
        application.diagnostics().is_empty(),
        "{}",
        application.diagnostic_snapshot()
    );
    let hir = application.hir().expect("consumer HIR");
    assert!(hir.dump(application.types()).contains("call.reference b2:"));
    let mir = lower_hir_bubble(hir, application.types()).expect("specialized consumer MIR");
    let dump = mir.dump();
    assert!(!dump.contains("callReference b2:"), "{dump}");
    assert_eq!(dump.matches("function s").count(), 3, "{dump}");
}

#[test]
fn portable_generic_capsules_remap_recursive_types_into_the_consumer_arena() {
    let library_bubble = BubbleId::from_raw(2);
    let library = analyze_bubble(FrontEndBubbleInput::new(
        library_bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            0,
            "src/arrays.pop",
            "namespace Pop.Sequence\n\
             private function privateFirst<T>(values: {T}, value: T): T\n\
                 return value\n\
             end\n\
             public function first<T>(values: {T}, value: T): T\n\
                 return privateFirst(values, value)\n\
             end\n",
        )],
    ));
    assert!(
        library.diagnostics().is_empty(),
        "{}",
        library.diagnostic_snapshot()
    );
    let metadata = library.reference_metadata().expect("array capsule").clone();
    let application = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(7),
            NamespaceId::from_raw(7),
            vec![library_bubble],
            vec![module(
                0,
                "src/main.pop",
                "namespace Application\n\
                 using Pop.Sequence\n\
                 private function localIdentity<T>(value: T): T\n\
                     return value\n\
                 end\n\
                 public function run(): Int\n\
                     local values: {Int} = {42}\n\
                     return first(values, 42)\n\
                 end\n",
            )],
        )
        .with_reference_metadata(vec![metadata]),
    );
    assert!(
        application.diagnostics().is_empty(),
        "{}",
        application.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(
        application.hir().expect("consumer HIR"),
        application.types(),
    )
    .expect("recursive capsule types remap before specialization");
    assert!(!mir.dump().contains("callReference b2:"));
}

#[test]
fn canonical_reference_metadata_round_trips_portable_generic_capsules() {
    let library_bubble = BubbleId::from_raw(2);
    let library = analyze_bubble(FrontEndBubbleInput::new(
        library_bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            0,
            "src/generics.pop",
            "namespace Pop.Sequence\n\
             private function privateFirst<T>(values: {T}, value: T): T\n\
                 return value\n\
             end\n\
             public function first<T>(values: {T}, value: T): T\n\
                 return privateFirst(values, value)\n\
             end\n",
        )],
    ));
    assert!(
        library.diagnostics().is_empty(),
        "{}",
        library.diagnostic_snapshot()
    );
    let metadata = library.reference_metadata().expect("generic metadata");
    let first = encode_reference_metadata(metadata).expect("canonical reference metadata");
    let second = encode_reference_metadata(metadata).expect("stable reference metadata");
    assert_eq!(first, second);
    assert_eq!(first.last(), Some(&b'\n'));
    assert!(!first[..first.len() - 1].contains(&b'\n'));

    let decoded = decode_reference_metadata(&first).expect("verified reference metadata");
    assert_eq!(&decoded, metadata);
    assert_eq!(
        encode_reference_metadata(&decoded).expect("canonical re-encoding"),
        first
    );

    let application = analyze_bubble(
        FrontEndBubbleInput::new(
            BubbleId::from_raw(7),
            NamespaceId::from_raw(7),
            vec![library_bubble],
            vec![module(
                0,
                "src/main.pop",
                "namespace Application\n\
                 using Pop.Sequence\n\
                 public function run(): Int\n\
                     local values: {Int} = {42}\n\
                     return first(values, 42)\n\
                 end\n",
            )],
        )
        .with_reference_metadata(vec![decoded]),
    );
    assert!(
        application.diagnostics().is_empty(),
        "{}",
        application.diagnostic_snapshot()
    );
    let mir = lower_hir_bubble(
        application.hir().expect("consumer HIR"),
        application.types(),
    )
    .expect("decoded capsule specializes");
    assert!(!mir.dump().contains("callReference b2:"));

    let mut noncanonical = first;
    noncanonical.insert(1, b' ');
    assert_eq!(
        decode_reference_metadata(&noncanonical),
        Err(ReferenceMetadataDecodeError::NonCanonical)
    );
}
