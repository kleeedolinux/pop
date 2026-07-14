use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_file, parse_function_signature};
use pop_types::{ResolvedTypeKind, SemanticType, SignatureResolver, embedded_bootstrap_schema};

fn function_signature(
    source: &SourceFile,
    syntax: &pop_syntax::SyntaxTree,
) -> pop_syntax::FunctionSignatureSyntax {
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    parse_function_signature(source, syntax, function).expect("signature")
}

#[test]
fn foundational_and_generic_types_resolve_to_canonical_ids() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/main.pop",
        "namespace Main\n\
         public function transform<T>(\n\
             values: Array<T>,\n\
             lookup: Table<String, T>\n\
         ): Result<T, String>\n\
             return nil\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let parsed = function_signature(&source, &syntax);
    let indexed = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(0),
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let function = indexed
        .index()
        .declaration_by_qualified_name("Main.transform", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut type_resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let resolution_result = type_resolver.resolve(ModuleId::from_raw(0), function, &parsed);

    assert!(
        resolution_result.diagnostics().is_empty(),
        "{}",
        resolution_result.diagnostic_snapshot()
    );
    let signature = resolution_result.signature().expect("resolved signature");
    let type_parameter = signature.type_parameters()[0].type_id();
    let array = signature.parameters()[0]
        .parameter_type()
        .type_id()
        .expect("canonical Array<T>");
    let table = signature.parameters()[1]
        .parameter_type()
        .type_id()
        .expect("canonical Table<String,T>");
    let result = signature.results()[0]
        .type_id()
        .expect("canonical Result<T,String>");

    assert_eq!(
        type_resolver.arena().get(array),
        Some(&SemanticType::Array(type_parameter))
    );
    assert!(matches!(
        type_resolver.arena().get(table),
        Some(SemanticType::Table { key: _, value }) if *value == type_parameter
    ));
    assert!(matches!(
        type_resolver.arena().get(result),
        Some(SemanticType::Builtin { arguments, .. }) if arguments.len() == 2
    ));
}

