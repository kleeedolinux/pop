use pop_foundation::{SourceSpan, TextRange};
use pop_source::SourceFile;

use crate::signature::parse_type_tokens;
use crate::{NodeKind, SyntaxNode, SyntaxTree, Token, TokenKind, TypeSyntax, VisibilitySyntax};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeAliasDeclarationSyntax {
    visibility: VisibilitySyntax,
    name: String,
    target: TypeSyntax,
    span: SourceSpan,
}

impl TypeAliasDeclarationSyntax {
    #[must_use]
    pub const fn visibility(&self) -> VisibilitySyntax {
        self.visibility
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn target(&self) -> &TypeSyntax {
        &self.target
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TypeAliasDeclarationError {
    span: SourceSpan,
    expectation: &'static str,
}

impl TypeAliasDeclarationError {
    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses one non-generic namespace type alias.
///
/// # Errors
///
/// Returns an error for a missing name, `=`, target type, or trailing source.
pub fn parse_type_alias_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<TypeAliasDeclarationSyntax, TypeAliasDeclarationError> {
    if node.kind() != NodeKind::TypeAliasDeclaration {
        return Err(error(source, node.range(), "type alias declaration"));
    }
    let tokens: Vec<_> = syntax
        .tokens()
        .iter()
        .copied()
        .filter(|token| {
            !token.kind().is_trivia()
                && token.range().start() >= node.range().start()
                && token.range().end() <= node.range().end()
        })
        .collect();
    let mut position = 0_usize;
    let visibility = match tokens.get(position).map(|token| token.kind()) {
        Some(TokenKind::Public) => VisibilitySyntax::Public,
        Some(TokenKind::Private) => VisibilitySyntax::Private,
        _ => VisibilitySyntax::Internal,
    };
    if matches!(
        tokens.get(position).map(|token| token.kind()),
        Some(TokenKind::Public | TokenKind::Internal | TokenKind::Private)
    ) {
        position += 1;
    }
    expect(source, &tokens, &mut position, TokenKind::Type, "`type`")?;
    let name = expect(
        source,
        &tokens,
        &mut position,
        TokenKind::Identifier,
        "type alias name",
    )?
    .text(source)
    .to_owned();
    expect(source, &tokens, &mut position, TokenKind::Equal, "`=`")?;
    if position == tokens.len() {
        return Err(at_position(
            source,
            &tokens,
            position,
            node.range(),
            "target type",
        ));
    }
    let target = parse_type_tokens(source, node, tokens[position..].to_vec()).map_err(|error| {
        TypeAliasDeclarationError {
            span: error.span(),
            expectation: error.expectation(),
        }
    })?;
    Ok(TypeAliasDeclarationSyntax {
        visibility,
        name,
        target,
        span: SourceSpan::new(source.id(), node.range()),
    })
}

fn expect(
    source: &SourceFile,
    tokens: &[Token],
    position: &mut usize,
    kind: TokenKind,
    expectation: &'static str,
) -> Result<Token, TypeAliasDeclarationError> {
    let token = tokens.get(*position).copied();
    if token.is_some_and(|token| token.kind() == kind) {
        *position += 1;
        Ok(token.expect("checked token"))
    } else {
        Err(at_position(
            source,
            tokens,
            *position,
            TextRange::empty(source.len()),
            expectation,
        ))
    }
}

fn at_position(
    source: &SourceFile,
    tokens: &[Token],
    position: usize,
    fallback: TextRange,
    expectation: &'static str,
) -> TypeAliasDeclarationError {
    error(
        source,
        tokens.get(position).map_or(fallback, |token| token.range()),
        expectation,
    )
}

fn error(
    source: &SourceFile,
    range: TextRange,
    expectation: &'static str,
) -> TypeAliasDeclarationError {
    TypeAliasDeclarationError {
        span: SourceSpan::new(source.id(), range),
        expectation,
    }
}
