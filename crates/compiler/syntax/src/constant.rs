use pop_foundation::{SourceSpan, TextRange};
use pop_source::SourceFile;

use crate::body::parse_expression_prefix;
use crate::signature::parse_type_tokens;
use crate::{
    ExpressionSyntax, NodeKind, SyntaxNode, SyntaxTree, Token, TokenKind, TypeSyntax,
    VisibilitySyntax,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConstDeclarationSyntax {
    visibility: VisibilitySyntax,
    name: String,
    annotation: Option<TypeSyntax>,
    initializer: ExpressionSyntax,
    span: SourceSpan,
}

impl ConstDeclarationSyntax {
    #[must_use]
    pub const fn visibility(&self) -> VisibilitySyntax {
        self.visibility
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn annotation(&self) -> Option<&TypeSyntax> {
        self.annotation.as_ref()
    }

    #[must_use]
    pub const fn initializer(&self) -> &ExpressionSyntax {
        &self.initializer
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConstDeclarationError {
    span: SourceSpan,
    expectation: &'static str,
}

impl ConstDeclarationError {
    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses one namespace-scope constant with an optional explicit type.
///
/// # Errors
///
/// Returns an error for a missing name/initializer, an invalid type, or trailing
/// source after the initializer. Omitted visibility is `internal`.
pub fn parse_const_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<ConstDeclarationSyntax, ConstDeclarationError> {
    if node.kind() != NodeKind::ConstDeclaration {
        return Err(error(source, node.range(), "const declaration"));
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
    let has_explicit_visibility = matches!(
        tokens.get(position).map(|token| token.kind()),
        Some(TokenKind::Public | TokenKind::Internal | TokenKind::Private)
    );
    if has_explicit_visibility {
        position += 1;
    }
    expect(source, &tokens, &mut position, TokenKind::Const, "`const`")?;
    let name = expect(
        source,
        &tokens,
        &mut position,
        TokenKind::Identifier,
        "constant name",
    )?;
    let name = name.text(source).to_owned();

    let annotation = if tokens
        .get(position)
        .is_some_and(|token| token.kind() == TokenKind::Colon)
    {
        position += 1;
        let type_start = position;
        let equal = tokens[type_start..]
            .iter()
            .position(|token| token.kind() == TokenKind::Equal)
            .map(|offset| type_start + offset)
            .ok_or_else(|| at_position(source, &tokens, position, node.range(), "`=`"))?;
        let annotation = parse_type_tokens(source, node, tokens[type_start..equal].to_vec())
            .map_err(|error| ConstDeclarationError {
                span: error.span(),
                expectation: error.expectation(),
            })?;
        position = equal;
        Some(annotation)
    } else {
        None
    };
    expect(source, &tokens, &mut position, TokenKind::Equal, "`=`")?;
    let initializer =
        parse_expression_prefix(source, node, &tokens, &mut position).map_err(|error| {
            ConstDeclarationError {
                span: error.span(),
                expectation: error.expectation(),
            }
        })?;
    if position != tokens.len() {
        return Err(at_position(
            source,
            &tokens,
            position,
            node.range(),
            "end of constant declaration",
        ));
    }
    Ok(ConstDeclarationSyntax {
        visibility,
        name,
        annotation,
        initializer,
        span: SourceSpan::new(source.id(), node.range()),
    })
}

fn expect(
    source: &SourceFile,
    tokens: &[Token],
    position: &mut usize,
    kind: TokenKind,
    expectation: &'static str,
) -> Result<Token, ConstDeclarationError> {
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
) -> ConstDeclarationError {
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
) -> ConstDeclarationError {
    ConstDeclarationError {
        span: SourceSpan::new(source.id(), range),
        expectation,
    }
}
