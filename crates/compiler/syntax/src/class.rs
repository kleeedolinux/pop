use pop_foundation::{FileId, SourceSpan, TextRange, TextSize};
use pop_source::SourceFile;

use crate::body::{parse_body_range, parse_expression_prefix};
use crate::signature::{GenericParameterSyntax, parse_type_prefix};
use crate::{
    ExpressionSyntax, FunctionBodyError, FunctionBodySyntax, NodeKind, SyntaxNode, SyntaxTree,
    Token, TokenKind, TypeSyntax,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VisibilitySyntax {
    Public,
    Internal,
    Private,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ClassMethodDispatchSyntax {
    Static,
    Receiver,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassDeclarationSyntax {
    name: String,
    type_parameters: Vec<GenericParameterSyntax>,
    is_open: bool,
    interfaces: Vec<TypeSyntax>,
    fields: Vec<ClassFieldSyntax>,
    methods: Vec<ClassMethodSyntax>,
    span: SourceSpan,
}

impl ClassDeclarationSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[GenericParameterSyntax] {
        &self.type_parameters
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.is_open
    }

    #[must_use]
    pub fn interfaces(&self) -> &[TypeSyntax] {
        &self.interfaces
    }

    #[must_use]
    pub fn fields(&self) -> &[ClassFieldSyntax] {
        &self.fields
    }

    #[must_use]
    pub fn methods(&self) -> &[ClassMethodSyntax] {
        &self.methods
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassFieldSyntax {
    visibility: VisibilitySyntax,
    name: String,
    field_type: TypeSyntax,
    default: Option<ExpressionSyntax>,
    span: SourceSpan,
}

impl ClassFieldSyntax {
    #[must_use]
    pub const fn visibility(&self) -> VisibilitySyntax {
        self.visibility
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
    pub const fn default(&self) -> Option<&ExpressionSyntax> {
        self.default.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassMethodSyntax {
    visibility: VisibilitySyntax,
    owner: String,
    name: String,
    dispatch: ClassMethodDispatchSyntax,
    parameters: Vec<ClassMethodParameterSyntax>,
    results: Vec<TypeSyntax>,
    signature_span: SourceSpan,
    body_range: TextRange,
}

impl ClassMethodSyntax {
    #[must_use]
    pub const fn visibility(&self) -> VisibilitySyntax {
        self.visibility
    }

    #[must_use]
    pub fn owner(&self) -> &str {
        &self.owner
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn dispatch(&self) -> ClassMethodDispatchSyntax {
        self.dispatch
    }

    #[must_use]
    pub fn parameters(&self) -> &[ClassMethodParameterSyntax] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeSyntax] {
        &self.results
    }

    #[must_use]
    pub const fn signature_span(&self) -> SourceSpan {
        self.signature_span
    }

    #[must_use]
    pub const fn body_range(&self) -> TextRange {
        self.body_range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassMethodParameterSyntax {
    name: String,
    parameter_type: TypeSyntax,
    span: SourceSpan,
}

impl ClassMethodParameterSyntax {
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
pub struct ClassDeclarationError {
    span: SourceSpan,
    expectation: &'static str,
}

impl ClassDeclarationError {
    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses the native fields and owned methods of one class declaration.
///
/// # Errors
///
/// Returns a precise syntax error for a required type/name delimiter. Omitted
/// class and class-member visibility is `internal`; it never reinterprets
/// members as table entries or runtime string lookups.
pub fn parse_class_declaration(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
) -> Result<ClassDeclarationSyntax, ClassDeclarationError> {
    if node.kind() != NodeKind::ClassDeclaration {
        return Err(ClassDeclarationError {
            span: SourceSpan::new(source.id(), node.range()),
            expectation: "class declaration",
        });
    }
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
    ClassParser {
        source,
        file: source.id(),
        node,
        tokens,
        position: 0,
    }
    .parse()
}

/// Parses one class method body using the exact range retained by the class parser.
///
/// # Errors
///
/// Returns a body syntax error without extending recovery into adjacent methods.
pub fn parse_class_method_body(
    source: &SourceFile,
    syntax: &SyntaxTree,
    class_node: &SyntaxNode,
    method: &ClassMethodSyntax,
) -> Result<FunctionBodySyntax, FunctionBodyError> {
    if class_node.kind() != NodeKind::ClassDeclaration {
        return Err(FunctionBodyError::new(
            SourceSpan::new(source.id(), class_node.range()),
            "class declaration",
        ));
    }
    parse_body_range(source, syntax, class_node, method.body_range())
}

struct ClassParser<'source> {
    source: &'source SourceFile,
    file: FileId,
    node: &'source SyntaxNode,
    tokens: Vec<Token>,
    position: usize,
}

impl ClassParser<'_> {
    fn parse(mut self) -> Result<ClassDeclarationSyntax, ClassDeclarationError> {
        let start = self.current_span().range().start();
        self.parse_visibility_or_internal();
        let is_open = self.consume(TokenKind::Open).is_some();
        self.expect(TokenKind::Class, "`class`")?;
        let name = self.expect(TokenKind::Identifier, "class name")?;
        let name = name.text(self.source).to_owned();
        let type_parameters = self.parse_type_parameters()?;
        let mut interfaces = Vec::new();
        if self.consume(TokenKind::Implements).is_some() {
            loop {
                interfaces.push(self.parse_type()?);
                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        self.expect(TokenKind::Newline, "line break after class declaration")?;
        let mut fields = Vec::new();
        let mut methods = Vec::new();
        self.skip_newlines();
        while self.current_kind() != Some(TokenKind::End) {
            let visibility = self.parse_visibility_or_internal();
            if self.current_kind() == Some(TokenKind::Function) {
                methods.push(self.parse_method(visibility)?);
            } else {
                fields.push(self.parse_field(visibility)?);
            }
            self.skip_newlines();
        }
        let end = self.expect(TokenKind::End, "`end`")?.range().end();
        Ok(ClassDeclarationSyntax {
            name,
            type_parameters,
            is_open,
            interfaces,
            fields,
            methods,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_type_parameters(
        &mut self,
    ) -> Result<Vec<GenericParameterSyntax>, ClassDeclarationError> {
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

    fn parse_field(
        &mut self,
        visibility: VisibilitySyntax,
    ) -> Result<ClassFieldSyntax, ClassDeclarationError> {
        let name = self.expect(TokenKind::Identifier, "field name")?;
        let start = name.range().start();
        let field_name = name.text(self.source).to_owned();
        self.expect(TokenKind::Colon, "`:`")?;
        let field_type =
            parse_type_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
                |error| ClassDeclarationError {
                    span: error.span(),
                    expectation: error.expectation(),
                },
            )?;
        let default = if self.consume(TokenKind::Equal).is_some() {
            Some(
                parse_expression_prefix(self.source, self.node, &self.tokens, &mut self.position)
                    .map_err(|error| ClassDeclarationError {
                    span: error.span(),
                    expectation: error.expectation(),
                })?,
            )
        } else {
            None
        };
        let end = default
            .as_ref()
            .map_or(field_type.span().range().end(), |value| {
                value.span().range().end()
            });
        self.expect_line_end()?;
        Ok(ClassFieldSyntax {
            visibility,
            name: field_name,
            field_type,
            default,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_method(
        &mut self,
        visibility: VisibilitySyntax,
    ) -> Result<ClassMethodSyntax, ClassDeclarationError> {
        let start = self
            .expect(TokenKind::Function, "`function`")?
            .range()
            .start();
        let owner = self.expect(TokenKind::Identifier, "method owner")?;
        let owner = owner.text(self.source).to_owned();
        let dispatch = if self.consume(TokenKind::Colon).is_some() {
            ClassMethodDispatchSyntax::Receiver
        } else {
            self.expect(TokenKind::Dot, "`.` or `:`")?;
            ClassMethodDispatchSyntax::Static
        };
        let name = self.expect(TokenKind::Identifier, "method name")?;
        let name = name.text(self.source).to_owned();
        self.expect(TokenKind::LeftParenthesis, "`(`")?;
        let parameters = self.parse_method_parameters()?;
        let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
        let results = if self.consume(TokenKind::Colon).is_some() {
            vec![self.parse_type()?]
        } else {
            Vec::new()
        };
        let signature_end = results
            .last()
            .map_or(right.range().end(), |result| result.span().range().end());
        self.expect(TokenKind::Newline, "line break after method signature")?;
        let body_start = self
            .tokens
            .get(self.position)
            .map_or(signature_end, |token| token.range().start());
        let body_end = self.skip_method_body()?;
        Ok(ClassMethodSyntax {
            visibility,
            owner,
            name,
            dispatch,
            parameters,
            results,
            signature_span: SourceSpan::new(self.file, ordered_range(start, signature_end)),
            body_range: ordered_range(body_start, body_end),
        })
    }

    fn parse_method_parameters(
        &mut self,
    ) -> Result<Vec<ClassMethodParameterSyntax>, ClassDeclarationError> {
        let mut parameters = Vec::new();
        while self.current_kind() != Some(TokenKind::RightParenthesis) {
            let name = self.expect(TokenKind::Identifier, "parameter name")?;
            self.expect(TokenKind::Colon, "`:`")?;
            let parameter_type = self.parse_type()?;
            parameters.push(ClassMethodParameterSyntax {
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

    fn parse_type(&mut self) -> Result<TypeSyntax, ClassDeclarationError> {
        parse_type_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
            |error| ClassDeclarationError {
                span: error.span(),
                expectation: error.expectation(),
            },
        )
    }

    fn skip_method_body(&mut self) -> Result<TextSize, ClassDeclarationError> {
        let mut depth = 1_u32;
        while let Some(token) = self.advance() {
            match token.kind() {
                TokenKind::Function
                | TokenKind::If
                | TokenKind::While
                | TokenKind::For
                | TokenKind::Match => depth = depth.saturating_add(1),
                TokenKind::End => {
                    depth = depth.saturating_sub(1);
                    if depth == 0 {
                        return Ok(token.range().start());
                    }
                }
                _ => {}
            }
        }
        Err(self.error("method `end`"))
    }

    fn parse_visibility_or_internal(&mut self) -> VisibilitySyntax {
        let visibility = match self.current_kind() {
            Some(TokenKind::Public) => VisibilitySyntax::Public,
            Some(TokenKind::Internal) => VisibilitySyntax::Internal,
            Some(TokenKind::Private) => VisibilitySyntax::Private,
            _ => return VisibilitySyntax::Internal,
        };
        self.position += 1;
        visibility
    }

    fn expect_line_end(&mut self) -> Result<(), ClassDeclarationError> {
        if self.consume(TokenKind::Newline).is_some() || self.current_kind() == Some(TokenKind::End)
        {
            Ok(())
        } else {
            Err(self.error("end of class member"))
        }
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
    ) -> Result<Token, ClassDeclarationError> {
        self.consume(kind).ok_or_else(|| self.error(expectation))
    }

    fn error(&self, expectation: &'static str) -> ClassDeclarationError {
        ClassDeclarationError {
            span: self.current_span(),
            expectation,
        }
    }
}

fn ordered_range(start: TextSize, end: TextSize) -> TextRange {
    TextRange::new(start, end).unwrap_or_else(|| TextRange::empty(start))
}
