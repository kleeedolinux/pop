use pop_foundation::{FileId, SourceSpan, TextRange};
use pop_source::SourceFile;

use crate::body::parse_expression_prefix;
use crate::signature::parse_type_prefix;
use crate::{ExpressionSyntax, NodeKind, SyntaxNode, SyntaxTree, Token, TokenKind, TypeSyntax};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeDeclarationSyntax {
    name: String,
    parameters: Vec<AttributeParameterSyntax>,
    span: SourceSpan,
}

impl AttributeDeclarationSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parameters(&self) -> &[AttributeParameterSyntax] {
        &self.parameters
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeParameterSyntax {
    name: String,
    parameter_type: TypeSyntax,
    default_value: Option<ExpressionSyntax>,
    span: SourceSpan,
}

impl AttributeParameterSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn parameter_type(&self) -> &TypeSyntax {
        &self.parameter_type
    }

    #[must_use]
    pub const fn default_value(&self) -> Option<&ExpressionSyntax> {
        self.default_value.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeUseSyntax {
    path: Vec<String>,
    arguments: Vec<AttributeArgumentSyntax>,
    span: SourceSpan,
}

impl AttributeUseSyntax {
    #[must_use]
    pub fn path(&self) -> &[String] {
        &self.path
    }

    #[must_use]
    pub fn arguments(&self) -> &[AttributeArgumentSyntax] {
        &self.arguments
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeArgumentSyntax {
    name: Option<String>,
    value: ExpressionSyntax,
    span: SourceSpan,
}

impl AttributeArgumentSyntax {
    #[must_use]
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    #[must_use]
    pub const fn value(&self) -> &ExpressionSyntax {
        &self.value
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AttributeSyntaxError {
    span: SourceSpan,
    expectation: &'static str,
}

impl AttributeSyntaxError {
    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses a typed user-defined attribute declaration.
///
/// # Errors
///
/// Returns an error for a missing parameter name, type, delimiter, or default.
pub fn parse_attribute_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<AttributeDeclarationSyntax, AttributeSyntaxError> {
    let mut parser = AttributeParser::new(source, syntax, node);
    if node.kind() != NodeKind::AttributeDeclaration {
        return Err(parser.error("attribute declaration"));
    }
    parser.seek(TokenKind::Attribute)?;
    let name = parser.expect(TokenKind::Identifier, "attribute name")?;
    parser.expect(TokenKind::LeftParenthesis, "`(`")?;
    let mut parameters = Vec::new();
    while parser.current_kind() != Some(TokenKind::RightParenthesis) {
        let parameter = parser.expect(TokenKind::Identifier, "attribute parameter name")?;
        parser.expect(TokenKind::Colon, "`:`")?;
        let parameter_type = parser.parse_type()?;
        let default_value = if parser.consume(TokenKind::Equal).is_some() {
            Some(parser.parse_expression()?)
        } else {
            None
        };
        let end = default_value.as_ref().map_or_else(
            || parameter_type.span().range().end(),
            |value| value.span().range().end(),
        );
        parameters.push(AttributeParameterSyntax {
            name: parameter.text(source).to_owned(),
            parameter_type,
            default_value,
            span: SourceSpan::new(source.id(), ordered_range(parameter.range().start(), end)),
        });
        if parser.consume(TokenKind::Comma).is_none() {
            break;
        }
    }
    parser.expect(TokenKind::RightParenthesis, "`)`")?;
    Ok(AttributeDeclarationSyntax {
        name: name.text(source).to_owned(),
        parameters,
        span: SourceSpan::new(source.id(), node.range()),
    })
}

/// Parses one attribute attachment without assigning semantic target policy.
///
/// # Errors
///
/// Returns an error for malformed names, arguments, or delimiters.
pub fn parse_attribute_use(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<AttributeUseSyntax, AttributeSyntaxError> {
    let mut parser = AttributeParser::new(source, syntax, node);
    if node.kind() != NodeKind::AttributeUse {
        return Err(parser.error("attribute use"));
    }
    parser.parse_use()
}

pub(crate) fn parse_attribute_use_prefix(
    source: &SourceFile,
    node: &SyntaxNode,
    tokens: &[Token],
    position: &mut usize,
) -> Result<AttributeUseSyntax, AttributeSyntaxError> {
    let mut parser = AttributeParser {
        source,
        file: source.id(),
        node,
        tokens: tokens.to_vec(),
        position: *position,
    };
    let parsed = parser.parse_use()?;
    *position = parser.position;
    Ok(parsed)
}

impl AttributeParser<'_> {
    fn parse_use(&mut self) -> Result<AttributeUseSyntax, AttributeSyntaxError> {
        let start = self.expect(TokenKind::At, "`@`")?.range().start();
        let first = self.expect(TokenKind::Identifier, "attribute name")?;
        let mut path = vec![first.text(self.source).to_owned()];
        while self.consume(TokenKind::Dot).is_some() {
            let component = self.expect(TokenKind::Identifier, "qualified attribute name")?;
            path.push(component.text(self.source).to_owned());
        }
        let mut arguments = Vec::new();
        if self.consume(TokenKind::LeftParenthesis).is_some() {
            while self.current_kind() != Some(TokenKind::RightParenthesis) {
                let start = self.current_range().start();
                let name = if self.current_kind() == Some(TokenKind::Identifier)
                    && self.peek_kind() == Some(TokenKind::Equal)
                {
                    let name = self.expect(TokenKind::Identifier, "argument name")?;
                    self.expect(TokenKind::Equal, "`=`")?;
                    Some(name.text(self.source).to_owned())
                } else {
                    None
                };
                let value = self.parse_expression()?;
                arguments.push(AttributeArgumentSyntax {
                    name,
                    span: SourceSpan::new(
                        self.source.id(),
                        ordered_range(start, value.span().range().end()),
                    ),
                    value,
                });
                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RightParenthesis, "`)`")?;
        }
        let end = self
            .tokens
            .get(self.position.saturating_sub(1))
            .map_or(start, |token| token.range().end());
        let parsed = AttributeUseSyntax {
            path,
            arguments,
            span: SourceSpan::new(self.source.id(), ordered_range(start, end)),
        };
        if self.current_kind() == Some(TokenKind::Newline) {
            self.position += 1;
        }
        Ok(parsed)
    }
}

struct AttributeParser<'source> {
    source: &'source SourceFile,
    node: &'source SyntaxNode,
    file: FileId,
    tokens: Vec<Token>,
    position: usize,
}

impl<'source> AttributeParser<'source> {
    fn new(source: &'source SourceFile, syntax: &SyntaxTree, node: &'source SyntaxNode) -> Self {
        let tokens = syntax
            .tokens()
            .iter()
            .copied()
            .filter(|token| {
                !token.kind().is_trivia()
                    && token.range().start() >= node.range().start()
                    && token.range().end() <= node.range().end()
            })
            .collect();
        Self {
            source,
            node,
            file: source.id(),
            tokens,
            position: 0,
        }
    }

    fn parse_type(&mut self) -> Result<TypeSyntax, AttributeSyntaxError> {
        parse_type_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
            |error| AttributeSyntaxError {
                span: error.span(),
                expectation: error.expectation(),
            },
        )
    }

    fn parse_expression(&mut self) -> Result<ExpressionSyntax, AttributeSyntaxError> {
        parse_expression_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
            |error| AttributeSyntaxError {
                span: error.span(),
                expectation: error.expectation(),
            },
        )
    }

    fn seek(&mut self, kind: TokenKind) -> Result<(), AttributeSyntaxError> {
        while self.current_kind().is_some_and(|current| current != kind) {
            self.position += 1;
        }
        self.expect(kind, "declaration keyword").map(|_| ())
    }

    fn current_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position).map(|token| token.kind())
    }

    fn peek_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position + 1).map(|token| token.kind())
    }

    fn current_range(&self) -> TextRange {
        self.tokens.get(self.position).map_or_else(
            || TextRange::empty(self.node.range().end()),
            |token| token.range(),
        )
    }

    fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).copied()?;
        self.position += 1;
        Some(token)
    }

    fn consume(&mut self, kind: TokenKind) -> Option<Token> {
        (self.current_kind() == Some(kind))
            .then(|| self.advance())
            .flatten()
    }

    fn expect(
        &mut self,
        kind: TokenKind,
        expectation: &'static str,
    ) -> Result<Token, AttributeSyntaxError> {
        if self.current_kind() == Some(kind) {
            self.advance().ok_or_else(|| self.error(expectation))
        } else {
            Err(self.error(expectation))
        }
    }

    fn error(&self, expectation: &'static str) -> AttributeSyntaxError {
        AttributeSyntaxError {
            span: SourceSpan::new(self.file, self.current_range()),
            expectation,
        }
    }
}

fn ordered_range(start: pop_foundation::TextSize, end: pop_foundation::TextSize) -> TextRange {
    TextRange::new(start, end).unwrap_or_else(|| TextRange::empty(start))
}
