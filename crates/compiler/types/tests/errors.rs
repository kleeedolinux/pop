#![allow(clippy::too_many_lines)]

use std::collections::BTreeMap;

use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_error_declaration, parse_file, parse_function_body, parse_function_signature,
};
use pop_types::{
    BodyChecker, SemanticType, SignatureResolver, TypedBodyResult, TypedExpressionKind,
    TypedStatementKind, embedded_bootstrap_schema,
};

fn check_function(text: &str, name: &str) -> TypedBodyResult {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/check.pop", text).expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let functions = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::FunctionDeclaration)
        .map(|node| {
            let syntax =
                parse_function_signature(&source, &syntax, node).expect("function signature");
            let symbol = database.index().declaration_by_qualified_name(
                &format!("Example.{}", syntax.name()),
                SymbolSpace::Value,
            )[0]
            .symbol();
            (symbol, node, syntax)
        })
        .collect::<Vec<_>>();
    let mut signatures = BTreeMap::new();
    for (symbol, _, syntax) in &functions {
        let result = resolver.resolve(module, *symbol, syntax);
        assert!(result.diagnostics().is_empty(), "signature diagnostics");
        signatures.insert(*symbol, result.signature().expect("signature").clone());
    }
    let (symbol, node, signature_syntax) = functions
        .iter()
        .find(|(_, _, signature)| signature.name() == name)
        .expect("target function");
    let body = parse_function_body(&source, &syntax, node, signature_syntax).expect("body");
    BodyChecker::new(module, &mut resolver, &signatures)
        .check(signatures.get(symbol).expect("resolved signature"), &body)
}

#[test]
fn nominal_error_declarations_have_distinct_error_and_case_identities() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/loadError.pop",
        "namespace Example\n\
         public error LoadError<Source>\n\
             Io(error: Source)\n\
             InvalidData(message: String)\n\
         end\n\
         public function load(): Result<String, LoadError<String>>\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ErrorDeclaration)
        .expect("error declaration");
    let parsed = parse_error_declaration(&source, &syntax, node).expect("error syntax");
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.LoadError", SymbolSpace::Type)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let result = resolver.define_error(module, symbol, &parsed);

    assert!(result.diagnostics().is_empty());
    let definition = result.definition().expect("error definition");
    assert_ne!(definition.cases()[0].case(), definition.cases()[1].case());
    assert!(matches!(
        resolver.arena().get(definition.type_id()),
        Some(SemanticType::ErrorUnion { definition: found, .. }) if *found == definition.error()
    ));

    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let function_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature syntax");
    let function = database
        .index()
        .declaration_by_qualified_name("Example.load", SymbolSpace::Value)[0]
        .symbol();
    let resolution = resolver.resolve(module, function, &function_syntax);
    let signature = resolution.signature().expect("resolved signature");
    let result_type = signature.results()[0].type_id().expect("Result type");
    let Some(SemanticType::Builtin { arguments, .. }) = resolver.arena().get(result_type) else {
        panic!("reserved Result");
    };
    assert!(matches!(
        resolver.arena().get(arguments[1]),
        Some(SemanticType::ErrorUnion { arguments, .. }) if arguments.len() == 1
    ));
}

