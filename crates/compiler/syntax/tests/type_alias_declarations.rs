use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{NodeKind, TypeSyntaxKind, parse_file, parse_type_alias_declaration};

#[test]
fn parses_visible_erased_type_aliases() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/aliases.pop",
        "namespace Example\npublic type Scores = {[String]: Int}\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::TypeAliasDeclaration)
        .expect("type alias node");
    let alias = parse_type_alias_declaration(&source, &syntax, node).expect("type alias");

    assert_eq!(alias.name(), "Scores");
    assert!(matches!(
        alias.target().kind(),
        TypeSyntaxKind::Table { .. }
    ));
}
