use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{NodeKind, TokenKind, lex, parse_file};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId::from_raw(0), "src/main.pop", text).expect("small source")
}

#[test]
fn lexer_is_lossless_and_distinguishes_documentation_trivia() {
    let text = "namespace Demo\n\n--- <summary>Greets.</summary>\n-- ordinary\npublic function greet()\nend\n";
    let source = source(text);
    let result = lex(&source);

    assert!(result.diagnostics().is_empty());
    assert_eq!(result.reconstruct(&source), text);
    assert!(
        result
            .tokens()
            .iter()
            .any(|token| token.kind() == TokenKind::DocumentationComment)
    );
    assert!(
        result
            .tokens()
            .iter()
            .any(|token| token.kind() == TokenKind::LineComment)
    );
}

#[test]
fn lexer_preserves_unicode_identifiers_without_an_ascii_only_policy() {
    let text = "namespace Café\n";
    let source = source(text);
    let result = lex(&source);

    assert!(result.diagnostics().is_empty());
    assert_eq!(result.reconstruct(&source), text);
    assert!(
        result.tokens().iter().any(|token| {
            token.kind() == TokenKind::Identifier && token.text(&source) == "Café"
        })
    );
}

#[test]
fn parser_builds_header_and_declaration_nodes_without_losing_source() {
    let text = "namespace Demo\n\nusing Pop.Text\n\npublic function greet(name: String): String\n    return name\nend\n";
    let source = source(text);
    let tree = parse_file(&source);

    assert!(
        tree.diagnostics().is_empty(),
        "{}",
        tree.diagnostic_snapshot()
    );
    assert_eq!(tree.reconstruct(&source), text);
    assert_eq!(
        tree.root()
            .children()
            .iter()
            .map(pop_syntax::SyntaxNode::kind)
            .collect::<Vec<_>>(),
        [
            NodeKind::NamespaceDeclaration,
            NodeKind::UsingDirective,
            NodeKind::FunctionDeclaration
        ]
    );
}

#[test]
fn parser_diagnostics_have_a_deterministic_snapshot() {
    let text = "namespace Demo\nexport function greet()\nend\nfunction hidden()\nend\n";
    let source = source(text);
    let tree = parse_file(&source);

    assert_eq!(tree.reconstruct(&source), text);
    assert_eq!(tree.diagnostic_snapshot(), "POP0004@15..21\n");
}

#[test]
fn missing_namespace_recovers_at_the_start_of_the_file() {
    let source = source("public function greet()\nend\n");
    let tree = parse_file(&source);

    assert_eq!(tree.diagnostic_snapshot(), "POP0003@0..0\n");
    assert_eq!(
        tree.root().children()[0].kind(),
        NodeKind::FunctionDeclaration
    );
}
