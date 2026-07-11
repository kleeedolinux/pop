use std::collections::BTreeMap;

use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_file, parse_function_body, parse_function_signature, parse_record_declaration,
};
use pop_types::{
    BodyChecker, RecordDefinition, RecordDefinitionResult, SignatureResolver, TypeArena,
    TypedExpressionKind, TypedStatementKind, embedded_bootstrap_schema,
};

struct RecordFixture {
    result: pop_types::TypedBodyResult,
    definition: RecordDefinition,
}

fn define_record(text: &str) -> (RecordDefinitionResult, TypeArena) {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/record.pop", text).expect("source");
    let syntax = parse_file(&source);
    let record_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::RecordDeclaration)
        .expect("record");
    let record_syntax =
        parse_record_declaration(&source, &syntax, record_node).expect("record syntax");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let record_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Record", SymbolSpace::Type)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let result = resolver.define_record(module, record_symbol, &record_syntax);
    (result, resolver.into_arena())
}

fn check_record_program(text: &str) -> RecordFixture {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/player.pop", text).expect("source");
    let syntax = parse_file(&source);
    let record_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::RecordDeclaration)
        .expect("record");
    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let record_syntax =
        parse_record_declaration(&source, &syntax, record_node).expect("record syntax");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body =
        parse_function_body(&source, &syntax, function_node, &function_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let record_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.Player", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.update", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let definition_result = resolver.define_record(module, record_symbol, &record_syntax);
    assert!(
        definition_result.diagnostics().is_empty(),
        "{}",
        definition_result.diagnostic_snapshot()
    );
    let definition = definition_result
        .definition()
        .expect("record definition")
        .clone();
    let signature_result = resolver.resolve(module, function_symbol, &function_syntax);
    assert!(signature_result.diagnostics().is_empty());
    let signature = signature_result.signature().expect("signature").clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    let result = BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body);
    RecordFixture { result, definition }
}

#[test]
fn typed_record_literals_fields_and_with_updates_use_resolved_field_ids() {
    let fixture = check_record_program(
        "namespace Example\n\
         public record Player\n\
             name: String\n\
             score: Int = 0\n\
         end\n\
         public function update(player: Player): Player\n\
             local replacement: Player = {\n\
                 name = \"Ana\",\n\
                 score = 10,\n\
             }\n\
             return replacement with {\n\
                 score = player.score + 1,\n\
             }\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("local");
    };
    assert!(matches!(
        initializer.kind(),
        TypedExpressionKind::Record { fields, .. }
            if fields.iter().map(pop_types::TypedFieldValue::field).collect::<Vec<_>>()
                == fixture
                    .definition
                    .fields()
                    .iter()
                    .map(pop_types::RecordFieldDefinition::field)
                    .collect::<Vec<_>>()
    ));
    let TypedStatementKind::Return { values } = body.statements()[1].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        TypedExpressionKind::RecordUpdate { fields, .. }
            if fields[0].field() == fixture.definition.fields()[1].field()
    ));
}

#[test]
fn record_defaults_are_typed_and_materialized_for_omitted_fields() {
    let fixture = check_record_program(
        "namespace Example\n\
         public record Player\n\
             name: String\n\
             score: Int = 7\n\
             enabled: Boolean = true\n\
         end\n\
         public function update(player: Player): Player\n\
             local replacement: Player = { name = \"Ana\", }\n\
             return replacement\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("local");
    };
    let TypedExpressionKind::Record { fields, .. } = initializer.kind() else {
        panic!("record");
    };
    assert_eq!(fields.len(), fixture.definition.fields().len());
    assert_eq!(
        fields
            .iter()
            .map(pop_types::TypedFieldValue::field)
            .collect::<Vec<_>>(),
        fixture
            .definition
            .fields()
            .iter()
            .map(pop_types::RecordFieldDefinition::field)
            .collect::<Vec<_>>()
    );
    assert!(matches!(
        fields[1].value().kind(),
        TypedExpressionKind::Integer(value) if value.to_string() == "7"
    ));
    assert!(matches!(
        fields[2].value().kind(),
        TypedExpressionKind::Boolean(true)
    ));
}

#[test]
fn record_field_defaults_evaluate_pure_typed_arithmetic() {
    let fixture = check_record_program(
        "namespace Example\n\
         public record Player\n\
             score: Int = 1 + 2 * 3\n\
         end\n\
         public function update(player: Player): Player\n\
             local replacement: Player = {}\n\
             return replacement\n\
         end\n",
    );

    assert!(fixture.result.diagnostics().is_empty());
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("local");
    };
    assert!(matches!(
        initializer.kind(),
        TypedExpressionKind::Record { fields, .. }
            if matches!(fields[0].value().kind(), TypedExpressionKind::Integer(value) if value.to_string() == "7")
    ));
}

#[test]
fn record_field_defaults_reject_type_mismatches_and_ineligible_runtime_calls() {
    for (field, expected_code) in [
        ("value: Boolean = 1", "POP2003"),
        ("value: Int = runtimeValue()", "POP4001"),
        ("value: Int = 9223372036854775807 + 1", "POP4002"),
        ("value: Int = 1 / 0", "POP4003"),
    ] {
        let (result, _) = define_record(&format!(
            "namespace Example\n\
             public record Record\n\
                 {field}\n\
             end\n"
        ));

        assert!(result.definition().is_none());
        assert!(
            result.diagnostic_snapshot().contains(expected_code),
            "{}",
            result.diagnostic_snapshot()
        );
    }
}

#[test]
fn aggregate_literals_require_context_and_exact_known_fields() {
    for (body, expected) in [
        ("local value = {}\nreturn player", "POP2007"),
        (
            "local value: Player = { score = 1, }\nreturn value",
            "POP2008",
        ),
        (
            "local value: Player = { name = \"Ana\", score = 1, extra = 2, }\nreturn value",
            "POP2009",
        ),
        (
            "local value: Player = { name = \"Ana\", name = \"Other\", score = 1, }\nreturn value",
            "POP2010",
        ),
    ] {
        let source = format!(
            "namespace Example\n\
             public record Player\n\
                 name: String\n\
                 score: Int\n\
             end\n\
             public function update(player: Player): Player\n\
                 {body}\n\
             end\n"
        );
        let fixture = check_record_program(&source);
        assert!(fixture.result.body().is_none());
        assert!(fixture.result.diagnostic_snapshot().starts_with(expected));
    }
}
