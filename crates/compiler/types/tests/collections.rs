use std::collections::BTreeMap;

use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_file, parse_function_body, parse_function_signature};
use pop_types::{
    BodyChecker, SemanticType, SignatureResolver, TypedExpressionKind, TypedStatementKind,
    embedded_bootstrap_schema,
};

struct CollectionFixture {
    result: pop_types::TypedBodyResult,
    arena: pop_types::TypeArena,
}

fn check_function(source_text: &str) -> CollectionFixture {
    let module = ModuleId::from_raw(0);
    let source =
        SourceFile::new(FileId::from_raw(0), "src/collections.pop", source_text).expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");
    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let signature_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature");
    let body =
        parse_function_body(&source, &syntax, function_node, &signature_syntax).expect("body");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.collections", SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let signature_result = resolver.resolve(module, symbol, &signature_syntax);
    assert!(signature_result.diagnostics().is_empty());
    let signature = signature_result.signature().expect("signature").clone();
    let signatures = BTreeMap::from([(symbol, signature.clone())]);
    let result = BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body);
    CollectionFixture {
        result,
        arena: resolver.into_arena(),
    }
}

#[test]
fn checks_luau_shaped_array_and_named_table_literals_against_declared_types() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(): ({String}, {[String]: Int})\n\
             local names: {String} = { \"Alice\", \"Bruno\" }\n\
             local scores: {[String]: Int} = {\n\
                 alice = 10,\n\
                 bruno = 12,\n\
             }\n\
             return (names, scores)\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local {
        local_type,
        initializer,
        ..
    } = body.statements()[0].kind()
    else {
        panic!("array local");
    };
    assert!(matches!(
        fixture.arena.get(*local_type),
        Some(SemanticType::Array(element))
            if matches!(fixture.arena.get(*element), Some(SemanticType::Primitive(_)))
    ));
    assert!(matches!(
        initializer.kind(),
        TypedExpressionKind::Array(elements) if elements.len() == 2
    ));

    let TypedStatementKind::Local {
        local_type,
        initializer,
        ..
    } = body.statements()[1].kind()
    else {
        panic!("table local");
    };
    assert!(matches!(
        fixture.arena.get(*local_type),
        Some(SemanticType::Table { key, value })
            if *key == fixture.arena.source_type("String").expect("String")
                && *value == fixture.arena.source_type("Int").expect("Int")
    ));
    assert!(matches!(
        initializer.kind(),
        TypedExpressionKind::Table(entries)
            if entries.len() == 2
                && entries[0].key().type_id()
                    == fixture.arena.source_type("String").expect("String")
                && entries[0].value().type_id()
                    == fixture.arena.source_type("Int").expect("Int")
    ));
}

#[test]
fn accepts_empty_collections_only_when_the_annotation_supplies_the_type() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(): ({String}, {[String]: Int})\n\
             local names: {String} = {}\n\
             local scores: {[String]: Int} = {}\n\
             return (names, scores)\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
}

#[test]
fn rejects_untyped_or_incompatible_collection_literals_without_dynamic_fallback() {
    for (body, expected_code) in [
        ("local values = { 1 }\nreturn", "POP2007"),
        (
            "local values: {String} = { \"valid\", 1 }\nreturn",
            "POP2003",
        ),
        (
            "local values: {[Int]: String} = { name = \"value\" }\nreturn",
            "POP2003",
        ),
    ] {
        let source = format!(
            "namespace Example\n\
             public function collections()\n\
                 {body}\n\
             end\n"
        );
        let fixture = check_function(&source);
        assert!(fixture.result.body().is_none());
        assert!(
            fixture
                .result
                .diagnostic_snapshot()
                .starts_with(expected_code)
        );
    }
}

#[test]
fn array_indexing_is_resolved_to_an_optional_element_type() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(values: {String}): String?\n\
             return values[1]\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Return { values } = body.statements()[0].kind() else {
        panic!("return");
    };
    assert!(matches!(
        values[0].kind(),
        TypedExpressionKind::ArrayGet { array, index }
            if matches!(array.kind(), TypedExpressionKind::Parameter(_))
                && index.type_id() == fixture.arena.source_type("Int").expect("Int")
    ));
    let string = fixture.arena.source_type("String").expect("String");
    let nil = fixture.arena.source_type("nil").expect("nil");
    assert!(matches!(
        fixture.arena.get(values[0].type_id()),
        Some(SemanticType::Union(members)) if members == &[nil, string] || members == &[string, nil]
    ));
}

#[test]
fn array_indexing_rejects_non_integer_indices_and_non_array_bases() {
    for (parameter, expression, expected_code) in [
        ("values: {String}", "values[true]", "POP2003"),
        ("value: String", "value[1]", "POP2005"),
    ] {
        let source = format!(
            "namespace Example\n\
             public function collections({parameter}): String?\n\
                 return {expression}\n\
             end\n"
        );
        let fixture = check_function(&source);
        assert!(fixture.result.body().is_none());
        assert!(
            fixture
                .result
                .diagnostic_snapshot()
                .starts_with(expected_code)
        );
    }
}

#[test]
fn indexed_array_assignment_is_statically_typed() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(): {Int}\n\
             local values: {Int} = { 0 }\n\
             values[1] = 42\n\
             return values\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    assert!(matches!(
        body.statements()[1].kind(),
        TypedStatementKind::ArraySet { array, index, value }
            if matches!(array.kind(), TypedExpressionKind::Local(_))
                && index.type_id() == fixture.arena.source_type("Int").expect("Int")
                && value.type_id() == fixture.arena.source_type("Int").expect("Int")
    ));
}

#[test]
fn indexed_array_assignment_rejects_the_wrong_element_type() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections()\n\
             local values: {Int} = { 0 }\n\
             values[1] = \"wrong\"\n\
         end\n",
    );

    assert!(fixture.result.body().is_none());
    assert!(fixture.result.diagnostic_snapshot().starts_with("POP2003"));
}

#[test]
fn fixed_array_core_operations_are_fully_typed() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(): (Int, Int, Int?, {Int})\n\
             local values = Array.create<<Int>>(4, 0)\n\
             Array.fill(values, 7)\n\
             values[1] = 3\n\
             local first = Array.get(values, 1)\n\
             local missing = values[5]\n\
             return (Array.length(values), first, missing, values)\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    assert!(fixture.result.body().is_some());
}

#[test]
fn fixed_array_core_operations_reject_incompatible_operands() {
    for statement in [
        "local values = Array.create<<Int>>(4, \"wrong\")",
        "local values = Array.create<<Int>>(true, 0)",
        "local length = Array.length(1)",
        "local values: {Int} = { 0 }\nlocal value = Array.get(values, true)",
        "local values: {Int} = { 0 }\nArray.fill(values, \"wrong\")",
    ] {
        let source = format!(
            "namespace Example\n\
             public function collections()\n\
                 {statement}\n\
             end\n"
        );
        let fixture = check_function(&source);
        assert!(fixture.result.body().is_none(), "{statement}");
        assert!(
            fixture.result.diagnostic_snapshot().starts_with("POP2003"),
            "{statement}: {}",
            fixture.result.diagnostic_snapshot()
        );
    }
}