#[test]
fn nominal_generic_bounds_resolve_against_only_earlier_parameters() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/bounds.pop",
        "namespace Main\n\
         private function consume<T, TSource: Iterable<T>>(source: TSource)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let parsed = function_signature(&source, &syntax);
    let indexed = build_declaration_index(&[ModuleInput::new(
        ModuleId::from_raw(0),
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let function = indexed
        .index()
        .declaration_by_qualified_name("Main.consume", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let result = resolver.resolve(ModuleId::from_raw(0), function, &parsed);

    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let signature = result.signature().expect("bounded signature");
    assert!(signature.type_parameters()[0].bound().is_none());
    let bound = signature.type_parameters()[1]
        .bound()
        .expect("nominal bound");
    assert!(matches!(
        resolver.arena().get(bound),
        Some(SemanticType::Builtin { arguments, .. })
            if arguments == &[signature.type_parameters()[0].type_id()]
    ));
}

#[test]
fn invalid_or_forward_generic_bounds_fail_closed() {
    for text in [
        "namespace Main\nprivate function invalid<T: Int>(value: T)\nend\n",
        "namespace Main\nprivate function invalid<TSource: Iterable<T>, T>(value: TSource)\nend\n",
    ] {
        let source =
            SourceFile::new(FileId::from_raw(0), "src/invalidBounds.pop", text).expect("source");
        let syntax = parse_file(&source);
        let parsed = function_signature(&source, &syntax);
        let indexed = build_declaration_index(&[ModuleInput::new(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            &source,
            &syntax,
        )]);
        let function = indexed
            .index()
            .declaration_by_qualified_name("Main.invalid", SymbolSpace::Value)[0]
            .symbol();
        let database = ResolutionDatabase::new(indexed.into_index());
        let mut resolver =
            SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
        let result = resolver.resolve(ModuleId::from_raw(0), function, &parsed);

        assert!(
            result.signature().is_none(),
            "invalid bound must not type-check"
        );
        assert!(
            !result.diagnostics().is_empty(),
            "invalid bound needs a diagnostic"
        );
    }
}

#[test]
fn accessible_user_types_resolve_to_stable_symbols() {
    let models = SourceFile::new(
        FileId::from_raw(0),
        "src/models.pop",
        "namespace Game.Models\npublic record Player\nend\n",
    )
    .expect("models");
    let service = SourceFile::new(
        FileId::from_raw(1),
        "src/service.pop",
        "namespace Game.Service\n\
         using Game.Models\n\
         public function save(player: Player): Game.Models.Player\n\
             return player\n\
         end\n",
    )
    .expect("service");
    let models_syntax = parse_file(&models);
    let service_syntax = parse_file(&service);
    let parsed = function_signature(&service, &service_syntax);
    let indexed = build_declaration_index(&[
        ModuleInput::new(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            &models,
            &models_syntax,
        ),
        ModuleInput::new(
            ModuleId::from_raw(1),
            BubbleId::from_raw(0),
            &service,
            &service_syntax,
        ),
    ]);
    let player = indexed
        .index()
        .declaration_by_qualified_name("Game.Models.Player", SymbolSpace::Type)[0]
        .symbol();
    let function = indexed
        .index()
        .declaration_by_qualified_name("Game.Service.save", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut type_resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let resolution_result = type_resolver.resolve(ModuleId::from_raw(1), function, &parsed);
    let signature = resolution_result.signature().expect("signature");

    assert!(resolution_result.diagnostics().is_empty());
    assert!(matches!(
        signature.parameters()[0].parameter_type().kind(),
        ResolvedTypeKind::Declaration { symbol, .. } if *symbol == player
    ));
    assert!(matches!(
        signature.results()[0].kind(),
        ResolvedTypeKind::Declaration { symbol, .. } if *symbol == player
    ));
}

#[test]
fn arity_duplicate_generic_and_unknown_type_errors_are_structured() {
    for (signature_text, expected) in [
        (
            "public function bad(value: Array<String, Boolean>)\nend\n",
            "POP2001",
        ),
        ("public function bad<T, T>(value: T)\nend\n", "POP2002"),
        ("public function bad(value: Missing)\nend\n", "POP1002"),
    ] {
        let text = format!("namespace Main\n{signature_text}");
        let source = SourceFile::new(FileId::from_raw(0), "src/main.pop", text).expect("source");
        let syntax = parse_file(&source);
        let parsed = function_signature(&source, &syntax);
        let indexed = build_declaration_index(&[ModuleInput::new(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            &source,
            &syntax,
        )]);
        let function = indexed
            .index()
            .declaration_by_qualified_name("Main.bad", SymbolSpace::Value)[0]
            .symbol();
        let database = ResolutionDatabase::new(indexed.into_index());
        let mut type_resolver =
            SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
        let resolution_result = type_resolver.resolve(ModuleId::from_raw(0), function, &parsed);

        assert!(resolution_result.signature().is_none());
        assert!(
            resolution_result
                .diagnostic_snapshot()
                .starts_with(expected)
        );
    }
}

#[test]
fn inaccessible_signature_types_preserve_visibility_diagnostics() {
    let private_type = SourceFile::new(
        FileId::from_raw(0),
        "src/private.pop",
        "namespace Hidden\nprivate record Secret\nend\n",
    )
    .expect("private");
    let consumer = SourceFile::new(
        FileId::from_raw(1),
        "src/main.pop",
        "namespace Main\nusing Hidden\npublic function reveal(value: Secret)\nend\n",
    )
    .expect("consumer");
    let private_syntax = parse_file(&private_type);
    let consumer_syntax = parse_file(&consumer);
    let parsed = function_signature(&consumer, &consumer_syntax);
    let indexed = build_declaration_index(&[
        ModuleInput::new(
            ModuleId::from_raw(0),
            BubbleId::from_raw(0),
            &private_type,
            &private_syntax,
        ),
        ModuleInput::new(
            ModuleId::from_raw(1),
            BubbleId::from_raw(0),
            &consumer,
            &consumer_syntax,
        ),
    ]);
    let function = indexed
        .index()
        .declaration_by_qualified_name("Main.reveal", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut type_resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let resolution_result = type_resolver.resolve(ModuleId::from_raw(1), function, &parsed);

    assert!(resolution_result.signature().is_none());
    assert_eq!(
        resolution_result.diagnostics()[0].code().as_str(),
        "POP1004"
    );
}
