use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{
    ExpressionSyntaxKind, NodeKind, TypeSyntaxKind, parse_error_declaration, parse_file,
    parse_record_declaration, parse_union_declaration,
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
fn error_declarations_preserve_nominal_cases_and_generic_payloads() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/loadError.pop",
        "namespace Saves\n\
         public error LoadError<Source>\n\
             Io(error: Source)\n\
             InvalidData(message: String)\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::ErrorDeclaration)
        .expect("error declaration");
    let error = parse_error_declaration(&source, &syntax, node).expect("error syntax");

    assert_eq!(error.name(), "LoadError");
    assert_eq!(error.type_parameters()[0].name(), "Source");
    assert_eq!(
        error
            .cases()
            .iter()
            .map(pop_syntax::ErrorCaseSyntax::name)
            .collect::<Vec<_>>(),
        ["Io", "InvalidData"]
    );
    assert_eq!(error.cases()[0].payload()[0].name(), "error");
}

#[test]
fn records_and_unions_preserve_ordered_type_parameters() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/genericData.pop",
        "namespace GenericData\n\
         private record Pair<Left, Right>\n\
             left: Left\n\
             right: Right\n\
         end\n\
         private union Choice<Value>\n\
             Some(value: Value)\n\
             None\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");
    let record_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::RecordDeclaration)
        .expect("record");
    let union_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::UnionDeclaration)
        .expect("union");
    let record =
        parse_record_declaration(&source, &syntax, record_node).expect("record declaration");
    let union = parse_union_declaration(&source, &syntax, union_node).expect("union declaration");

    assert_eq!(
        record
            .type_parameters()
            .iter()
            .map(pop_syntax::GenericParameterSyntax::name)
            .collect::<Vec<_>>(),
        ["Left", "Right"]
    );
    assert_eq!(
        union
            .type_parameters()
            .iter()
            .map(pop_syntax::GenericParameterSyntax::name)
            .collect::<Vec<_>>(),
        ["Value"]
    );
}

#[test]
fn generic_data_parameters_preserve_nominal_bounds() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/boundedData.pop",
        "namespace BoundedData\n\
         private record Source<T, TValues: Iterable<T>>\n\
             values: TValues\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    assert!(syntax.diagnostics().is_empty(), "structural syntax");
    let record_node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::RecordDeclaration)
        .expect("record");
    let record =
        parse_record_declaration(&source, &syntax, record_node).expect("record declaration");

    assert!(record.type_parameters()[0].bound().is_none());
    assert!(matches!(
        record.type_parameters()[1]
            .bound()
            .map(pop_syntax::TypeSyntax::kind),
        Some(TypeSyntaxKind::Named { path, arguments })
            if path == &["Iterable"] && arguments.len() == 1
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
