use pop_foundation::{FileId, SourceSpan, TextRange};
use pop_source::SourceFile;

use crate::attribute::parse_attribute_use_prefix;
use crate::body::parse_expression_prefix;
use crate::signature::parse_type_prefix;
use crate::{
    AttributeUseSyntax, ExpressionSyntax, GenericParameterSyntax, NodeKind, SyntaxNode, SyntaxTree,
    Token, TokenKind, TypeSyntax,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordDeclarationSyntax {
    name: String,
    type_parameters: Vec<GenericParameterSyntax>,
    fields: Vec<RecordFieldSyntax>,
    span: SourceSpan,
}

impl RecordDeclarationSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[GenericParameterSyntax] {
        &self.type_parameters
    }

    #[must_use]
    pub fn fields(&self) -> &[RecordFieldSyntax] {
        &self.fields
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordFieldSyntax {
    attributes: Vec<AttributeUseSyntax>,
    name: String,
    field_type: TypeSyntax,
    default_value: Option<ExpressionSyntax>,
    span: SourceSpan,
}

impl RecordFieldSyntax {
    #[must_use]
    pub fn attributes(&self) -> &[AttributeUseSyntax] {
        &self.attributes
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn field_type(&self) -> &TypeSyntax {
        &self.field_type
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
pub struct UnionDeclarationSyntax {
    name: String,
    type_parameters: Vec<GenericParameterSyntax>,
    cases: Vec<UnionCaseSyntax>,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorDeclarationSyntax {
    name: String,
    type_parameters: Vec<GenericParameterSyntax>,
    cases: Vec<ErrorCaseSyntax>,
    span: SourceSpan,
}

impl ErrorDeclarationSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[GenericParameterSyntax] {
        &self.type_parameters
    }

    #[must_use]
    pub fn cases(&self) -> &[ErrorCaseSyntax] {
        &self.cases
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorCaseSyntax {
    attributes: Vec<AttributeUseSyntax>,
    name: String,
    payload: Vec<ErrorCaseParameterSyntax>,
    span: SourceSpan,
}

impl ErrorCaseSyntax {
    #[must_use]
    pub fn attributes(&self) -> &[AttributeUseSyntax] {
        &self.attributes
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn payload(&self) -> &[ErrorCaseParameterSyntax] {
        &self.payload
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorCaseParameterSyntax {
    name: String,
    parameter_type: TypeSyntax,
    span: SourceSpan,
}

impl ErrorCaseParameterSyntax {
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
pub struct EnumDeclarationSyntax {
    name: String,
    cases: Vec<EnumCaseSyntax>,
    span: SourceSpan,
}

impl EnumDeclarationSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn cases(&self) -> &[EnumCaseSyntax] {
        &self.cases
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumCaseSyntax {
    attributes: Vec<AttributeUseSyntax>,
    name: String,
    span: SourceSpan,
}

impl EnumCaseSyntax {
    #[must_use]
    pub fn attributes(&self) -> &[AttributeUseSyntax] {
        &self.attributes
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl UnionDeclarationSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[GenericParameterSyntax] {
        &self.type_parameters
    }

    #[must_use]
    pub fn cases(&self) -> &[UnionCaseSyntax] {
        &self.cases
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnionCaseSyntax {
    attributes: Vec<AttributeUseSyntax>,
    name: String,
    payload: Vec<UnionCaseParameterSyntax>,
    span: SourceSpan,
}

impl UnionCaseSyntax {
    #[must_use]
    pub fn attributes(&self) -> &[AttributeUseSyntax] {
        &self.attributes
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn payload(&self) -> &[UnionCaseParameterSyntax] {
        &self.payload
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnionCaseParameterSyntax {
    name: String,
    parameter_type: TypeSyntax,
    span: SourceSpan,
}

impl UnionCaseParameterSyntax {
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
pub struct DataDeclarationError {
    span: SourceSpan,
    expectation: &'static str,
}

impl DataDeclarationError {
    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses one native record declaration.
///
/// # Errors
///
/// Returns an error when the declaration or a field lacks required syntax.
pub fn parse_record_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<RecordDeclarationSyntax, DataDeclarationError> {
    let mut parser = DataParser::new(source, syntax, node);
    if node.kind() != NodeKind::RecordDeclaration {
        return Err(parser.error("record declaration"));
    }
    parser.seek(TokenKind::Record)?;
    let name_token = parser.expect(TokenKind::Identifier, "record name")?;
    let name = name_token.text(source).to_owned();
    let type_parameters = parser.parse_type_parameters()?;
    parser.expect(TokenKind::Newline, "line break after record name")?;
    let mut fields = Vec::new();
    loop {
        parser.skip_newlines();
        let attributes = parser.parse_member_attributes()?;
        if parser.consume(TokenKind::End).is_some() {
            if !attributes.is_empty() {
                return Err(parser.error("member after attribute"));
            }
            break;
        }
        let field_name = parser.expect(TokenKind::Identifier, "field name")?;
        let start = field_name.range().start();
        parser.expect(TokenKind::Colon, "`:`")?;
        let field_type = parser.parse_type()?;
        let default_value = if parser.consume(TokenKind::Equal).is_some() {
            Some(parser.parse_expression()?)
        } else {
            None
        };
        let end = default_value.as_ref().map_or_else(
            || field_type.span().range().end(),
            |value| value.span().range().end(),
        );
        fields.push(RecordFieldSyntax {
            attributes,
            name: field_name.text(source).to_owned(),
            field_type,
            default_value,
            span: SourceSpan::new(source.id(), ordered_range(start, end)),
        });
        parser.expect_line_end()?;
    }
    Ok(RecordDeclarationSyntax {
        name,
        type_parameters,
        fields,
        span: SourceSpan::new(source.id(), node.range()),
    })
}

/// Parses one native tagged-union declaration.
///
/// # Errors
///
/// Returns an error when a case or typed payload is malformed.
pub fn parse_union_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<UnionDeclarationSyntax, DataDeclarationError> {
    let mut parser = DataParser::new(source, syntax, node);
    if node.kind() != NodeKind::UnionDeclaration {
        return Err(parser.error("union declaration"));
    }
    parser.seek(TokenKind::Union)?;
    let name_token = parser.expect(TokenKind::Identifier, "union name")?;
    let name = name_token.text(source).to_owned();
    let type_parameters = parser.parse_type_parameters()?;
    parser.expect(TokenKind::Newline, "line break after union name")?;
    let mut cases = Vec::new();
    loop {
        parser.skip_newlines();
        let attributes = parser.parse_member_attributes()?;
        if parser.consume(TokenKind::End).is_some() {
            if !attributes.is_empty() {
                return Err(parser.error("member after attribute"));
            }
            break;
        }
        let case_name = parser.expect(TokenKind::Identifier, "union case name")?;
        let start = case_name.range().start();
        let mut end = case_name.range().end();
        let mut payload = Vec::new();
        if parser.consume(TokenKind::LeftParenthesis).is_some() {
            while parser.current_kind() != Some(TokenKind::RightParenthesis) {
                let parameter = parser.expect(TokenKind::Identifier, "case parameter name")?;
                parser.expect(TokenKind::Colon, "`:`")?;
                let parameter_type = parser.parse_type()?;
                payload.push(UnionCaseParameterSyntax {
                    name: parameter.text(source).to_owned(),
                    parameter_type,
                    span: SourceSpan::new(source.id(), parameter.range()),
                });
                if parser.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
            end = parser
                .expect(TokenKind::RightParenthesis, "`)`")?
                .range()
                .end();
        }
        cases.push(UnionCaseSyntax {
            attributes,
            name: case_name.text(source).to_owned(),
            payload,
            span: SourceSpan::new(source.id(), ordered_range(start, end)),
        });
        parser.expect_line_end()?;
    }
    Ok(UnionDeclarationSyntax {
        name,
        type_parameters,
        cases,
        span: SourceSpan::new(source.id(), node.range()),
    })
}

/// Parses one closed nominal recoverable-error declaration.
///
/// # Errors
///
/// Returns an error when a case or typed payload is malformed.
pub fn parse_error_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<ErrorDeclarationSyntax, DataDeclarationError> {
    let mut parser = DataParser::new(source, syntax, node);
    if node.kind() != NodeKind::ErrorDeclaration {
        return Err(parser.error("error declaration"));
    }
    if !parser
        .tokens
        .iter()
        .any(|token| token.kind() == TokenKind::Identifier && token.text(source) == "error")
    {
        return Err(parser.error("`error`"));
    }
    parser.seek(TokenKind::Identifier)?;
    let name_token = parser.expect(TokenKind::Identifier, "error name")?;
    let name = name_token.text(source).to_owned();
    let type_parameters = parser.parse_type_parameters()?;
    parser.expect(TokenKind::Newline, "line break after error name")?;
    let mut cases = Vec::new();
    loop {
        parser.skip_newlines();
        let attributes = parser.parse_member_attributes()?;
        if parser.consume(TokenKind::End).is_some() {
            if !attributes.is_empty() {
                return Err(parser.error("member after attribute"));
            }
            break;
        }
        let case_name = parser.expect(TokenKind::Identifier, "error case name")?;
        let start = case_name.range().start();
        let mut end = case_name.range().end();
        let mut payload = Vec::new();
        if parser.consume(TokenKind::LeftParenthesis).is_some() {
            while parser.current_kind() != Some(TokenKind::RightParenthesis) {
                let parameter = parser.expect(TokenKind::Identifier, "case parameter name")?;
                parser.expect(TokenKind::Colon, "`:`")?;
                let parameter_type = parser.parse_type()?;
                payload.push(ErrorCaseParameterSyntax {
                    name: parameter.text(source).to_owned(),
                    parameter_type,
                    span: SourceSpan::new(source.id(), parameter.range()),
                });
                if parser.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
            end = parser
                .expect(TokenKind::RightParenthesis, "`)`")?
                .range()
                .end();
        }
        cases.push(ErrorCaseSyntax {
            attributes,
            name: case_name.text(source).to_owned(),
            payload,
            span: SourceSpan::new(source.id(), ordered_range(start, end)),
        });
        parser.expect_line_end()?;
    }
    Ok(ErrorDeclarationSyntax {
        name,
        type_parameters,
        cases,
        span: SourceSpan::new(source.id(), node.range()),
    })
}

/// Parses one nominal payload-free enum declaration.
///
/// # Errors
///
/// Returns an error when the declaration or a case is malformed.
pub fn parse_enum_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<EnumDeclarationSyntax, DataDeclarationError> {
    let mut parser = DataParser::new(source, syntax, node);
    if node.kind() != NodeKind::EnumDeclaration {
        return Err(parser.error("enum declaration"));
    }
    parser.seek(TokenKind::Enum)?;
    let name_token = parser.expect(TokenKind::Identifier, "enum name")?;
    let name = name_token.text(source).to_owned();
    parser.expect(TokenKind::Newline, "line break after enum name")?;
    let mut cases = Vec::new();
    loop {
        parser.skip_newlines();
        let attributes = parser.parse_member_attributes()?;
        if parser.consume(TokenKind::End).is_some() {
            if !attributes.is_empty() {
                return Err(parser.error("case after attribute"));
            }
            break;
        }
        let case = parser.expect(TokenKind::Identifier, "enum case name")?;
        cases.push(EnumCaseSyntax {
            attributes,
            name: case.text(source).to_owned(),
            span: SourceSpan::new(source.id(), case.range()),
        });
        parser.expect_line_end()?;
    }
    Ok(EnumDeclarationSyntax {
        name,
        cases,
        span: SourceSpan::new(source.id(), node.range()),
    })
}

struct DataParser<'source> {
    source: &'source SourceFile,
    node: &'source SyntaxNode,
    file: FileId,
    tokens: Vec<Token>,
    position: usize,
}

impl<'source> DataParser<'source> {
    fn new(source: &'source SourceFile, syntax: &SyntaxTree, node: &'source SyntaxNode) -> Self {
        let tokens = syntax
            .tokens()
            .iter()
            .copied()
            .filter(|token| {
                !matches!(
                    token.kind(),
                    TokenKind::Whitespace
                        | TokenKind::LineComment
                        | TokenKind::DocumentationComment
                ) && token.range().start() >= node.range().start()
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

    fn parse_type_parameters(
        &mut self,
    ) -> Result<Vec<GenericParameterSyntax>, DataDeclarationError> {
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

    fn parse_type(&mut self) -> Result<TypeSyntax, DataDeclarationError> {
        parse_type_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
            |error| DataDeclarationError {
                span: error.span(),
                expectation: error.expectation(),
            },
        )
    }

    fn parse_expression(&mut self) -> Result<ExpressionSyntax, DataDeclarationError> {
        parse_expression_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
            |error| DataDeclarationError {
                span: error.span(),
                expectation: error.expectation(),
            },
        )
    }

    fn parse_member_attributes(&mut self) -> Result<Vec<AttributeUseSyntax>, DataDeclarationError> {
        let mut attributes = Vec::new();
        while self.current_kind() == Some(TokenKind::At) {
            attributes.push(
                parse_attribute_use_prefix(
                    self.source,
                    self.node,
                    &self.tokens,
                    &mut self.position,
                )
                .map_err(|error| DataDeclarationError {
                    span: error.span(),
                    expectation: error.expectation(),
                })?,
            );
            self.skip_newlines();
        }
        Ok(attributes)
    }

    fn seek(&mut self, kind: TokenKind) -> Result<(), DataDeclarationError> {
        while self.current_kind().is_some_and(|current| current != kind) {
            self.position += 1;
        }
        self.expect(kind, "declaration keyword").map(|_| ())
    }

    fn expect_line_end(&mut self) -> Result<(), DataDeclarationError> {
        if self.consume(TokenKind::Newline).is_some() || self.current_kind() == Some(TokenKind::End)
        {
            Ok(())
        } else {
            Err(self.error("end of member declaration"))
        }
    }

    fn skip_newlines(&mut self) {
        while self.consume(TokenKind::Newline).is_some() {}
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
    ) -> Result<Token, DataDeclarationError> {
        if self.current_kind() == Some(kind) {
            self.advance().ok_or_else(|| self.error(expectation))
        } else {
            Err(self.error(expectation))
        }
    }

    fn error(&self, expectation: &'static str) -> DataDeclarationError {
        let range = self.tokens.get(self.position).map_or_else(
            || TextRange::empty(self.node.range().end()),
            |token| token.range(),
        );
        DataDeclarationError {
            span: SourceSpan::new(self.file, range),
            expectation,
        }
    }
}

fn ordered_range(start: pop_foundation::TextSize, end: pop_foundation::TextSize) -> TextRange {
    TextRange::new(start, end).unwrap_or_else(|| TextRange::empty(start))
}
