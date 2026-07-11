use std::collections::BTreeMap;

use pop_foundation::{BubbleId, FileId, ModuleId, SymbolId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_file, parse_function_body, parse_function_signature};
use pop_types::{
    BodyChecker, CaptureMode, SemanticType, SignatureResolver, TypedExpressionKind,
    TypedStatementKind, embedded_bootstrap_schema,
};

struct CheckedFixture {
    result: pop_types::TypedBodyResult,
    arena: pop_types::TypeArena,
    symbols: BTreeMap<String, SymbolId>,
}

fn check_function(source_text: &str, checked_name: &str) -> CheckedFixture {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/body.pop", source_text).expect("source");
    let syntax = parse_file(&source);
    assert!(
        syntax.diagnostics().is_empty(),
        "structural syntax: {}",
        syntax.diagnostic_snapshot()
    );
    let parsed: Vec<_> = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::FunctionDeclaration)
        .map(|node| {
            let signature =
                parse_function_signature(&source, &syntax, node).expect("function signature");
            let body = parse_function_body(&source, &syntax, node, &signature).expect("body");
            (node, signature, body)
        })
        .collect();
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let mut symbols = BTreeMap::new();
    for (_, signature, _) in &parsed {
        let qualified = format!("Example.{}", signature.name());
        let symbol = indexed
            .index()
            .declaration_by_qualified_name(&qualified, SymbolSpace::Value)[0]
            .symbol();
        symbols.insert(signature.name().to_owned(), symbol);
    }
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let mut signatures = BTreeMap::new();
    for (_, syntax_signature, _) in &parsed {
        let symbol = symbols[syntax_signature.name()];
        let resolution_result = resolver.resolve(module, symbol, syntax_signature);
        assert!(
            resolution_result.diagnostics().is_empty(),
            "resolved signature"
        );
        signatures.insert(
            symbol,
            resolution_result
                .signature()
                .expect("valid signature")
                .clone(),
        );
    }
    let (_, _, body) = parsed
        .iter()
        .find(|(_, signature, _)| signature.name() == checked_name)
        .expect("checked function");
    let checked_symbol = symbols[checked_name];
    let result = BodyChecker::new(module, &mut resolver, &signatures)
        .check(&signatures[&checked_symbol], body);
    CheckedFixture {
        result,
        arena: resolver.into_arena(),
        symbols,
    }
}

