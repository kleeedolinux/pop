use pop_foundation::{FileId, SourceSpan, TextRange};
use pop_source::SourceFile;

use crate::signature::{GenericParameterSyntax, parse_type_prefix};
use crate::{NodeKind, SyntaxNode, SyntaxTree, Token, TokenKind, TypeSyntax};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceDeclarationSyntax {
    name: String,
    type_parameters: Vec<GenericParameterSyntax>,
    methods: Vec<InterfaceMethodSyntax>,
    span: SourceSpan,
}

impl InterfaceDeclarationSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[GenericParameterSyntax] {
        &self.type_parameters
    }

    #[must_use]
    pub fn methods(&self) -> &[InterfaceMethodSyntax] {
        &self.methods
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceMethodSyntax {
    name: String,
    parameters: Vec<InterfaceMethodParameterSyntax>,
    results: Vec<TypeSyntax>,
    span: SourceSpan,
}

impl InterfaceMethodSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parameters(&self) -> &[InterfaceMethodParameterSyntax] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeSyntax] {
        &self.results
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceMethodParameterSyntax {
    name: String,
    parameter_type: TypeSyntax,
    span: SourceSpan,
}

impl InterfaceMethodParameterSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn parameter_type(&self) -> &TypeSyntax {
        &self.parameter_type
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterfaceDeclarationError {
    span: SourceSpan,
    expectation: &'static str,
}

impl InterfaceDeclarationError {
    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses one nominal interface containing public instance signatures only.
///
/// # Errors
///
/// Rejects fields, member visibility, method bodies, and malformed types. It
/// never assigns structural or runtime-name semantics to an invalid member.
pub fn parse_interface_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<InterfaceDeclarationSyntax, InterfaceDeclarationError> {
    let tokens = syntax
        .tokens()
        .iter()
        .copied()
        .filter(|token| {
            !matches!(
                token.kind(),
                TokenKind::Whitespace | TokenKind::LineComment | TokenKind::DocumentationComment
            ) && token.range().start() >= node.range().start()
                && token.range().end() <= node.range().end()
        })
        .collect();
    InterfaceParser {
        source,
        file: source.id(),
        node,
        tokens,
        position: 0,
    }
    .parse()
}

struct InterfaceParser<'source> {
    source: &'source SourceFile,
    file: FileId,
    node: &'source SyntaxNode,
    tokens: Vec<Token>,
    position: usize,
}

impl InterfaceParser<'_> {
    fn parse(mut self) -> Result<InterfaceDeclarationSyntax, InterfaceDeclarationError> {
        if self.node.kind() != NodeKind::InterfaceDeclaration {
            return Err(self.error("interface declaration"));
        }
        let start = self.current_span().range().start();
        if matches!(
            self.current_kind(),
            Some(TokenKind::Public | TokenKind::Internal | TokenKind::Private)
        ) {
            self.position += 1;
        }
        self.expect(TokenKind::Interface, "`interface`")?;
        let name = self.expect(TokenKind::Identifier, "interface name")?;
        let name = name.text(self.source).to_owned();
        let type_parameters = self.parse_type_parameters()?;
        self.expect(TokenKind::Newline, "line break after interface declaration")?;

        let mut methods = Vec::new();
        self.skip_newlines();
        while self.current_kind() != Some(TokenKind::End) {
            methods.push(self.parse_method()?);
            self.skip_newlines();
        }
        let end = self.expect(TokenKind::End, "`end`")?.range().end();
        if self.current_kind().is_some() {
            return Err(self.error("end of interface declaration"));
        }
        Ok(InterfaceDeclarationSyntax {
            name,
            type_parameters,
            methods,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_type_parameters(
        &mut self,
    ) -> Result<Vec<GenericParameterSyntax>, InterfaceDeclarationError> {
        if self.consume(TokenKind::LessThan).is_none() {
            return Ok(Vec::new());
        }
        let mut parameters = Vec::new();
        loop {
            let token = self.expect(TokenKind::Identifier, "type parameter")?;
            let bound = if self.consume(TokenKind::Colon).is_some() {
                Some(self.parse_type()?)
            } else {
                None
            };
            parameters.push(GenericParameterSyntax::new(
                token.text(self.source).to_owned(),
                bound,
                SourceSpan::new(self.file, token.range()),
            ));
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        self.expect(TokenKind::GreaterThan, "`>`")?;
        Ok(parameters)
    }

    fn parse_method(&mut self) -> Result<InterfaceMethodSyntax, InterfaceDeclarationError> {
        let start = self
            .expect(TokenKind::Function, "interface method signature")?
            .range()
            .start();
        let name = self.expect(TokenKind::Identifier, "interface method name")?;
        let method_name = name.text(self.source).to_owned();
        self.expect(TokenKind::LeftParenthesis, "`(`")?;
        let mut parameters = Vec::new();
        while self.current_kind() != Some(TokenKind::RightParenthesis) {
            let parameter = self.expect(TokenKind::Identifier, "parameter name")?;
            self.expect(TokenKind::Colon, "`:`")?;
            let parameter_type = self.parse_type()?;
            parameters.push(InterfaceMethodParameterSyntax {
                name: parameter.text(self.source).to_owned(),
                parameter_type,
                span: SourceSpan::new(self.file, parameter.range()),
            });
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
        let results = if self.consume(TokenKind::Colon).is_some() {
            vec![self.parse_type()?]
        } else {
            Vec::new()
        };
        let end = results
            .last()
            .map_or(right.range().end(), |result| result.span().range().end());
        self.expect(TokenKind::Newline, "line break after interface method")?;
        Ok(InterfaceMethodSyntax {
            name: method_name,
            parameters,
            results,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_type(&mut self) -> Result<TypeSyntax, InterfaceDeclarationError> {
        parse_type_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
            |error| InterfaceDeclarationError {
                span: error.span(),
                expectation: error.expectation(),
            },
        )
    }

    fn skip_newlines(&mut self) {
        while self.consume(TokenKind::Newline).is_some() {}
    }

    fn current_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position).map(|token| token.kind())
    }

    fn current_span(&self) -> SourceSpan {
        self.tokens.get(self.position).map_or_else(
            || SourceSpan::new(self.file, TextRange::empty(self.node.range().end())),
            |token| SourceSpan::new(self.file, token.range()),
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
    ) -> Result<Token, InterfaceDeclarationError> {
        self.consume(kind).ok_or_else(|| self.error(expectation))
    }

    fn error(&self, expectation: &'static str) -> InterfaceDeclarationError {
        InterfaceDeclarationError {
            span: self.current_span(),
            expectation,
        }
    }
}

fn ordered_range(start: pop_foundation::TextSize, end: pop_foundation::TextSize) -> TextRange {
    TextRange::new(start, end).unwrap_or_else(|| TextRange::empty(start))
}
