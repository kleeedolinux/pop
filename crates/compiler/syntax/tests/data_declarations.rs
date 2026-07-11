use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{
    ExpressionSyntaxKind, NodeKind, TypeSyntaxKind, parse_file, parse_record_declaration,
    parse_union_declaration,
};

#[test]
fn record_fields_have_typed_names_and_optional_defaults() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/player.pop",
        "namespace Game\n\
         public record Player\n\
             name: String\n\
             score: Int = 0\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::RecordDeclaration)
        .expect("record");
    let record = parse_record_declaration(&source, &syntax, node).expect("record declaration");

    assert_eq!(record.name(), "Player");
    assert_eq!(record.fields().len(), 2);
    assert_eq!(record.fields()[0].name(), "name");
    assert!(matches!(
        record.fields()[0].field_type().kind(),
        TypeSyntaxKind::Named { path, .. } if path.as_slice() == ["String"]
    ));
    assert!(record.fields()[0].default_value().is_none());
    assert!(matches!(
        record.fields()[1]
            .default_value()
            .expect("score default")
            .kind(),
        ExpressionSyntaxKind::Integer(value) if value == "0"
    ));
}

#[test]
fn tagged_union_cases_preserve_typed_payloads() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/state.pop",
        "namespace Loading\n\
         public union LoadState\n\
             Idle\n\
             Loading(progress: Float)\n\
             Ready(data: Bytes)\n\
             Failed(error: String)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::UnionDeclaration)
        .expect("union");
    let union = parse_union_declaration(&source, &syntax, node).expect("union declaration");

    assert_eq!(union.name(), "LoadState");
    assert_eq!(
        union
            .cases()
            .iter()
            .map(pop_syntax::UnionCaseSyntax::name)
            .collect::<Vec<_>>(),
        ["Idle", "Loading", "Ready", "Failed"]
    );
    assert!(union.cases()[0].payload().is_empty());
    assert_eq!(union.cases()[1].payload()[0].name(), "progress");
    assert!(matches!(
        union.cases()[1].payload()[0].parameter_type().kind(),
        TypeSyntaxKind::Named { path, .. } if path.as_slice() == ["Float"]
    ));
}

#[test]
fn malformed_record_fields_are_rejected_without_untyped_recovery() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/broken.pop",
        "namespace Broken\n\
         public record BrokenRecord\n\
             value Int\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::RecordDeclaration)
        .expect("record");
    let error = parse_record_declaration(&source, &syntax, node).expect_err("missing colon");

    assert_eq!(error.expectation(), "`:`");
}
