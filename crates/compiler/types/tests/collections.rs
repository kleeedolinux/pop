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
fn generalized_for_resolves_array_and_table_protocols_with_exact_bindings() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(values: {Int}, scores: {[String]: Int}): Int\n\
             local total = 0\n\
             for value in values do\n\
                 total += value\n\
             end\n\
             for key, value in scores do\n\
                 total += value\n\
             end\n\
             return total\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let statements = fixture.result.body().expect("typed body").statements();
    assert!(matches!(
        statements[1].kind(),
        TypedStatementKind::GeneralizedFor {
            source: pop_types::TypedIterationSource::Array,
            bindings,
            ..
        } if bindings.len() == 1
            && bindings[0].local_type() == fixture.arena.source_type("Int").expect("Int")
    ));
    assert!(matches!(
        statements[2].kind(),
        TypedStatementKind::GeneralizedFor {
            source: pop_types::TypedIterationSource::Table,
            bindings,
            ..
        } if bindings.len() == 2
            && bindings[0].local_type() == fixture.arena.source_type("String").expect("String")
            && bindings[1].local_type() == fixture.arena.source_type("Int").expect("Int")
    ));
}

#[test]
fn generalized_for_accepts_exact_list_and_protocol_instances() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(\n\
             values: List<Int>,\n\
             iterable: Iterable<String>,\n\
             iterator: Iterator<Int?>,\n\
         ): Int\n\
             local count = 0\n\
             for value in values do\n\
                 count += value\n\
             end\n\
             for text in iterable do\n\
                 print(text)\n\
             end\n\
             for optional in iterator do\n\
                 if local value = optional then\n\
                     count += value\n\
                 end\n\
             end\n\
             return count\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let statements = fixture.result.body().expect("typed body").statements();
    for (statement, source) in [
        (&statements[1], pop_types::TypedIterationSource::List),
        (&statements[2], pop_types::TypedIterationSource::Iterable),
        (&statements[3], pop_types::TypedIterationSource::Iterator),
    ] {
        assert!(matches!(
            statement.kind(),
            TypedStatementKind::GeneralizedFor { source: actual, .. } if *actual == source
        ));
    }
    let TypedStatementKind::GeneralizedFor { bindings, .. } = statements[3].kind() else {
        panic!("iterator loop");
    };
    assert!(matches!(
        fixture.arena.get(bindings[0].local_type()),
        Some(SemanticType::Union(members))
            if members.contains(&fixture.arena.source_type("Int").expect("Int"))
                && members.contains(&fixture.arena.source_type("nil").expect("nil"))
    ));
}

#[test]
fn generalized_for_rejects_nonprotocol_sources_and_invalid_bindings() {
    for (source, code) in [
        (
            "namespace Example\n\
             public function collections()\n\
                 for value in 1 do\n\
                     value\n\
                 end\n\
             end\n",
            "POP2005",
        ),
        (
            "namespace Example\n\
             public function collections(values: {Int})\n\
                 for left, right in values do\n\
                     left\n\
                 end\n\
             end\n",
            "POP2004",
        ),
        (
            "namespace Example\n\
             public function collections(entries: {[String]: Int})\n\
                 for value, value in entries do\n\
                     value\n\
                 end\n\
             end\n",
            "POP2023",
        ),
        (
            "namespace Example\n\
             public function collections(values: {Int})\n\
                 for value in values do\n\
                     value = 2\n\
                 end\n\
             end\n",
            "POP2005",
        ),
    ] {
        let fixture = check_function(source);
        assert!(fixture.result.body().is_none());
        assert!(
            fixture.result.diagnostic_snapshot().contains(code),
            "{}",
            fixture.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn generalized_for_rejects_proven_list_growth_but_allows_replacement() {
    let growth = check_function(
        "namespace Example\n\
         public function collections(values: List<Int>)\n\
             for value in values do\n\
                 List.add(values, value)\n\
             end\n\
         end\n",
    );
    assert_eq!(growth.result.diagnostic_snapshot(), "POP2029@88..111\n");

    let replacement = check_function(
        "namespace Example\n\
         public function collections(values: List<Int>)\n\
             for value in values do\n\
                 values[1] = value\n\
             end\n\
         end\n",
    );
    assert!(
        replacement.result.diagnostics().is_empty(),
        "{}",
        replacement.result.diagnostic_snapshot()
    );
}

#[test]
fn checks_the_closed_growable_list_surface_with_exact_element_types() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(index: Int): (List<Int>, Int?, Int, Int)\n\
             local values = List.create<<Int>>()\n\
             local reserved = List.withCapacity<<Int>>(8)\n\
             List.add(values, 42)\n\
             local optional = values[index]\n\
             local value = List.get(values, index)\n\
             values[index] = value\n\
             return (reserved, optional, value, List.length(values))\n\
         end\n",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let statements = fixture.result.body().expect("typed body").statements();
    assert!(matches!(
        statements[0].kind(),
        TypedStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), TypedExpressionKind::ListCreate { capacity: None })
    ));
    assert!(matches!(
        statements[1].kind(),
        TypedStatementKind::Local { initializer, .. }
            if matches!(initializer.kind(), TypedExpressionKind::ListCreate { capacity: Some(_) })
    ));
    assert!(matches!(
        statements[2].kind(),
        TypedStatementKind::Expression(expression)
            if matches!(expression.kind(), TypedExpressionKind::ListAdd { .. })
    ));
    assert!(matches!(
        statements[5].kind(),
        TypedStatementKind::ListSet { .. }
    ));
}

#[test]
fn rejects_invalid_list_arity_and_non_list_operands_without_fallback() {
    for source in [
        "namespace Example\n\
         public function collections()\n\
             List.create()\n\
         end\n",
        "namespace Example\n\
         public function collections()\n\
             List.withCapacity<<Int>>(1, 2)\n\
         end\n",
        "namespace Example\n\
         public function collections()\n\
             local values = List.create<<Int>>()\n\
             List.add(values, \"wrong\")\n\
         end\n",
        "namespace Example\n\
         public function collections()\n\
             List.length(1)\n\
         end\n",
        "namespace Example\n\
         public function collections(values: List<Int>)\n\
             values[\"wrong\"] = 1\n\
         end\n",
    ] {
        let fixture = check_function(source);
        assert!(fixture.result.body().is_none(), "{source}");
        assert!(
            !fixture.result.diagnostics().is_empty(),
            "missing diagnostic for {source}"
        );
    }
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
fn typed_table_indexing_returns_optional_and_assignment_inserts_or_replaces() {
    let fixture = check_function(
        "namespace Example\n\
         public function collections(): Int?\n\
             local scores: {[String]: Int} = { alice = 10 }\n\
             scores[\"alice\"] = 11\n\
             scores[\"bruno\"] = 12\n\
             return scores[\"bruno\"]\n\
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
fn typed_table_access_rejects_incompatible_or_unhashable_keys_and_values() {
    for source in [
        "namespace Example\n\
         public function collections(scores: {[String]: Int}): Int?\n\
             return scores[1]\n\
         end\n",
        "namespace Example\n\
         public function collections()\n\
             local scores: {[String]: Int} = {}\n\
             scores[\"alice\"] = \"wrong\"\n\
         end\n",
        "namespace Example\n\
         public function collections(key: {Int}): String?\n\
             local values: {[{Int}]: String} = {}\n\
             return values[key]\n\
         end\n",
    ] {
        let fixture = check_function(source);
        assert!(fixture.result.body().is_none(), "{source}");
        assert!(!fixture.result.diagnostics().is_empty(), "{source}");
    }
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
