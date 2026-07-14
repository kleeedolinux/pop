use pop_foundation::FileId;
use pop_source::SourceFile;
use pop_syntax::{NodeKind, TokenKind, lex, parse_file};

fn source(text: &str) -> SourceFile {
    SourceFile::new(FileId::from_raw(0), "src/main.pop", text).expect("small source")
}

#[test]
fn lexer_is_lossless_and_distinguishes_documentation_trivia() {
    let text = "namespace Demo\n\n--- <summary>\n--- Greets.\n--- </summary>\n-- ordinary\npublic function greet()\nend\n";
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
fn lexer_reserves_typed_error_workflow_words() {
    let source = source("error try defer\n");
    let result = lex(&source);

    assert!(result.diagnostics().is_empty());
    assert_eq!(result.reconstruct(&source), source.text());
    let significant = result
        .tokens()
        .iter()
        .filter(|token| !token.kind().is_trivia())
        .map(|token| token.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        significant,
        [TokenKind::Identifier, TokenKind::Try, TokenKind::Defer]
    );
}

#[test]
fn lexer_reserves_typed_async_workflow_words() {
    let source = source("async await\n");
    let result = lex(&source);

    assert!(result.diagnostics().is_empty());
    assert_eq!(result.reconstruct(&source), source.text());
    let significant = result
        .tokens()
        .iter()
        .filter(|token| !token.kind().is_trivia())
        .map(|token| token.kind())
        .collect::<Vec<_>>();
    assert_eq!(significant, [TokenKind::Async, TokenKind::Await]);
}

#[test]
fn async_function_is_one_namespace_declaration() {
    let source = source(
        "namespace Example\n\
         public async function load(): String\n\
             return await fetch()\n\
         end\n",
    );
    let syntax = parse_file(&source);

    assert!(
        syntax.diagnostics().is_empty(),
        "{}",
        syntax.diagnostic_snapshot()
    );
    assert_eq!(syntax.root().children().len(), 2);
    assert_eq!(
        syntax.root().children()[1].kind(),
        NodeKind::FunctionDeclaration
    );
}

#[test]
fn async_cannot_modify_a_non_function_declaration() {
    let source = source(
        "namespace Example\n\
         public async const LIMIT: Int = 1\n",
    );
    let syntax = parse_file(&source);

    assert_eq!(syntax.root().children()[1].kind(), NodeKind::Error);
    assert!(!syntax.diagnostics().is_empty());
}

#[test]
fn lexer_keeps_decimal_exponents_complete_and_member_dots_separate() {
    // ADR 0040: decimal fractions and exponents are one lossless number token,
    // while a dot without a following digit remains member punctuation.
    let source = source("1.5 6.02e23 1_000.25 2e-3 1.value <= 2 >= 1\n");
    let result = lex(&source);

    assert!(result.diagnostics().is_empty());
    assert_eq!(result.reconstruct(&source), source.text());
    let significant = result
        .tokens()
        .iter()
        .filter(|token| !token.kind().is_trivia())
        .map(|token| (token.kind(), token.text(&source)))
        .collect::<Vec<_>>();
    assert_eq!(
        significant,
        [
            (TokenKind::Number, "1.5"),
            (TokenKind::Number, "6.02e23"),
            (TokenKind::Number, "1_000.25"),
            (TokenKind::Number, "2e-3"),
            (TokenKind::Number, "1"),
            (TokenKind::Dot, "."),
            (TokenKind::Identifier, "value"),
            (TokenKind::LessThanEqual, "<="),
            (TokenKind::Number, "2"),
            (TokenKind::GreaterThanEqual, ">="),
            (TokenKind::Number, "1"),
        ]
    );
}

#[test]
fn lexer_preserves_typed_string_composition_tokens_losslessly() {
    // ADR 0041: quoted escapes, Luau concatenation, and backtick
    // interpolation are distinct lossless source forms.
    let text = r#""line\n\u{1F9FC}" .. `checked {count + 1} \{literal\}`"#;
    let source = source(text);
    let result = lex(&source);

    assert!(result.diagnostics().is_empty());
    assert_eq!(result.reconstruct(&source), text);
    let significant = result
        .tokens()
        .iter()
        .filter(|token| !token.kind().is_trivia())
        .map(|token| token.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        significant,
        [
            TokenKind::String,
            TokenKind::DotDot,
            TokenKind::InterpolatedString,
        ]
    );
}

#[test]
fn lexer_distinguishes_optional_default_and_propagation_tokens() {
    // ADR 0051: `??` is one defaulting token while postfix `?` remains a
    // distinct propagation token (and the existing optional-type suffix).
    let source = source("primary ?? fallback?\n");
    let result = lex(&source);

    assert!(result.diagnostics().is_empty());
    assert_eq!(result.reconstruct(&source), source.text());
    let significant = result
        .tokens()
        .iter()
        .filter(|token| !token.kind().is_trivia())
        .map(|token| token.kind())
        .collect::<Vec<_>>();
    assert_eq!(
        significant,
        [
            TokenKind::Identifier,
            TokenKind::QuestionQuestion,
            TokenKind::Identifier,
            TokenKind::Question,
        ]
    );
}

#[test]
fn lexer_rejects_invalid_or_non_scalar_string_escapes() {
    for text in [r#""bad\q""#, r#""bad\x0""#, r#""bad\u{D800}""#] {
        let result = lex(&source(text));
        assert_eq!(result.diagnostics().len(), 1, "{text}");
        assert_eq!(result.diagnostics()[0].code().as_str(), "POP0007");
    }
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
