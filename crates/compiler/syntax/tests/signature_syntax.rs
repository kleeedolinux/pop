use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{NodeKind, TypeSyntaxKind, parse_file, parse_function_signature};

fn signature(text: &str) -> pop_syntax::FunctionSignatureSyntax {
    let source = SourceFile::new(FileId::from_raw(0), "src/signature.pop", text).expect("source");
    let syntax = parse_file(&source);
    assert!(
        syntax.diagnostics().is_empty(),
        "{}",
        syntax.diagnostic_snapshot()
    );
    let function = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::FunctionDeclaration)
        .expect("function node");
    parse_function_signature(&source, &syntax, function).expect("valid signature")
}

#[test]
fn generic_array_and_optional_signature_is_structured() {
    let signature = signature(
        "namespace Values\n\
         private function first<T>(values: {T}): T?\n\
             return nil\n\
         end\n",
    );

    assert_eq!(signature.name(), "first");
    assert_eq!(signature.type_parameters()[0].name(), "T");
    assert_eq!(signature.parameters()[0].name(), "values");
    assert!(matches!(
        signature.parameters()[0].parameter_type().kind(),
        TypeSyntaxKind::Array(element)
            if matches!(element.kind(), TypeSyntaxKind::Named { path, arguments }
                if path == &["T"] && arguments.is_empty())
    ));
    assert!(matches!(
        signature.results()[0].kind(),
        TypeSyntaxKind::Optional(inner)
            if matches!(inner.kind(), TypeSyntaxKind::Named { path, .. } if path == &["T"])
    ));
}

#[test]
fn generic_function_parameters_preserve_nominal_bounds() {
    let signature = signature(
        "namespace Values\n\
         private function consume<T, TSource: Iterable<T>>(source: TSource)\n\
         end\n",
    );

    assert!(signature.type_parameters()[0].bound().is_none());
    assert!(matches!(
        signature.type_parameters()[1]
            .bound()
            .map(pop_syntax::TypeSyntax::kind),
        Some(TypeSyntaxKind::Named { path, arguments })
            if path == &["Iterable"]
                && matches!(arguments.as_slice(), [argument]
                    if matches!(argument.kind(), TypeSyntaxKind::Named { path, arguments }
                        if path == &["T"] && arguments.is_empty()))
    ));
}

#[test]
fn qualified_generic_table_and_tuple_results_are_structured() {
    let signature = signature(
        "namespace Values\n\
         public function lookup(\n\
             values: Table<String, Game.Player>,\n\
             fallback: {[String]: Game.Player}\n\
         ): (Game.Player?, Boolean)\n\
             return nil, false\n\
         end\n",
    );

    assert_eq!(signature.parameters().len(), 2);
    assert!(matches!(
        signature.parameters()[0].parameter_type().kind(),
        TypeSyntaxKind::Named { path, arguments }
            if path == &["Table"] && arguments.len() == 2
    ));
    assert!(matches!(
        signature.parameters()[1].parameter_type().kind(),
        TypeSyntaxKind::Table { key, value }
            if matches!(key.kind(), TypeSyntaxKind::Named { path, .. } if path == &["String"])
                && matches!(value.kind(), TypeSyntaxKind::Named { path, .. }
                    if path == &["Game", "Player"])
    ));
    assert!(matches!(
        signature.results()[0].kind(),
        TypeSyntaxKind::Tuple(elements) if elements.len() == 2
    ));
}

#[test]
fn function_types_remain_fully_typed() {
    let signature = signature(
        "namespace Values\n\
         internal function select(\n\
             predicate: function(value: String): Boolean\n\
         ): String\n\
             return \"\"\n\
         end\n",
    );

    assert!(matches!(
        signature.parameters()[0].parameter_type().kind(),
        TypeSyntaxKind::Function { parameters, results }
            if parameters.len() == 1 && results.len() == 1
    ));
}

#[test]
fn malformed_parameter_type_is_rejected_without_recovery_as_unknown() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/broken.pop",
        "namespace Broken\npublic function run(value:)\nend\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let function = &syntax.root().children()[1];
    let error =
        parse_function_signature(&source, &syntax, function).expect_err("missing type must fail");

    assert_eq!(error.span().file(), FileId::from_raw(0));
    assert_eq!(error.expectation(), "type");
}
