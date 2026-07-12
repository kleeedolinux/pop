use pop_driver::{FrontEndBubbleInput, FrontEndModule, ReferenceMetadataError, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId, SymbolIdentity};
use pop_mir::{lower_hir_bubble, parse_mir_dump, verify_mir_bubble};
use pop_source::SourceFile;

fn module(raw: u32, path: &str, text: &str) -> FrontEndModule {
    FrontEndModule::new(
        ModuleId::from_raw(raw),
        SourceFile::new(FileId::from_raw(raw), path, text).expect("test source"),
    )
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
fn unsupported_public_signature_types_fail_reference_emission() {
    let bubble = BubbleId::from_raw(2);
    let result = analyze_bubble(FrontEndBubbleInput::new(
        bubble,
        NamespaceId::from_raw(2),
        Vec::new(),
        vec![module(
            0,
            "src/unsupported.pop",
            "namespace Pop.Sequence\n\
             public function first(values: Array<Int>): Int\n\
                 return 0\n\
             end\n",
        )],
    ));
    assert!(result.diagnostics().is_empty());
    assert!(matches!(
        result.reference_metadata(),
        Err(ReferenceMetadataError::UnsupportedPublicType { function, .. })
            if function == SymbolIdentity::new(bubble, pop_foundation::SymbolId::from_raw(0))
    ));
}
