use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{
    BinaryOperator, ExpressionSyntaxKind, NodeKind, TypeSyntaxKind, VisibilitySyntax,
    parse_const_declaration, parse_file,
};

fn parse(text: &str) -> pop_syntax::ConstDeclarationSyntax {
    let source = SourceFile::new(FileId::from_raw(0), "src/constants.pop", text).expect("source");
    let syntax = parse_file(&source);
    assert!(
        syntax.diagnostics().is_empty(),
        "{}",
        syntax.diagnostic_snapshot()
    );
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ConstDeclaration)
        .expect("const declaration");
    parse_const_declaration(&source, &syntax, node).expect("typed const syntax")
}

#[test]
fn parses_inferred_and_explicit_constant_initializers() {
    let inferred = parse("namespace Example\nprivate const ANSWER = 40 + 2\n");
    assert_eq!(inferred.visibility(), VisibilitySyntax::Private);
    assert_eq!(inferred.name(), "ANSWER");
    assert!(inferred.annotation().is_none());
    assert!(matches!(
        inferred.initializer().kind(),
        ExpressionSyntaxKind::Binary {
            operator: BinaryOperator::Add,
            ..
        }
    ));

    let explicit = parse("namespace Example\npublic const ANSWER: UInt8 = 42\n");
    assert_eq!(explicit.visibility(), VisibilitySyntax::Public);
    assert!(matches!(
        explicit.annotation().map(pop_syntax::TypeSyntax::kind),
        Some(TypeSyntaxKind::Named { path, arguments })
            if path == &["UInt8"] && arguments.is_empty()
    ));
}

#[test]
fn rejects_missing_initializer_and_trailing_source() {
    for source_text in [
        "namespace Example\nprivate const ANSWER\n",
        "namespace Example\nprivate const ANSWER = 42 unexpected\n",
    ] {
        let source =
            SourceFile::new(FileId::from_raw(0), "src/constants.pop", source_text).expect("source");
        let syntax = parse_file(&source);
        let node = syntax
            .root()
            .children()
            .iter()
            .find(|node| node.kind() == NodeKind::ConstDeclaration)
            .expect("const declaration");
        assert!(parse_const_declaration(&source, &syntax, node).is_err());
    }
}
