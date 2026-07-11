use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{
    ExpressionSyntaxKind, NodeKind, TypeSyntaxKind, parse_attribute_declaration,
    parse_attribute_use, parse_file,
};

#[test]
fn attribute_declarations_have_typed_parameters_and_constant_defaults() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/attributes.pop",
        "namespace Metadata\n\
         public attribute Serializable(version: UInt32 = 1, name: String = \"default\")\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::AttributeDeclaration)
        .expect("attribute declaration");
    let declaration =
        parse_attribute_declaration(&source, &syntax, node).expect("attribute syntax");

    assert_eq!(declaration.name(), "Serializable");
    assert_eq!(declaration.parameters().len(), 2);
    assert_eq!(declaration.parameters()[0].name(), "version");
    assert!(matches!(
        declaration.parameters()[0].parameter_type().kind(),
        TypeSyntaxKind::Named { path, .. } if path.as_slice() == ["UInt32"]
    ));
    assert!(matches!(
        declaration.parameters()[0]
            .default_value()
            .expect("default")
            .kind(),
        ExpressionSyntaxKind::Integer(value) if value == "1"
    ));
}

#[test]
fn attribute_uses_preserve_source_order_and_named_or_positional_arguments() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/model.pop",
        "namespace Model\n\
         @Serializable(version = 2)\n\
         @FieldName(\"userName\")\n\
         private record User\n\
             name: String\n\
         end\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let uses: Vec<_> = syntax
        .root()
        .children()
        .iter()
        .filter(|node| node.kind() == NodeKind::AttributeUse)
        .map(|node| parse_attribute_use(&source, &syntax, node).expect("attribute use"))
        .collect();

    assert_eq!(uses.len(), 2);
    assert_eq!(uses[0].path(), &["Serializable"]);
    assert_eq!(uses[0].arguments()[0].name(), Some("version"));
    assert_eq!(uses[1].path(), &["FieldName"]);
    assert_eq!(uses[1].arguments()[0].name(), None);
    assert!(matches!(
        uses[1].arguments()[0].value().kind(),
        ExpressionSyntaxKind::String(value) if value == "\"userName\""
    ));
}

#[test]
fn malformed_attribute_arguments_are_rejected_without_textual_fallback() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/model.pop",
        "namespace Model\n@Serializable(version =)\nprivate record User\nend\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::AttributeUse)
        .expect("attribute use");
    let error = parse_attribute_use(&source, &syntax, node).expect_err("missing value");

    assert_eq!(error.expectation(), "expression");
}
