use std::collections::BTreeMap;

use pop_foundation::{BubbleId, FileId, ModuleId};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_source::SourceFile;
use pop_syntax::{
    NodeKind, parse_file, parse_function_body, parse_function_signature, parse_union_declaration,
};
use pop_types::{
    BodyChecker, SignatureResolver, TypedExpressionKind, TypedStatementKind,
    embedded_bootstrap_schema,
};

fn check_union_function(text: &str, function_name: &str) -> pop_types::TypedBodyResult {
    let module = ModuleId::from_raw(0);
    let source = SourceFile::new(FileId::from_raw(0), "src/state.pop", text).expect("source");
    let syntax = parse_file(&source);
    let union_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::UnionDeclaration)
        .expect("union");
    let function_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| {
            node.kind() == NodeKind::FunctionDeclaration
                && parse_function_signature(&source, &syntax, node)
                    .is_ok_and(|signature| signature.name() == function_name)
        })
        .expect("function");
    let union_syntax = parse_union_declaration(&source, &syntax, union_node).expect("union syntax");
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
    let union_symbol = indexed
        .index()
        .declaration_by_qualified_name("Example.LoadState", SymbolSpace::Type)[0]
        .symbol();
    let function_symbol = indexed
        .index()
        .declaration_by_qualified_name(&format!("Example.{function_name}"), SymbolSpace::Value)[0]
        .symbol();
    let database = ResolutionDatabase::new(indexed.into_index());
    let mut resolver =
        SignatureResolver::new(&database, embedded_bootstrap_schema().expect("bootstrap"));
    let definition = resolver.define_union(module, union_symbol, &union_syntax);
    assert!(definition.diagnostics().is_empty());
    let signature = resolver
        .resolve(module, function_symbol, &function_syntax)
        .signature()
        .expect("signature")
        .clone();
    let signatures = BTreeMap::from([(function_symbol, signature.clone())]);
    BodyChecker::new(module, &mut resolver, &signatures).check(&signature, &body)
}

#[test]
fn union_cases_construct_one_known_static_union_type() {
    let source = "namespace Example\n\
         public union LoadState\n\
             Idle\n\
             Loading(progress: Int)\n\
             Failed(error: String)\n\
         end\n\
         public function idle(): LoadState\n\
             return LoadState.Idle\n\
         end\n\
         public function loading(progress: Int): LoadState\n\
             return LoadState.Loading(progress)\n\
         end\n";
    for function in ["idle", "loading"] {
        let result = check_union_function(source, function);
        assert!(
            result.diagnostics().is_empty(),
            "{}",
            result.diagnostic_snapshot()
        );
        let TypedStatementKind::Return { values } =
            result.body().expect("typed").statements()[0].kind()
        else {
            panic!("return");
        };
        assert!(matches!(
            values[0].kind(),
            TypedExpressionKind::UnionCase { arguments, .. }
                if arguments.len() == usize::from(function == "loading")
        ));
    }
}

#[test]
fn union_case_payloads_reject_wrong_arity_and_types() {
    for (expression, expected) in [
        ("LoadState.Loading()", "POP2004"),
        ("LoadState.Loading(\"wrong\")", "POP2003"),
        ("LoadState.Missing", "POP1002"),
    ] {
        let source = format!(
            "namespace Example\n\
             public union LoadState\n\
                 Idle\n\
                 Loading(progress: Int)\n\
             end\n\
             public function invalid(): LoadState\n\
                 return {expression}\n\
             end\n"
        );
        let result = check_union_function(&source, "invalid");
        assert!(result.body().is_none());
        assert!(result.diagnostic_snapshot().starts_with(expected));
    }
}