#[test]
fn reserved_result_construction_and_try_use_exact_static_types() {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/result.pop",
        "namespace Example\n\
         public error LoadError<Source>\n\
             Missing\n\
             Source(error: Source)\n\
         end\n\
         private function transform(value: Int): Result<String, LoadError<String>>\n\
         end\n\
         public function propagate(result: Result<Int, LoadError<String>>): Result<String, LoadError<String>>\n\
             local value = try result\n\
             return transform(value)\n\
         end\n\
         public function ok(): Result<Int, String>\n\
             return Result.Ok(7)\n\
         end\n\
         public function failed(): Result<Int, String>\n\
             return Result.Error(\"failed\")\n\
         end\n\
         public function explicit(): Result<Int, String>\n\
             return Result.Ok<<Int, String>>(9)\n\
         end\n\
         public function missing(): LoadError<String>\n\
             return LoadError.Missing<<String>>()\n\
         end\n\
         public function describe(error: LoadError<String>): String\n\
             match error\n\
             when LoadError.Missing then\n\
                 return \"missing\"\n\
             when LoadError.Source(message) then\n\
                 return message\n\
             end\n\
         end\n\
         public function describeResult(result: Result<Int, String>): String\n\
             match result\n\
             when Result.Ok(value) then\n\
                 return String(value)\n\
             when Result.Error(error) then\n\
                 return error\n\
             end\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let indexed = build_declaration_index(&[ModuleInput::new(
        module,
        BubbleId::from_raw(0),
        &source,
        &syntax,
    )]);
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));

    let error_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ErrorDeclaration)
        .expect("error");
    let error_syntax = parse_error_declaration(&source, &syntax, error_node).expect("error syntax");
    let error_symbol = database
        .index()
        .declaration_by_qualified_name("Example.LoadError", SymbolSpace::Type)[0]
        .symbol();
    assert!(
        resolver
            .define_error(module, error_symbol, &error_syntax)
            .diagnostics()
            .is_empty()
    );

    let functions = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::FunctionDeclaration)
        .map(|node| {
            let signature =
                parse_function_signature(&source, &syntax, node).expect("function signature");
            let symbol = database.index().declaration_by_qualified_name(
                &format!("Example.{}", signature.name()),
                SymbolSpace::Value,
            )[0]
            .symbol();
            (symbol, node, signature)
        })
        .collect::<Vec<_>>();
    let mut signatures = BTreeMap::new();
    for (symbol, _, syntax) in &functions {
        let result = resolver.resolve(module, *symbol, syntax);
        assert!(result.diagnostics().is_empty(), "signature diagnostics");
        signatures.insert(*symbol, result.signature().expect("signature").clone());
    }

    for name in [
        "propagate",
        "ok",
        "failed",
        "explicit",
        "missing",
        "describe",
        "describeResult",
    ] {
        let (symbol, node, signature_syntax) = functions
            .iter()
            .find(|(_, _, signature)| signature.name() == name)
            .expect("function");
        let body = parse_function_body(&source, &syntax, node, signature_syntax).expect("body");
        let signature = signatures.get(symbol).expect("resolved signature");
        let checked = BodyChecker::new(module, &mut resolver, &signatures).check(signature, &body);
        assert!(
            checked.diagnostics().is_empty(),
            "{name}: {}",
            checked.diagnostic_snapshot()
        );
        let statements = checked.body().expect("typed body").statements();
        if name == "propagate" {
            let TypedStatementKind::Local { initializer, .. } = statements[0].kind() else {
                panic!("propagating local");
            };
            assert!(matches!(
                initializer.kind(),
                TypedExpressionKind::ResultPropagate { .. }
            ));
        } else if name == "describe" {
            assert!(matches!(
                statements[0].kind(),
                TypedStatementKind::ErrorMatch { .. }
            ));
        } else if name == "describeResult" {
            assert!(matches!(
                statements[0].kind(),
                TypedStatementKind::ResultMatch { .. }
            ));
        } else {
            let TypedStatementKind::Return { values } = statements[0].kind() else {
                panic!("result return");
            };
            assert!(matches!(
                values[0].kind(),
                TypedExpressionKind::ResultCase { .. } | TypedExpressionKind::ErrorCase { .. }
            ));
        }
    }
}

#[test]
fn result_propagation_rejects_non_results_and_mismatched_error_types() {
    for (operand, parameter) in [
        ("value", "value: Int"),
        ("result", "result: Result<Int, String>"),
    ] {
        let result = check_function(
            &format!(
                "namespace Example\n\
                 public function invalid({parameter}): Result<Int, Boolean>\n\
                     local value = try {operand}\n\
                     return Result.Ok(value)\n\
                 end\n"
            ),
            "invalid",
        );
        assert!(result.body().is_none());
        assert!(result.diagnostic_snapshot().starts_with("POP2024"));
    }
}

#[test]
fn result_case_without_complete_context_is_rejected_as_ambiguous() {
    let result = check_function(
        "namespace Example\n\
         public function invalid()\n\
             local result = Result.Ok(1)\n\
         end\n",
        "invalid",
    );

    assert!(result.body().is_none());
    assert!(result.diagnostic_snapshot().starts_with("POP2025"));
}

#[test]
fn cleanup_bodies_reject_control_transfer_and_nested_registration() {
    for statement in ["return", "defer\nend", "local value = try result"] {
        let result = check_function(
            &format!(
                "namespace Example\n\
                 public function invalid(result: Result<Int, String>)\n\
                     defer\n\
                         {statement}\n\
                     end\n\
                 end\n"
            ),
            "invalid",
        );
        assert!(result.body().is_none(), "{statement}");
        assert!(
            result.diagnostic_snapshot().starts_with("POP2026"),
            "{statement}: {}",
            result.diagnostic_snapshot()
        );
    }
}
