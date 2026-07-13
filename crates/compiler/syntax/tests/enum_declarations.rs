use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{NodeKind, parse_enum_declaration, parse_file};

#[test]
fn parses_nominal_payload_free_enum_cases() {
    let source = SourceFile::new(
        FileId::from_raw(0),
        "src/colors.pop",
        "namespace Example\npublic enum Color\n    Red\n    Green\n    Blue\nend\n",
    )
    .expect("source");
    let syntax = parse_file(&source);
    let node = syntax
        .root()
        .children()
        .iter()
        .find(|node| node.kind() == NodeKind::EnumDeclaration)
        .expect("enum node");
    let declaration = parse_enum_declaration(&source, &syntax, node).expect("enum");

    assert_eq!(declaration.name(), "Color");
    assert_eq!(
        declaration
            .cases()
            .iter()
            .map(pop_syntax::EnumCaseSyntax::name)
            .collect::<Vec<_>>(),
        ["Red", "Green", "Blue"]
    );
}
