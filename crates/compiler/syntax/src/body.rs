use pop_foundation::{FileId, SourceSpan, TextRange, TextSize};
use pop_source::SourceFile;

use crate::signature::{parse_type_prefix, parse_type_tokens};
use crate::{
    FunctionSignatureSyntax, NodeKind, SyntaxNode, SyntaxTree, Token, TokenKind, TypeSyntax,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FunctionBodySyntax {
    statements: Vec<StatementSyntax>,
    range: TextRange,
}

impl FunctionBodySyntax {
    #[must_use]
    pub fn statements(&self) -> &[StatementSyntax] {
        &self.statements
    }

    #[must_use]
    pub const fn range(&self) -> TextRange {
        self.range
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StatementSyntax {
    kind: StatementSyntaxKind,
    span: SourceSpan,
}

impl StatementSyntax {
    #[must_use]
    pub const fn kind(&self) -> &StatementSyntaxKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StatementSyntaxKind {
    Local {
        name: String,
        annotation: Option<TypeSyntax>,
        initializer: ExpressionSyntax,
    },
    LocalFunction {
        name: String,
        function: CaptureFunctionSyntax,
    },
    Return {
        values: Vec<ExpressionSyntax>,
    },
    If {
        condition: ExpressionSyntax,
        then_body: Vec<StatementSyntax>,
        else_body: Vec<StatementSyntax>,
    },
    While {
        condition: ExpressionSyntax,
        body: Vec<StatementSyntax>,
    },
    Match {
        scrutinee: ExpressionSyntax,
        arms: Vec<MatchArmSyntax>,
    },
    Assignment {
        target: ExpressionSyntax,
        value: ExpressionSyntax,
    },
    Expression(ExpressionSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExpressionSyntax {
    kind: ExpressionSyntaxKind,
    span: SourceSpan,
}

impl ExpressionSyntax {
    #[must_use]
    pub const fn kind(&self) -> &ExpressionSyntaxKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ExpressionSyntaxKind {
    Integer(String),
    String(String),
    Boolean(bool),
    Nil,
    Function(CaptureFunctionSyntax),
    Name(Vec<String>),
    Call {
        callee: Box<ExpressionSyntax>,
        arguments: Vec<ExpressionSyntax>,
    },
    GenericCall {
        callee: Box<ExpressionSyntax>,
        type_arguments: Vec<TypeSyntax>,
        arguments: Vec<ExpressionSyntax>,
    },
    MethodCall {
        receiver: Box<ExpressionSyntax>,
        method: String,
        arguments: Vec<ExpressionSyntax>,
    },
    Index {
        base: Box<ExpressionSyntax>,
        index: Box<ExpressionSyntax>,
    },
    Construct {
        type_name: Vec<String>,
        fields: Vec<FieldInitializerSyntax>,
    },
    Aggregate {
        fields: Vec<FieldInitializerSyntax>,
    },
    Array(Vec<ExpressionSyntax>),
    With {
        base: Box<ExpressionSyntax>,
        fields: Vec<FieldInitializerSyntax>,
    },
    Tuple(Vec<ExpressionSyntax>),
    Unary {
        operator: UnaryOperator,
        operand: Box<ExpressionSyntax>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<ExpressionSyntax>,
        right: Box<ExpressionSyntax>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFunctionSyntax {
    parameters: Vec<CaptureFunctionParameterSyntax>,
    results: Vec<TypeSyntax>,
    body: Vec<StatementSyntax>,
    span: SourceSpan,
}

impl CaptureFunctionSyntax {
    #[must_use]
    pub fn parameters(&self) -> &[CaptureFunctionParameterSyntax] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeSyntax] {
        &self.results
    }

    #[must_use]
    pub fn body(&self) -> &[StatementSyntax] {
        &self.body
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFunctionParameterSyntax {
    name: String,
    parameter_type: TypeSyntax,
    span: SourceSpan,
}

impl CaptureFunctionParameterSyntax {
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
pub struct MatchArmSyntax {
    case_path: Vec<String>,
    bindings: Vec<String>,
    body: Vec<StatementSyntax>,
    span: SourceSpan,
}

impl MatchArmSyntax {
    #[must_use]
    pub fn case_path(&self) -> &[String] {
        &self.case_path
    }

    #[must_use]
    pub fn bindings(&self) -> &[String] {
        &self.bindings
    }

    #[must_use]
    pub fn body(&self) -> &[StatementSyntax] {
        &self.body
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldInitializerSyntax {
    name: String,
    value: ExpressionSyntax,
    span: SourceSpan,
}

impl FieldInitializerSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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
pub enum UnaryOperator {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BinaryOperator {
    Or,
    And,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionBodyError {
    span: SourceSpan,
    expectation: &'static str,
}

impl FunctionBodyError {
    pub(crate) const fn new(span: SourceSpan, expectation: &'static str) -> Self {
        Self { span, expectation }
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn expectation(self) -> &'static str {
        self.expectation
    }
}

/// Parses statements and expressions owned by one function declaration.
///
/// # Errors
///
/// Returns [`FunctionBodyError`] if a required statement or expression token
/// is absent. Recovery never manufactures a dynamically typed expression.
pub fn parse_function_body(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
    signature: &FunctionSignatureSyntax,
) -> Result<FunctionBodySyntax, FunctionBodyError> {
    if node.kind() != NodeKind::FunctionDeclaration {
        return Err(FunctionBodyError {
            span: SourceSpan::new(source.id(), node.range()),
            expectation: "function declaration",
        });
    }
    let mut tokens: Vec<_> = syntax
        .tokens()
        .iter()
        .copied()
        .filter(|token| {
            !matches!(
                token.kind(),
                TokenKind::Whitespace | TokenKind::LineComment | TokenKind::DocumentationComment
            ) && token.range().start() >= signature.range().end()
                && token.range().end() <= node.range().end()
        })
        .collect();
    let Some(closing_end) = tokens
        .iter()
        .rposition(|token| token.kind() == TokenKind::End)
    else {
        return Err(FunctionBodyError {
            span: SourceSpan::new(source.id(), TextRange::empty(node.range().end())),
            expectation: "`end`",
        });
    };
    tokens.truncate(closing_end);
    BodyParser {
        source,
        file: source.id(),
        node,
        boundary_end: node.range().end(),
        tokens,
        position: 0,
    }
    .parse()
}

pub(crate) fn parse_body_range(
    source: &SourceFile,
    syntax: &SyntaxTree,
    node: &SyntaxNode,
    range: TextRange,
) -> Result<FunctionBodySyntax, FunctionBodyError> {
    let tokens = syntax
        .tokens()
        .iter()
        .copied()
        .filter(|token| {
            !matches!(
                token.kind(),
                TokenKind::Whitespace | TokenKind::LineComment | TokenKind::DocumentationComment
            ) && token.range().start() >= range.start()
                && token.range().end() <= range.end()
        })
        .collect();
    BodyParser {
        source,
        file: source.id(),
        node,
        boundary_end: range.end(),
        tokens,
        position: 0,
    }
    .parse()
}

pub(crate) fn parse_expression_prefix(
    source: &SourceFile,
    node: &SyntaxNode,
    tokens: &[Token],
    position: &mut usize,
) -> Result<ExpressionSyntax, FunctionBodyError> {
    let mut parser = BodyParser {
        source,
        file: source.id(),
        node,
        boundary_end: node.range().end(),
        tokens: tokens.to_vec(),
        position: *position,
    };
    let expression = parser.parse_expression(0)?;
    *position = parser.position;
    Ok(expression)
}

struct BodyParser<'source> {
    source: &'source SourceFile,
    file: FileId,
    node: &'source SyntaxNode,
    boundary_end: TextSize,
    tokens: Vec<Token>,
    position: usize,
}

impl BodyParser<'_> {
    fn parse(mut self) -> Result<FunctionBodySyntax, FunctionBodyError> {
        let statements = self.parse_statement_list(&[])?;
        if self.current_kind().is_some() {
            return Err(self.error("end of function body"));
        }
        let range = statements.first().map_or_else(
            || TextRange::empty(self.boundary_end),
            |first| {
                ordered_range(
                    first.span().range().start(),
                    statements
                        .last()
                        .map_or(first.span().range().end(), |last| last.span().range().end()),
                )
            },
        );
        Ok(FunctionBodySyntax { statements, range })
    }

    fn parse_statement(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        match self.current_kind() {
            Some(TokenKind::Local) => self.parse_local(),
            Some(TokenKind::Return) => self.parse_return(),
            Some(TokenKind::If) => self.parse_if(),
            Some(TokenKind::While) => self.parse_while(),
            Some(TokenKind::Match) => self.parse_match(),
            _ => {
                let expression = self.parse_expression(0)?;
                if self.consume(TokenKind::Equal).is_some() {
                    let value = self.parse_expression(0)?;
                    let span = SourceSpan::new(
                        self.file,
                        ordered_range(
                            expression.span().range().start(),
                            value.span().range().end(),
                        ),
                    );
                    return Ok(StatementSyntax {
                        kind: StatementSyntaxKind::Assignment {
                            target: expression,
                            value,
                        },
                        span,
                    });
                }
                let span = expression.span();
                Ok(StatementSyntax {
                    kind: StatementSyntaxKind::Expression(expression),
                    span,
                })
            }
        }
    }

    fn parse_statement_list(
        &mut self,
        terminators: &[TokenKind],
    ) -> Result<Vec<StatementSyntax>, FunctionBodyError> {
        let mut statements = Vec::new();
        self.skip_newlines();
        while self
            .current_kind()
            .is_some_and(|kind| !terminators.contains(&kind))
        {
            statements.push(self.parse_statement()?);
            if self
                .current_kind()
                .is_some_and(|kind| kind != TokenKind::Newline && !terminators.contains(&kind))
            {
                return Err(self.error("end of statement"));
            }
            self.skip_newlines();
        }
        Ok(statements)
    }

    fn parse_local(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::Local, "`local`")?.range().start();
        if let Some(function_token) = self.consume(TokenKind::Function) {
            let name = self.expect(TokenKind::Identifier, "local function name")?;
            let name = name.text(self.source).to_owned();
            let function = self.parse_capture_function(function_token.range().start())?;
            let end = function.span().range().end();
            return Ok(StatementSyntax {
                kind: StatementSyntaxKind::LocalFunction { name, function },
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            });
        }
        let name_token = self.expect(TokenKind::Identifier, "local name")?;
        let name = name_token.text(self.source).to_owned();
        let annotation = if self.consume(TokenKind::Colon).is_some() {
            let type_start = self.position;
            let Some(equal) = self.tokens[type_start..]
                .iter()
                .position(|token| token.kind() == TokenKind::Equal)
                .map(|offset| type_start + offset)
            else {
                return Err(self.error("`=`"));
            };
            if self.tokens[type_start..equal]
                .iter()
                .any(|token| token.kind() == TokenKind::Newline)
            {
                return Err(self.error("`=`"));
            }
            let parsed = parse_type_tokens(
                self.source,
                self.node,
                self.tokens[type_start..equal].to_vec(),
            )
            .map_err(|error| FunctionBodyError {
                span: error.span(),
                expectation: error.expectation(),
            })?;
            self.position = equal;
            Some(parsed)
        } else {
            None
        };
        self.expect(TokenKind::Equal, "`=`")?;
        let initializer = self.parse_expression(0)?;
        let end = initializer.span().range().end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::Local {
                name,
                annotation,
                initializer,
            },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_return(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let token = self.expect(TokenKind::Return, "`return`")?;
        let start = token.range().start();
        let mut end = token.range().end();
        let mut values = Vec::new();
        if !matches!(self.current_kind(), None | Some(TokenKind::Newline)) {
            loop {
                let value = self.parse_expression(0)?;
                end = value.span().range().end();
                values.push(value);
                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
        }
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::Return { values },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_if(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::If, "`if`")?.range().start();
        let condition = self.parse_expression(0)?;
        self.expect(TokenKind::Then, "`then`")?;
        self.expect(TokenKind::Newline, "line break after `then`")?;
        let then_body = self.parse_statement_list(&[TokenKind::Else, TokenKind::End])?;
        let else_body = if self.consume(TokenKind::Else).is_some() {
            self.expect(TokenKind::Newline, "line break after `else`")?;
            self.parse_statement_list(&[TokenKind::End])?
        } else {
            Vec::new()
        };
        let end = self.expect(TokenKind::End, "`end`")?.range().end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::If {
                condition,
                then_body,
                else_body,
            },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_while(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::While, "`while`")?.range().start();
        let condition = self.parse_expression(0)?;
        self.expect(TokenKind::Do, "`do`")?;
        self.expect(TokenKind::Newline, "line break after `do`")?;
        let body = self.parse_statement_list(&[TokenKind::End])?;
        let end = self.expect(TokenKind::End, "`end`")?.range().end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::While { condition, body },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_match(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::Match, "`match`")?.range().start();
        let scrutinee = self.parse_expression(0)?;
        self.expect(TokenKind::Newline, "line break after match scrutinee")?;
        self.skip_newlines();
        let mut arms = Vec::new();
        while self.current_kind() == Some(TokenKind::When) {
            arms.push(self.parse_match_arm()?);
            self.skip_newlines();
        }
        if arms.is_empty() {
            return Err(self.error("`when` arm"));
        }
        let end = self.expect(TokenKind::End, "match `end`")?.range().end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::Match { scrutinee, arms },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_match_arm(&mut self) -> Result<MatchArmSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::When, "`when`")?.range().start();
        let first = self.expect(TokenKind::Identifier, "qualified union case")?;
        let mut case_path = vec![first.text(self.source).to_owned()];
        while self.consume(TokenKind::Dot).is_some() {
            let component = self.expect(TokenKind::Identifier, "qualified union case")?;
            case_path.push(component.text(self.source).to_owned());
        }
        let mut bindings = Vec::new();
        if self.consume(TokenKind::LeftParenthesis).is_some() {
            while self.current_kind() != Some(TokenKind::RightParenthesis) {
                let binding = self.expect(TokenKind::Identifier, "case payload binding")?;
                bindings.push(binding.text(self.source).to_owned());
                if self.consume(TokenKind::Comma).is_none() {
                    break;
                }
            }
            self.expect(TokenKind::RightParenthesis, "`)`")?;
        }
        let then = self.expect(TokenKind::Then, "`then`")?;
        self.expect(TokenKind::Newline, "line break after match arm")?;
        let body = self.parse_statement_list(&[TokenKind::When, TokenKind::End])?;
        let end = body.last().map_or(then.range().end(), |statement| {
            statement.span().range().end()
        });
        Ok(MatchArmSyntax {
            case_path,
            bindings,
            body,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_capture_function(
        &mut self,
        start: TextSize,
    ) -> Result<CaptureFunctionSyntax, FunctionBodyError> {
        self.expect(TokenKind::LeftParenthesis, "`(`")?;
        let mut parameters = Vec::new();
        while self.current_kind() != Some(TokenKind::RightParenthesis) {
            let parameter = self.expect(TokenKind::Identifier, "parameter name")?;
            self.expect(TokenKind::Colon, "`:`")?;
            let parameter_type = self.parse_type_prefix()?;
            parameters.push(CaptureFunctionParameterSyntax {
                name: parameter.text(self.source).to_owned(),
                parameter_type,
                span: SourceSpan::new(self.file, parameter.range()),
            });
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        self.expect(TokenKind::RightParenthesis, "`)`")?;
        let results = if self.consume(TokenKind::Colon).is_some() {
            vec![self.parse_type_prefix()?]
        } else {
            Vec::new()
        };
        self.expect(TokenKind::Newline, "line break after function signature")?;
        let body = self.parse_statement_list(&[TokenKind::End])?;
        let end = self.expect(TokenKind::End, "function `end`")?.range().end();
        Ok(CaptureFunctionSyntax {
            parameters,
            results,
            body,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_type_prefix(&mut self) -> Result<TypeSyntax, FunctionBodyError> {
        parse_type_prefix(self.source, self.node, &self.tokens, &mut self.position).map_err(
            |error| FunctionBodyError {
                span: error.span(),
                expectation: error.expectation(),
            },
        )
    }

    fn parse_expression(
        &mut self,
        minimum_precedence: u8,
    ) -> Result<ExpressionSyntax, FunctionBodyError> {
        let mut left = self.parse_unary()?;
        while let Some((operator, precedence)) = self.current_binary_operator() {
            if precedence < minimum_precedence {
                break;
            }
            self.position += 1;
            let right = self.parse_expression(precedence + 1)?;
            let start = left.span().range().start();
            let end = right.span().range().end();
            left = self.expression(
                ExpressionSyntaxKind::Binary {
                    operator,
                    left: Box::new(left),
                    right: Box::new(right),
                },
                start,
                end,
            );
        }
        Ok(left)
    }

    fn parse_unary(&mut self) -> Result<ExpressionSyntax, FunctionBodyError> {
        let operator = match self.current_kind() {
            Some(TokenKind::Not) => UnaryOperator::Not,
            Some(TokenKind::Minus) => UnaryOperator::Negate,
            _ => return self.parse_postfix(),
        };
        let start = self
            .advance()
            .ok_or_else(|| self.error("unary operator"))?
            .range()
            .start();
        let operand = self.parse_unary()?;
        let end = operand.span().range().end();
        Ok(self.expression(
            ExpressionSyntaxKind::Unary {
                operator,
                operand: Box::new(operand),
            },
            start,
            end,
        ))
    }

    #[allow(clippy::too_many_lines)]
    fn parse_postfix(&mut self) -> Result<ExpressionSyntax, FunctionBodyError> {
        let mut expression = self.parse_primary()?;
        loop {
            if self.current_kind() == Some(TokenKind::LeftBrace)
                && let ExpressionSyntaxKind::Name(type_name) = expression.kind()
            {
                let type_name = type_name.clone();
                let start = expression.span().range().start();
                self.position += 1;
                let (fields, end) = self.parse_field_initializers()?;
                expression = self.expression(
                    ExpressionSyntaxKind::Construct { type_name, fields },
                    start,
                    end,
                );
                continue;
            }
            if self.consume(TokenKind::Colon).is_some() {
                let start = expression.span().range().start();
                let method = self.expect(TokenKind::Identifier, "method name")?;
                let method = method.text(self.source).to_owned();
                self.expect(TokenKind::LeftParenthesis, "`(`")?;
                let mut arguments = Vec::new();
                if self.current_kind() != Some(TokenKind::RightParenthesis) {
                    loop {
                        arguments.push(self.parse_expression(0)?);
                        if self.consume(TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                }
                let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
                expression = self.expression(
                    ExpressionSyntaxKind::MethodCall {
                        receiver: Box::new(expression),
                        method,
                        arguments,
                    },
                    start,
                    right.range().end(),
                );
                continue;
            }
            if self.consume(TokenKind::LeftParenthesis).is_some() {
                let mut arguments = Vec::new();
                if self.current_kind() != Some(TokenKind::RightParenthesis) {
                    loop {
                        arguments.push(self.parse_expression(0)?);
                        if self.consume(TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                }
                let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
                let start = expression.span().range().start();
                expression = self.expression(
                    ExpressionSyntaxKind::Call {
                        callee: Box::new(expression),
                        arguments,
                    },
                    start,
                    right.range().end(),
                );
                continue;
            }
            if self.current_kind() == Some(TokenKind::LessThan)
                && self.peek_kind() == Some(TokenKind::LessThan)
            {
                let start = expression.span().range().start();
                self.position += 2;
                let mut type_arguments = Vec::new();
                loop {
                    type_arguments.push(self.parse_type_prefix()?);
                    if self.consume(TokenKind::Comma).is_none() {
                        break;
                    }
                }
                self.expect(TokenKind::GreaterThan, "first `>` in `>>`")?;
                self.expect(TokenKind::GreaterThan, "second `>` in `>>`")?;
                self.expect(TokenKind::LeftParenthesis, "`(` after generic arguments")?;
                let mut arguments = Vec::new();
                if self.current_kind() != Some(TokenKind::RightParenthesis) {
                    loop {
                        arguments.push(self.parse_expression(0)?);
                        if self.consume(TokenKind::Comma).is_none() {
                            break;
                        }
                    }
                }
                let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
                expression = self.expression(
                    ExpressionSyntaxKind::GenericCall {
                        callee: Box::new(expression),
                        type_arguments,
                        arguments,
                    },
                    start,
                    right.range().end(),
                );
                continue;
            }
            if self.consume(TokenKind::LeftBracket).is_some() {
                let start = expression.span().range().start();
                let index = self.parse_expression(0)?;
                let right = self.expect(TokenKind::RightBracket, "`]`")?;
                expression = self.expression(
                    ExpressionSyntaxKind::Index {
                        base: Box::new(expression),
                        index: Box::new(index),
                    },
                    start,
                    right.range().end(),
                );
                continue;
            }
            if self.consume(TokenKind::With).is_some() {
                let start = expression.span().range().start();
                self.expect(TokenKind::LeftBrace, "`{`")?;
                let (fields, end) = self.parse_field_initializers()?;
                expression = self.expression(
                    ExpressionSyntaxKind::With {
                        base: Box::new(expression),
                        fields,
                    },
                    start,
                    end,
                );
                continue;
            }
            break;
        }
        Ok(expression)
    }

    fn parse_primary(&mut self) -> Result<ExpressionSyntax, FunctionBodyError> {
        let Some(token) = self.advance() else {
            return Err(self.error("expression"));
        };
        let start = token.range().start();
        let end = token.range().end();
        match token.kind() {
            TokenKind::Number => Ok(self.expression(
                ExpressionSyntaxKind::Integer(token.text(self.source).to_owned()),
                start,
                end,
            )),
            TokenKind::String => Ok(self.expression(
                ExpressionSyntaxKind::String(token.text(self.source).to_owned()),
                start,
                end,
            )),
            TokenKind::True | TokenKind::False => Ok(self.expression(
                ExpressionSyntaxKind::Boolean(token.kind() == TokenKind::True),
                start,
                end,
            )),
            TokenKind::Nil => Ok(self.expression(ExpressionSyntaxKind::Nil, start, end)),
            TokenKind::Function => {
                let function = self.parse_capture_function(start)?;
                let end = function.span().range().end();
                Ok(self.expression(ExpressionSyntaxKind::Function(function), start, end))
            }
            TokenKind::Identifier | TokenKind::Attribute => self.parse_name(token),
            TokenKind::LeftParenthesis => self.parse_parenthesized(start),
            TokenKind::LeftBrace => self.parse_brace_literal(start),
            _ => Err(FunctionBodyError {
                span: SourceSpan::new(self.file, token.range()),
                expectation: "expression",
            }),
        }
    }

    fn parse_field_initializers(
        &mut self,
    ) -> Result<(Vec<FieldInitializerSyntax>, TextSize), FunctionBodyError> {
        let mut fields = Vec::new();
        self.skip_newlines();
        while self.current_kind() != Some(TokenKind::RightBrace) {
            let name = self.expect(TokenKind::Identifier, "field name")?;
            let start = name.range().start();
            self.expect(TokenKind::Equal, "`=`")?;
            let value = self.parse_expression(0)?;
            let end = value.span().range().end();
            fields.push(FieldInitializerSyntax {
                name: name.text(self.source).to_owned(),
                value,
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            });
            if self.consume(TokenKind::Comma).is_some() {
                self.skip_newlines();
            } else {
                self.skip_newlines();
                if self.current_kind() != Some(TokenKind::RightBrace) {
                    return Err(self.error("`,` or `}`"));
                }
            }
        }
        let right = self.expect(TokenKind::RightBrace, "`}`")?;
        Ok((fields, right.range().end()))
    }

    fn parse_brace_literal(
        &mut self,
        start: TextSize,
    ) -> Result<ExpressionSyntax, FunctionBodyError> {
        self.skip_newlines();
        if self.current_kind() == Some(TokenKind::RightBrace)
            || (self.current_kind() == Some(TokenKind::Identifier)
                && self.peek_kind() == Some(TokenKind::Equal))
        {
            let (fields, end) = self.parse_field_initializers()?;
            return Ok(self.expression(ExpressionSyntaxKind::Aggregate { fields }, start, end));
        }
        let mut elements = Vec::new();
        while self.current_kind() != Some(TokenKind::RightBrace) {
            elements.push(self.parse_expression(0)?);
            if self.consume(TokenKind::Comma).is_some() {
                self.skip_newlines();
            } else {
                self.skip_newlines();
                if self.current_kind() != Some(TokenKind::RightBrace) {
                    return Err(self.error("`,` or `}`"));
                }
            }
        }
        let right = self.expect(TokenKind::RightBrace, "`}`")?;
        Ok(self.expression(
            ExpressionSyntaxKind::Array(elements),
            start,
            right.range().end(),
        ))
    }

    fn parse_name(&mut self, first: Token) -> Result<ExpressionSyntax, FunctionBodyError> {
        let start = first.range().start();
        let mut end = first.range().end();
        let mut path = vec![first.text(self.source).to_owned()];
        while self.consume(TokenKind::Dot).is_some() {
            let component = self.expect(TokenKind::Identifier, "qualified name")?;
            end = component.range().end();
            path.push(component.text(self.source).to_owned());
        }
        Ok(self.expression(ExpressionSyntaxKind::Name(path), start, end))
    }

    fn parse_parenthesized(
        &mut self,
        start: TextSize,
    ) -> Result<ExpressionSyntax, FunctionBodyError> {
        if let Some(right) = self.consume(TokenKind::RightParenthesis) {
            return Ok(self.expression(
                ExpressionSyntaxKind::Tuple(Vec::new()),
                start,
                right.range().end(),
            ));
        }
        let first = self.parse_expression(0)?;
        if self.consume(TokenKind::Comma).is_none() {
            self.expect(TokenKind::RightParenthesis, "`)`")?;
            return Ok(first);
        }
        let mut elements = vec![first];
        loop {
            elements.push(self.parse_expression(0)?);
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        let right = self.expect(TokenKind::RightParenthesis, "`)`")?;
        Ok(self.expression(
            ExpressionSyntaxKind::Tuple(elements),
            start,
            right.range().end(),
        ))
    }

    fn current_binary_operator(&self) -> Option<(BinaryOperator, u8)> {
        Some(match self.current_kind()? {
            TokenKind::Or => (BinaryOperator::Or, 1),
            TokenKind::And => (BinaryOperator::And, 2),
            TokenKind::EqualEqual => (BinaryOperator::Equal, 3),
            TokenKind::TildeEqual => (BinaryOperator::NotEqual, 3),
            TokenKind::LessThan => (BinaryOperator::LessThan, 3),
            TokenKind::GreaterThan => (BinaryOperator::GreaterThan, 3),
            TokenKind::Plus => (BinaryOperator::Add, 4),
            TokenKind::Minus => (BinaryOperator::Subtract, 4),
            TokenKind::Star => (BinaryOperator::Multiply, 5),
            TokenKind::Slash => (BinaryOperator::Divide, 5),
            TokenKind::Percent => (BinaryOperator::Remainder, 5),
            _ => return None,
        })
    }

    fn expression(
        &self,
        kind: ExpressionSyntaxKind,
        start: TextSize,
        end: TextSize,
    ) -> ExpressionSyntax {
        ExpressionSyntax {
            kind,
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        }
    }

    fn skip_newlines(&mut self) {
        while self.consume(TokenKind::Newline).is_some() {}
    }

    fn current_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position).map(|token| token.kind())
    }

    fn peek_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position + 1).map(|token| token.kind())
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
    ) -> Result<Token, FunctionBodyError> {
        if self.current_kind() == Some(kind) {
            self.advance().ok_or_else(|| self.error(expectation))
        } else {
            Err(self.error(expectation))
        }
    }

    fn error(&self, expectation: &'static str) -> FunctionBodyError {
        let range = self.tokens.get(self.position).map_or_else(
            || TextRange::empty(self.boundary_end),
            |token| token.range(),
        );
        FunctionBodyError {
            span: SourceSpan::new(self.file, range),
            expectation,
        }
    }
}

fn ordered_range(start: TextSize, end: TextSize) -> TextRange {
    TextRange::new(start, end).unwrap_or_else(|| TextRange::empty(start))
}