#[test]
fn infers_locals_and_resolves_direct_calls_with_canonical_types() {
    let fixture = check_function(
        "namespace Example\n\
         private function combine(left: Int, right: Int): Int\n\
             return left + right\n\
         end\n\
         public function calculate(left: Int, right: Int): Int\n\
             local sum: Int = left + right\n\
             local copy = sum\n\
             return combine(copy, right)\n\
         end\n",
        "calculate",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let int = fixture.arena.source_type("Int").expect("Int");
    let TypedStatementKind::Local {
        local,
        local_type,
        initializer,
        ..
    } = body.statements()[0].kind()
    else {
        panic!("typed local");
    };
    assert_eq!(*local_type, int);
    assert_eq!(local.raw(), 0);
    assert_eq!(initializer.type_id(), int);
    assert!(matches!(
        fixture.arena.get(int),
        Some(SemanticType::Primitive(_))
    ));

    let TypedStatementKind::Return { values } = body.statements()[2].kind() else {
        panic!("typed return");
    };
    assert!(matches!(
        values[0].kind(),
        TypedExpressionKind::DirectCall { function, .. }
            if *function == fixture.symbols["combine"]
    ));
    assert_eq!(values[0].type_id(), int);
}

#[test]
fn reports_annotation_and_return_type_mismatches_without_dynamic_fallback() {
    for (source, expected_code) in [
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 local value: Boolean = 1\n\
                 return 1\n\
             end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
             public function invalid(): Boolean\n\
                 return 1\n\
             end\n",
            "POP2003",
        ),
    ] {
        let fixture = check_function(source, "invalid");
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
fn reports_unknown_values_wrong_call_arity_and_invalid_operands() {
    for (source, expected_code) in [
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 return missing\n\
             end\n",
            "POP1002",
        ),
        (
            "namespace Example\n\
             private function identity(value: Int): Int\n\
                 return value\n\
             end\n\
             public function invalid(): Int\n\
                 return identity(1, 2)\n\
             end\n",
            "POP2004",
        ),
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 return true + 1\n\
             end\n",
            "POP2005",
        ),
    ] {
        let fixture = check_function(source, "invalid");
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
fn types_structured_control_flow_and_proves_returns_on_both_branches() {
    let fixture = check_function(
        "namespace Example\n\
         public function choose(condition: Boolean): Int\n\
             while condition do\n\
                 condition\n\
             end\n\
             if condition then\n\
                 return 1\n\
             else\n\
                 return 2\n\
             end\n\
         end\n",
        "choose",
    );

    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    assert!(matches!(
        body.statements()[0].kind(),
        TypedStatementKind::While { body, .. } if body.len() == 1
    ));
    assert!(matches!(
        body.statements()[1].kind(),
        TypedStatementKind::If {
            then_body,
            else_body,
            ..
        } if then_body.len() == 1 && else_body.len() == 1
    ));
}

#[test]
fn rejects_non_boolean_conditions_and_missing_return_paths() {
    for (source, expected_code) in [
        (
            "namespace Example\n\
             public function invalid(): Int\n\
                 if 1 then\n\
                     return 1\n\
                 else\n\
                     return 2\n\
                 end\n\
             end\n",
            "POP2003",
        ),
        (
            "namespace Example\n\
             public function invalid(condition: Boolean): Int\n\
                 if condition then\n\
                     return 1\n\
                 end\n\
             end\n",
            "POP2006",
        ),
    ] {
        let fixture = check_function(source, "invalid");
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
fn branch_locals_do_not_escape_their_lexical_scope() {
    let fixture = check_function(
        "namespace Example\n\
         public function invalid(condition: Boolean): Int\n\
             if condition then\n\
                 local branchValue = 1\n\
             end\n\
             return branchValue\n\
         end\n",
        "invalid",
    );

    assert!(fixture.result.body().is_none());
    assert!(fixture.result.diagnostic_snapshot().starts_with("POP1002"));
}

#[test]
fn local_and_anonymous_closures_capture_lexical_values_with_static_function_types() {
    let fixture = check_function(
        "namespace Example\n\
         public function make(offset: Int): function(value: Int): Int\n\
             local function add(value: Int): Int\n\
                 return value + offset\n\
             end\n\
             local copy = function(value: Int): Int\n\
                 return add(value)\n\
             end\n\
             return copy\n\
         end\n",
        "make",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed closure body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("local function lowered to a typed closure binding");
    };
    let TypedExpressionKind::Closure(closure) = initializer.kind() else {
        panic!("closure expression");
    };
    assert_eq!(closure.captures().len(), 1);
    assert_eq!(closure.captures()[0].mode(), CaptureMode::Value);
    assert!(matches!(
        fixture.arena.get(initializer.type_id()),
        Some(SemanticType::Function { parameters, results })
            if parameters.len() == 1 && results.len() == 1
    ));

    let TypedStatementKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("anonymous closure local");
    };
    let TypedExpressionKind::Closure(closure) = initializer.kind() else {
        panic!("anonymous closure");
    };
    assert_eq!(closure.captures().len(), 1);
}

#[test]
fn captured_mutation_uses_one_typed_cell_and_shadowing_does_not_capture() {
    let fixture = check_function(
        "namespace Example\n\
         public function make(): function(delta: Int): Int\n\
             local total = 0\n\
             local function add(delta: Int): Int\n\
                 total = total + delta\n\
                 local shadow = total\n\
                 return shadow\n\
             end\n\
             return add\n\
         end\n",
        "make",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[1].kind() else {
        panic!("local function");
    };
    let TypedExpressionKind::Closure(closure) = initializer.kind() else {
        panic!("closure");
    };
    assert_eq!(closure.captures().len(), 1);
    assert_eq!(closure.captures()[0].mode(), CaptureMode::Cell);
    assert!(matches!(
        closure.body().statements()[0].kind(),
        TypedStatementKind::CaptureSet { .. }
    ));
}

#[test]
fn recursive_local_function_and_nested_capture_propagation_are_resolved_by_identity() {
    let fixture = check_function(
        "namespace Example\n\
         public function make(seed: Int): function(value: Int): Int\n\
             local function outer(value: Int): Int\n\
                 local function inner(next: Int): Int\n\
                     if next > 0 then\n\
                         return outer(next - 1) + seed\n\
                     else\n\
                         return seed\n\
                     end\n\
                 end\n\
                 return inner(value)\n\
             end\n\
             return outer\n\
         end\n",
        "make",
    );
    assert!(
        fixture.result.diagnostics().is_empty(),
        "{}",
        fixture.result.diagnostic_snapshot()
    );
    let body = fixture.result.body().expect("typed body");
    let TypedStatementKind::Local { initializer, .. } = body.statements()[0].kind() else {
        panic!("outer closure");
    };
    let TypedExpressionKind::Closure(outer) = initializer.kind() else {
        panic!("outer closure");
    };
    assert!(
        outer
            .captures()
            .iter()
            .any(|capture| capture.mode() == CaptureMode::Cell)
    );
    let TypedStatementKind::Local { initializer, .. } = outer.body().statements()[0].kind() else {
        panic!("inner closure");
    };
    let TypedExpressionKind::Closure(inner) = initializer.kind() else {
        panic!("inner closure");
    };
    assert!(
        inner.captures().len() >= 2,
        "recursive function and seed propagate"
    );
}

#[test]
fn local_assignment_never_changes_the_binding_type() {
    let fixture = check_function(
        "namespace Example\n\
         public function invalid(): Int\n\
             local value = 1\n\
             value = false\n\
             return value\n\
         end\n",
        "invalid",
    );
    assert!(fixture.result.body().is_none());
    assert!(fixture.result.diagnostic_snapshot().starts_with("POP2003"));
}

#[test]
fn fixed_width_literals_and_binary_context_keep_the_exact_declared_type() {
    for source in [
        "namespace Example\n\
         public function value(): Int8\n\
             return 127\n\
         end\n",
        "namespace Example\n\
         public function value(): Int8\n\
             return -128\n\
         end\n",
        "namespace Example\n\
         public function value(): UInt64\n\
             return 18446744073709551615\n\
         end\n",
        "namespace Example\n\
         public function add(value: Int8): Int8\n\
             return value + 1\n\
         end\n",
        "namespace Example\n\
         public function add(value: Int8): Int8\n\
             return 1 + value\n\
         end\n",
        "namespace Example\n\
         public function value(): Int8\n\
             local result: Int8 = 1 + 2\n\
             return result\n\
         end\n",
        "namespace Example\n\
         public function add(value: Float32): Float32\n\
             return value + 1\n\
         end\n",
    ] {
        let fixture = check_function(
            source,
            if source.contains("function add") {
                "add"
            } else {
                "value"
            },
        );
        assert!(
            fixture.result.diagnostics().is_empty(),
            "{}",
            fixture.result.diagnostic_snapshot()
        );
    }
}

#[test]
fn numeric_typing_rejects_out_of_range_and_mixed_or_unsupported_operations() {
    for (source, expected) in [
        (
            "namespace Example\n\
             public function value(): Int8\n\
                 return 128\n\
             end\n",
            "POP2013",
        ),
        (
            "namespace Example\n\
             public function value(): UInt8\n\
                 return -1\n\
             end\n",
            "POP2013",
        ),
        (
            "namespace Example\n\
             public function add(left: Int8, right: Int16): Int8\n\
                 return left + right\n\
             end\n",
            "POP2005",
        ),
        (
            "namespace Example\n\
             public function remainder(left: Float32, right: Float32): Float32\n\
                 return left % right\n\
             end\n",
            "POP2005",
        ),
    ] {
        let name = if source.contains("function add") {
            "add"
        } else if source.contains("function remainder") {
            "remainder"
        } else {
            "value"
        };
        let fixture = check_function(source, name);
        assert!(fixture.result.body().is_none());
        assert!(
            fixture.result.diagnostic_snapshot().starts_with(expected),
            "{}",
            fixture.result.diagnostic_snapshot()
        );
    }
}
