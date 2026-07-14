use pop_foundation::{FileId, SourceSpan, TextRange};
use pop_source::SourceFile;

use crate::{NodeKind, SyntaxNode, SyntaxTree, Token, TokenKind};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionSignatureSyntax {
    name: String,
    type_parameters: Vec<GenericParameterSyntax>,
    parameters: Vec<FunctionParameterSyntax>,
    results: Vec<TypeSyntax>,
    range: TextRange,
}

impl FunctionSignatureSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[GenericParameterSyntax] {
        &self.type_parameters
    }

    #[must_use]
    pub fn parameters(&self) -> &[FunctionParameterSyntax] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeSyntax] {
        &self.results
    }

    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GenericParameterSyntax {
    name: String,
    bound: Option<TypeSyntax>,
    span: SourceSpan,
}

impl GenericParameterSyntax {
    pub(crate) fn new(name: String, bound: Option<TypeSyntax>, span: SourceSpan) -> Self {
        Self { name, bound, span }
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn bound(&self) -> Option<&TypeSyntax> {
        self.bound.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionParameterSyntax {
    name: String,
    parameter_type: TypeSyntax,
    span: SourceSpan,
}

impl FunctionParameterSyntax {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeSyntax {
    kind: TypeSyntaxKind,
    span: SourceSpan,
}

impl TypeSyntax {
    #[must_use]
    pub const fn kind(&self) -> &TypeSyntaxKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypeSyntaxKind {
    Named {
        path: Vec<String>,
        arguments: Vec<TypeSyntax>,
    },
    Optional(Box<TypeSyntax>),
    Union(Vec<TypeSyntax>),
    Tuple(Vec<TypeSyntax>),
    Array(Box<TypeSyntax>),
    Table {
        key: Box<TypeSyntax>,
        value: Box<TypeSyntax>,
    },
    Function {
        parameters: Vec<TypeSyntax>,
        results: Vec<TypeSyntax>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionSignatureError {
    span: SourceSpan,
    expectation: &'static str,
}

impl FunctionSignatureError {
    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses the typed signature owned by one function declaration node.
///
/// # Errors
///
/// Returns [`FunctionSignatureError`] when a required name, delimiter, or type
/// is absent. It never manufactures an unknown type for recovery.
pub fn parse_function_signature(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<FunctionSignatureSyntax, FunctionSignatureError> {
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
    SignatureParser {
        source,
        file: source.id(),
        node,
        tokens,
        position: 0,
    }
    .parse()
}

pub(crate) fn parse_type_tokens(
    source: &SourceFile,
    node: &SyntaxNode,
    tokens: Vec<Token>,
) -> Result<TypeSyntax, FunctionSignatureError> {
    let mut parser = SignatureParser {
        source,
        file: source.id(),
        node,
        tokens,
        position: 0,
    };
    let parsed = parser.parse_type()?;
    if parser.position != parser.tokens.len() {
        return Err(parser.error("end of type"));
    }
    Ok(parsed)
}

pub(crate) fn parse_type_prefix(
    source: &SourceFile,
    node: &SyntaxNode,
    tokens: &[Token],
    position: &mut usize,
) -> Result<TypeSyntax, FunctionSignatureError> {
    let mut parser = SignatureParser {
        source,
        file: source.id(),
        node,
        tokens: tokens.to_vec(),
        position: *position,
    };
    let parsed = parser.parse_type()?;
    *position = parser.position;
    Ok(parsed)
}

struct SignatureParser<'source> {
    source: &'source SourceFile,
    file: FileId,
    node: &'source SyntaxNode,
    tokens: Vec<Token>,
    position: usize,
}

impl SignatureParser<'_> {
    fn parse(mut self) -> Result<FunctionSignatureSyntax, FunctionSignatureError> {
        if self.node.kind() != NodeKind::FunctionDeclaration {
            return Err(self.error("function declaration"));
        }
        while self
            .current_kind()
            .is_some_and(|kind| kind != TokenKind::Function)
        {
            self.position += 1;
        }
        let start = self
            .expect(TokenKind::Function, "`function`")?
            .range()
            .start();
        let name_token = self.expect(TokenKind::Identifier, "function name")?;
        let name = name_token.text(self.source).to_owned();
        let type_parameters = self.parse_type_parameters()?;
        self.expect(TokenKind::LeftParenthesis, "`(`")?;
        let parameters = self.parse_parameters()?;
        let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
        let results = if self.consume(TokenKind::Colon).is_some() {
            vec![self.parse_type()?]
        } else {
            Vec::new()
        };
        let end = results
            .last()
            .map_or(right.range().end(), |result| result.span().range().end());
        Ok(FunctionSignatureSyntax {
            name,
            type_parameters,
            parameters,
            results,
            range: ordered_range(start, end),
        })
    }

    fn parse_type_parameters(
        &mut self,
    ) -> Result<Vec<GenericParameterSyntax>, FunctionSignatureError> {
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

    fn parse_parameters(&mut self) -> Result<Vec<FunctionParameterSyntax>, FunctionSignatureError> {
        let mut parameters = Vec::new();
        while self.current_kind() != Some(TokenKind::RightParenthesis) {
            let name = self.expect(TokenKind::Identifier, "parameter name")?;
            self.expect(TokenKind::Colon, "`:`")?;
            let parameter_type = self.parse_type()?;
            parameters.push(FunctionParameterSyntax {
                name: name.text(self.source).to_owned(),
                parameter_type,
                span: SourceSpan::new(self.file, name.range()),
            });
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        Ok(parameters)
    }

    fn parse_type(&mut self) -> Result<TypeSyntax, FunctionSignatureError> {
        let first = self.parse_postfix_type()?;
        if self.consume(TokenKind::Pipe).is_none() {
            return Ok(first);
        }
        let start = first.span().range().start();
        let mut members = vec![first];
        loop {
            members.push(self.parse_postfix_type()?);
            if self.consume(TokenKind::Pipe).is_none() {
                break;
            }
        }
        let end = members
            .last()
            .map_or(start, |member| member.span().range().end());
        Ok(self.type_node(TypeSyntaxKind::Union(members), start, end))
    }

    fn parse_postfix_type(&mut self) -> Result<TypeSyntax, FunctionSignatureError> {
        let mut parsed = self.parse_primary_type()?;
        while let Some(question) = self.consume(TokenKind::Question) {
            let start = parsed.span().range().start();
            parsed = self.type_node(
                TypeSyntaxKind::Optional(Box::new(parsed)),
                start,
                question.range().end(),
            );
        }
        Ok(parsed)
    }

    fn parse_primary_type(&mut self) -> Result<TypeSyntax, FunctionSignatureError> {
        match self.current_kind() {
            Some(TokenKind::Identifier | TokenKind::Nil) => self.parse_named_type(),
            Some(TokenKind::LeftBrace) => self.parse_collection_type(),
            Some(TokenKind::LeftParenthesis) => self.parse_tuple_or_grouped_type(),
            Some(TokenKind::Function) => self.parse_function_type(),
            _ => Err(self.error("type")),
        }
    }

    fn parse_named_type(&mut self) -> Result<TypeSyntax, FunctionSignatureError> {
        let first = self.advance().ok_or_else(|| self.error("type name"))?;
        let start = first.range().start();
        let mut end = first.range().end();
        let mut path = vec![first.text(self.source).to_owned()];
        while self.consume(TokenKind::Dot).is_some() {
            let component = self.expect(TokenKind::Identifier, "qualified type name")?;
            end = component.range().end();
            path.push(component.text(self.source).to_owned());
        }
        let mut arguments = Vec::new();
        if self.consume(TokenKind::LessThan).is_some() {
            loop {
                arguments.push(self.parse_type()?);
                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
            end = self.expect(TokenKind::GreaterThan, "`>`")?.range().end();
        }
        Ok(self.type_node(TypeSyntaxKind::Named { path, arguments }, start, end))
    }

    fn parse_collection_type(&mut self) -> Result<TypeSyntax, FunctionSignatureError> {
        let left = self.expect(TokenKind::LeftBrace, "`{`")?;
        let start = left.range().start();
        if self.consume(TokenKind::LeftBracket).is_some() {
            let key = self.parse_type()?;
            self.expect(TokenKind::RightBracket, "`]`")?;
            self.expect(TokenKind::Colon, "`:`")?;
            let value = self.parse_type()?;
            let right = self.expect(TokenKind::RightBrace, "`}`")?;
            return Ok(self.type_node(
                TypeSyntaxKind::Table {
                    key: Box::new(key),
                    value: Box::new(value),
                },
                start,
                right.range().end(),
            ));
        }
        let element = self.parse_type()?;
        let right = self.expect(TokenKind::RightBrace, "`}`")?;
        Ok(self.type_node(
            TypeSyntaxKind::Array(Box::new(element)),
            start,
            right.range().end(),
        ))
    }

    fn parse_tuple_or_grouped_type(&mut self) -> Result<TypeSyntax, FunctionSignatureError> {
        let left = self.expect(TokenKind::LeftParenthesis, "`(`")?;
        let start = left.range().start();
        if let Some(right) = self.consume(TokenKind::RightParenthesis) {
            return Ok(self.type_node(
                TypeSyntaxKind::Tuple(Vec::new()),
                start,
                right.range().end(),
            ));
        }
        let first = self.parse_type()?;
        if self.consume(TokenKind::Comma).is_none() {
            self.expect(TokenKind::RightParenthesis, "`)`")?;
            return Ok(first);
        }
        let mut elements = vec![first];
        loop {
            elements.push(self.parse_type()?);
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
        Ok(self.type_node(TypeSyntaxKind::Tuple(elements), start, right.range().end()))
    }

    fn parse_function_type(&mut self) -> Result<TypeSyntax, FunctionSignatureError> {
        let function = self.expect(TokenKind::Function, "`function`")?;
        let start = function.range().start();
        self.expect(TokenKind::LeftParenthesis, "`(`")?;
        let mut parameters = Vec::new();
        while self.current_kind() != Some(TokenKind::RightParenthesis) {
            self.expect(TokenKind::Identifier, "function-type parameter")?;
            self.expect(TokenKind::Colon, "`:`")?;
            parameters.push(self.parse_type()?);
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
        let mut end = right.range().end();
        let results = if self.consume(TokenKind::Colon).is_some() {
            let result = self.parse_type()?;
            end = result.span().range().end();
            vec![result]
        } else {
            Vec::new()
        };
        Ok(self.type_node(
            TypeSyntaxKind::Function {
                parameters,
                results,
            },
            start,
            end,
        ))
    }

    fn type_node(
        &self,
        kind: TypeSyntaxKind,
        start: pop_foundation::TextSize,
        end: pop_foundation::TextSize,
    ) -> TypeSyntax {
        TypeSyntax {
            kind,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        }
    }

    fn current_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position).map(|token| token.kind())
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
    ) -> Result<Token, FunctionSignatureError> {
        if self.current_kind() == Some(kind) {
            self.advance().ok_or_else(|| self.error(expectation))
        } else {
            Err(self.error(expectation))
        }
    }

    fn error(&self, expectation: &'static str) -> FunctionSignatureError {
        let range = self.tokens.get(self.position).map_or_else(
            || TextRange::empty(self.node.range().end()),
            |token| token.range(),
        );
        FunctionSignatureError {
            span: SourceSpan::new(self.file, range),
            expectation,
        }
    }
}

fn ordered_range(start: pop_foundation::TextSize, end: pop_foundation::TextSize) -> TextRange {
    TextRange::new(start, end).unwrap_or_else(|| TextRange::empty(start))
}
