use std::collections::BTreeMap;

use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_file, parse_function_body, parse_function_signature, parse_union_declaration,
};
use pop_types::{BodyChecker, SignatureResolver, TypedStatementKind, embedded_bootstrap_schema};

fn check(text: &str) -> pop_types::TypedBodyResult {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/match.pop", text).expect("source");
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
    for node in syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::UnionDeclaration)
    {
        let union = parse_union_declaration(&source, &syntax, node).expect("union");
        let symbol = database
            .index()
            .declaration_by_qualified_name(&format!("Example.{}", union.name()), SymbolSpace::Type)
            [0]
        .symbol();
        let result = resolver.define_union(module, symbol, &union);
        assert!(result.diagnostics().is_empty());
    }
    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function");
    let signature_syntax =
        parse_function_signature(&source, &syntax, function_node).expect("signature syntax");
    let body = parse_function_body(&source, &syntax, function_node, &signature_syntax)
        .expect("body syntax");
    let function = database
        .index()
        .declaration_by_qualified_name("Example.consume", SymbolSpace::Value)[0]
        .symbol();
    let signature = resolver
        .resolve(module, function, &signature_syntax)
        .signature()
        .expect("signature")
        .clone();
    BodyChecker::new(
        module,
        &mut resolver,
        &BTreeMap::from([(function, signature.clone())]),
    )
    .check(&signature, &body)
}

const PREFIX: &str = "namespace Example\n\
    public union ResultValue\n\
        Ok(value: Int)\n\
        Error(message: String)\n\
    end\n";

#[test]
fn exhaustive_match_resolves_cases_and_arm_local_payload_bindings() {
    let result = check(&format!(
        "{PREFIX}\
         public function consume(result: ResultValue): Int\n\
             match result\n\
             when ResultValue.Ok(value) then\n\
                 return value\n\
             when ResultValue.Error(_) then\n\
                 return 0\n\
             end\n\
         end\n"
    ));
    assert!(
        result.diagnostics().is_empty(),
        "{}",
        result.diagnostic_snapshot()
    );
    let TypedStatementKind::Match { union, arms, .. } =
        result.body().expect("typed").statements()[0].kind()
    else {
        panic!("typed match");
    };
    assert_eq!(arms.len(), 2);
    assert_ne!(arms[0].case(), arms[1].case());
    assert_eq!(arms[0].bindings().len(), 1);
    assert!(arms[0].bindings()[0].local().is_some());
    assert!(arms[1].bindings()[0].local().is_none());
    assert_eq!(*union, arms[0].union());
}

#[test]
fn missing_case_is_an_error_with_a_safe_identity_based_insertion_fix() {
    let result = check(&format!(
        "{PREFIX}\
         public function consume(result: ResultValue): Int\n\
             match result\n\
             when ResultValue.Ok(value) then\n\
                 return value\n\
             end\n\
             return 0\n\
         end\n"
    ));
    assert!(result.body().is_none());
    assert!(result.diagnostic_snapshot().starts_with("POP2020"));
    let diagnostic = &result.diagnostics()[0];
    assert_eq!(diagnostic.fixes().len(), 1);
    assert!(diagnostic.fixes()[0].is_safe());
    assert!(
        diagnostic.fixes()[0].edit().edits()[0]
            .replacement()
            .contains("when ResultValue.Error(message) then")
    );
}

#[test]
fn duplicate_foreign_and_wrong_payload_cases_are_rejected() {
    for (extra_union, arms, code) in [
        (
            "",
            "when ResultValue.Ok(value) then\nreturn value\nwhen ResultValue.Ok(other) then\nreturn other\n",
            "POP2021",
        ),
        (
            "public union Other\nOnly\nend\n",
            "when ResultValue.Ok(value) then\nreturn value\nwhen Other.Only then\nreturn 0\n",
            "POP2022",
        ),
        (
            "",
            "when ResultValue.Ok then\nreturn 1\nwhen ResultValue.Error(message) then\nreturn 0\n",
            "POP2004",
        ),
    ] {
        let result = check(&format!(
            "{PREFIX}{extra_union}\
             public function consume(result: ResultValue): Int\n\
                 match result\n\
                 {arms}\
                 end\n\
             end\n"
        ));
        assert!(result.body().is_none());
        assert!(
            result.diagnostic_snapshot().contains(code),
            "{}",
            result.diagnostic_snapshot()
        );
    }
}
