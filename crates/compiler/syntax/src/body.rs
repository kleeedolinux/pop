use pop_foundation::{FileId, SourceSpan, TextRange, TextSize};
use pop_source::SourceFile;

use crate::signature::parse_type_prefix;
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
    MultipleLocal {
        bindings: Vec<LocalBindingSyntax>,
        values: Vec<ExpressionSyntax>,
    },
    Return {
        values: Vec<ExpressionSyntax>,
    },
    If {
        condition: ExpressionSyntax,
        then_body: Vec<StatementSyntax>,
        else_body: Vec<StatementSyntax>,
    },
    OptionalIf {
        name: String,
        initializer: ExpressionSyntax,
        then_body: Vec<StatementSyntax>,
        else_body: Vec<StatementSyntax>,
    },
    While {
        condition: ExpressionSyntax,
        body: Vec<StatementSyntax>,
    },
    OptionalWhile {
        name: String,
        initializer: ExpressionSyntax,
        body: Vec<StatementSyntax>,
    },
    RepeatUntil {
        body: Vec<StatementSyntax>,
        condition: ExpressionSyntax,
    },
    NumericFor {
        name: String,
        first: ExpressionSyntax,
        last: ExpressionSyntax,
        step: Option<ExpressionSyntax>,
        body: Vec<StatementSyntax>,
    },
    GeneralizedFor {
        bindings: Vec<String>,
        iterable: ExpressionSyntax,
        body: Vec<StatementSyntax>,
    },
    Break,
    Continue,
    Match {
        scrutinee: ExpressionSyntax,
        arms: Vec<MatchArmSyntax>,
    },
    Defer {
        body: Vec<StatementSyntax>,
    },
    AsyncDefer {
        body: Vec<StatementSyntax>,
    },
    Assignment {
        target: ExpressionSyntax,
        operator: Option<BinaryOperator>,
        value: ExpressionSyntax,
    },
    MultipleAssignment {
        targets: Vec<ExpressionSyntax>,
        values: Vec<ExpressionSyntax>,
    },
    Expression(ExpressionSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalBindingSyntax {
    name: String,
    annotation: Option<TypeSyntax>,
    span: SourceSpan,
}

impl LocalBindingSyntax {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn annotation(&self) -> Option<&TypeSyntax> {
        self.annotation.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
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
    Float(String),
    String(String),
    InterpolatedString(Vec<StringSegmentSyntax>),
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
    OptionalPropagate {
        operand: Box<ExpressionSyntax>,
    },
    ResultPropagate {
        operand: Box<ExpressionSyntax>,
    },
    Await {
        operand: Box<ExpressionSyntax>,
    },
    Binary {
        operator: BinaryOperator,
        left: Box<ExpressionSyntax>,
        right: Box<ExpressionSyntax>,
    },
    Conditional {
        condition: Box<ExpressionSyntax>,
        when_true: Box<ExpressionSyntax>,
        when_false: Box<ExpressionSyntax>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StringSegmentSyntax {
    pub(crate) kind: StringSegmentSyntaxKind,
    pub(crate) span: SourceSpan,
}

impl StringSegmentSyntax {
    #[must_use]
    pub const fn kind(&self) -> &StringSegmentSyntaxKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StringSegmentSyntaxKind {
    Text(String),
    Expression(ExpressionSyntax),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CaptureFunctionSyntax {
    pub(crate) is_async: bool,
    pub(crate) parameters: Vec<CaptureFunctionParameterSyntax>,
    pub(crate) results: Vec<TypeSyntax>,
    pub(crate) body: Vec<StatementSyntax>,
    pub(crate) span: SourceSpan,
}

impl CaptureFunctionSyntax {
    #[must_use]
    pub const fn is_async(&self) -> bool {
        self.is_async
    }

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
    pub(crate) name: String,
    pub(crate) parameter_type: TypeSyntax,
    pub(crate) span: SourceSpan,
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
    pub(crate) name: String,
    pub(crate) value: ExpressionSyntax,
    pub(crate) span: SourceSpan,
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
    OptionalDefault,
    And,
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Concat,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FunctionBodyError {
    pub(crate) span: SourceSpan,
    pub(crate) expectation: &'static str,
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

enum AssignmentOperator {
    Assign,
    Compound(BinaryOperator),
}

pub(crate) struct BodyParser<'source> {
    pub(crate) source: &'source SourceFile,
    pub(crate) file: FileId,
    pub(crate) node: &'source SyntaxNode,
    pub(crate) boundary_end: TextSize,
    pub(crate) tokens: Vec<Token>,
    pub(crate) position: usize,
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
            Some(TokenKind::Repeat) => self.parse_repeat_until(),
            Some(TokenKind::For) => self.parse_for(),
            Some(TokenKind::Break) => self.parse_loop_control(StatementSyntaxKind::Break),
            Some(TokenKind::Continue) => self.parse_loop_control(StatementSyntaxKind::Continue),
            Some(TokenKind::Match) => self.parse_match(),
            Some(TokenKind::Defer) => self.parse_defer(),
            Some(TokenKind::Async) if self.peek_kind() == Some(TokenKind::Defer) => {
                self.parse_async_defer()
            }
            _ => {
                let expression = self.parse_expression(0)?;
                if self.current_kind() == Some(TokenKind::Comma) {
                    let mut targets = vec![expression];
                    while self.consume(TokenKind::Comma).is_some() {
                        targets.push(self.parse_expression(0)?);
                    }
                    self.expect(TokenKind::Equal, "`=`")?;
                    let values = self.parse_expression_list()?;
                    let span = SourceSpan::new(
                        self.file,
                        ordered_range(
                            targets[0].span().range().start(),
                            values
                                .last()
                                .expect("nonempty value list")
                                .span()
                                .range()
                                .end(),
                        ),
                    );
                    return Ok(StatementSyntax {
                        kind: StatementSyntaxKind::MultipleAssignment { targets, values },
                        span,
                    });
                }
                if let Some(operator) = self.consume_assignment_operator() {
                    let operator = match operator {
                        AssignmentOperator::Assign => None,
                        AssignmentOperator::Compound(operator) => Some(operator),
                    };
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
                            operator,
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

    fn consume_assignment_operator(&mut self) -> Option<AssignmentOperator> {
        let operator = match self.current_kind()? {
            TokenKind::Equal => AssignmentOperator::Assign,
            TokenKind::PlusEqual => AssignmentOperator::Compound(BinaryOperator::Add),
            TokenKind::MinusEqual => AssignmentOperator::Compound(BinaryOperator::Subtract),
            TokenKind::StarEqual => AssignmentOperator::Compound(BinaryOperator::Multiply),
            TokenKind::SlashEqual => AssignmentOperator::Compound(BinaryOperator::Divide),
            TokenKind::PercentEqual => AssignmentOperator::Compound(BinaryOperator::Remainder),
            TokenKind::DotDotEqual => AssignmentOperator::Compound(BinaryOperator::Concat),
            _ => return None,
        };
        self.position = self.position.saturating_add(1);
        Some(operator)
    }

    fn parse_defer(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::Defer, "`defer`")?.range().start();
        self.expect(TokenKind::Newline, "line break after `defer`")?;
        let body = self.parse_statement_list(&[TokenKind::End])?;
        let end = self.expect(TokenKind::End, "cleanup `end`")?.range().end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::Defer { body },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_async_defer(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::Async, "`async`")?.range().start();
        self.expect(TokenKind::Defer, "`defer`")?;
        self.expect(TokenKind::Newline, "line break after `async defer`")?;
        let body = self.parse_statement_list(&[TokenKind::End])?;
        let end = self
            .expect(TokenKind::End, "async cleanup `end`")?
            .range()
            .end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::AsyncDefer { body },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    pub(crate) fn parse_statement_list(
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
        let async_token = self.consume(TokenKind::Async);
        if let Some(function_token) = self.consume(TokenKind::Function) {
            let name = self.expect(TokenKind::Identifier, "local function name")?;
            let name = name.text(self.source).to_owned();
            let function = self.parse_capture_function(
                async_token.is_some(),
                async_token.map_or(function_token.range().start(), |token| {
                    token.range().start()
                }),
            )?;
            let end = function.span().range().end();
            return Ok(StatementSyntax {
                kind: StatementSyntaxKind::LocalFunction { name, function },
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            });
        }
        if async_token.is_some() {
            return Err(self.error("local name"));
        }
        let mut bindings = Vec::new();
        loop {
            let name_token = self.expect(TokenKind::Identifier, "local name")?;
            let name = name_token.text(self.source).to_owned();
            let annotation = if self.consume(TokenKind::Colon).is_some() {
                Some(
                    parse_type_prefix(self.source, self.node, &self.tokens, &mut self.position)
                        .map_err(|error| FunctionBodyError {
                            span: error.span(),
                            expectation: error.expectation(),
                        })?,
                )
            } else {
                None
            };
            let end = annotation
                .as_ref()
                .map_or(name_token.range().end(), |annotation| {
                    annotation.span().range().end()
                });
            bindings.push(LocalBindingSyntax {
                name,
                annotation,
                span: SourceSpan::new(self.file, ordered_range(name_token.range().start(), end)),
            });
            if self.consume(TokenKind::Comma).is_none() {
                break;
            }
        }
        self.expect(TokenKind::Equal, "`=`")?;
        let mut values = self.parse_expression_list()?;
        let end = values
            .last()
            .expect("nonempty value list")
            .span()
            .range()
            .end();
        if bindings.len() > 1 || values.len() > 1 {
            return Ok(StatementSyntax {
                kind: StatementSyntaxKind::MultipleLocal { bindings, values },
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            });
        }
        let binding = bindings.pop().expect("one binding");
        let initializer = values.pop().expect("one initializer");
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::Local {
                name: binding.name,
                annotation: binding.annotation,
                initializer,
            },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_expression_list(&mut self) -> Result<Vec<ExpressionSyntax>, FunctionBodyError> {
        let mut values = vec![self.parse_expression(0)?];
        while self.consume(TokenKind::Comma).is_some() {
            values.push(self.parse_expression(0)?);
        }
        Ok(values)
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
        if self.consume(TokenKind::Local).is_some() {
            let (name, initializer) = self.parse_optional_binding()?;
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
            return Ok(StatementSyntax {
                kind: StatementSyntaxKind::OptionalIf {
                    name,
                    initializer,
                    then_body,
                    else_body,
                },
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            });
        }
        let (condition, then_body, else_body) = self.parse_if_parts()?;
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

    fn parse_if_parts(
        &mut self,
    ) -> Result<(ExpressionSyntax, Vec<StatementSyntax>, Vec<StatementSyntax>), FunctionBodyError>
    {
        let condition = self.parse_expression(0)?;
        self.expect(TokenKind::Then, "`then`")?;
        self.expect(TokenKind::Newline, "line break after `then`")?;
        let then_body =
            self.parse_statement_list(&[TokenKind::ElseIf, TokenKind::Else, TokenKind::End])?;
        let else_body = if self.consume(TokenKind::Else).is_some() {
            self.expect(TokenKind::Newline, "line break after `else`")?;
            self.parse_statement_list(&[TokenKind::End])?
        } else if let Some(elseif) = self.consume(TokenKind::ElseIf) {
            let start = elseif.range().start();
            let (condition, then_body, else_body) = self.parse_if_parts()?;
            let end = else_body
                .last()
                .or_else(|| then_body.last())
                .map_or(condition.span().range().end(), |statement| {
                    statement.span().range().end()
                });
            vec![StatementSyntax {
                kind: StatementSyntaxKind::If {
                    condition,
                    then_body,
                    else_body,
                },
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            }]
        } else {
            Vec::new()
        };
        Ok((condition, then_body, else_body))
    }

    fn parse_while(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::While, "`while`")?.range().start();
        if self.consume(TokenKind::Local).is_some() {
            let (name, initializer) = self.parse_optional_binding()?;
            self.expect(TokenKind::Do, "`do`")?;
            self.expect(TokenKind::Newline, "line break after `do`")?;
            let body = self.parse_statement_list(&[TokenKind::End])?;
            let end = self.expect(TokenKind::End, "`end`")?.range().end();
            return Ok(StatementSyntax {
                kind: StatementSyntaxKind::OptionalWhile {
                    name,
                    initializer,
                    body,
                },
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            });
        }
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

    fn parse_optional_binding(&mut self) -> Result<(String, ExpressionSyntax), FunctionBodyError> {
        let name = self.expect(TokenKind::Identifier, "optional binding name")?;
        let name = name.text(self.source).to_owned();
        self.expect(TokenKind::Equal, "`=` in optional binding")?;
        let initializer = self.parse_expression(0)?;
        Ok((name, initializer))
    }

    fn parse_repeat_until(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::Repeat, "`repeat`")?.range().start();
        self.expect(TokenKind::Newline, "line break after `repeat`")?;
        let body = self.parse_statement_list(&[TokenKind::Until, TokenKind::End])?;
        self.expect(TokenKind::Until, "`until`")?;
        let condition = self.parse_expression(0)?;
        let end = condition.span().range().end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::RepeatUntil { body, condition },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_for(&mut self) -> Result<StatementSyntax, FunctionBodyError> {
        let start = self.expect(TokenKind::For, "`for`")?.range().start();
        let first_binding = self.expect(TokenKind::Identifier, "`for` binding")?;
        let mut bindings = vec![first_binding.text(self.source).to_owned()];
        while self.consume(TokenKind::Comma).is_some() {
            let binding = self.expect(TokenKind::Identifier, "`for` binding after `,`")?;
            bindings.push(binding.text(self.source).to_owned());
        }
        if self.consume(TokenKind::Equal).is_none() {
            self.expect(TokenKind::In, "`in` after generalized `for` bindings")?;
            let iterable = self.parse_expression(0)?;
            self.expect(TokenKind::Do, "`do` after generalized `for` source")?;
            self.expect(TokenKind::Newline, "line break after `do`")?;
            let body = self.parse_statement_list(&[TokenKind::End])?;
            let end = self
                .expect(TokenKind::End, "generalized `for` `end`")?
                .range()
                .end();
            return Ok(StatementSyntax {
                kind: StatementSyntaxKind::GeneralizedFor {
                    bindings,
                    iterable,
                    body,
                },
                span: SourceSpan::new(self.file, ordered_range(start, end)),
            });
        }
        if bindings.len() != 1 {
            return Err(self.error("one binding before `=` in numeric `for`"));
        }
        let name = bindings.pop().expect("numeric `for` has one binding");
        let first = self.parse_expression(0)?;
        self.expect(TokenKind::Comma, "`,` after numeric `for` first value")?;
        let last = self.parse_expression(0)?;
        let step = if self.consume(TokenKind::Comma).is_some() {
            Some(self.parse_expression(0)?)
        } else {
            None
        };
        self.expect(TokenKind::Do, "`do` after numeric `for` range")?;
        self.expect(TokenKind::Newline, "line break after `do`")?;
        let body = self.parse_statement_list(&[TokenKind::End])?;
        let end = self
            .expect(TokenKind::End, "numeric `for` `end`")?
            .range()
            .end();
        Ok(StatementSyntax {
            kind: StatementSyntaxKind::NumericFor {
                name,
                first,
                last,
                step,
                body,
            },
            span: SourceSpan::new(self.file, ordered_range(start, end)),
        })
    }

    fn parse_loop_control(
        &mut self,
        kind: StatementSyntaxKind,
    ) -> Result<StatementSyntax, FunctionBodyError> {
        let token = self
            .advance()
            .ok_or_else(|| self.error("loop-control statement"))?;
        Ok(StatementSyntax {
            kind,
            span: SourceSpan::new(self.file, token.range()),
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
}

impl BodyParser<'_> {
    pub(crate) fn expression(
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

    pub(crate) fn skip_newlines(&mut self) {
        while self.consume(TokenKind::Newline).is_some() {}
    }

    pub(crate) fn current_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position).map(|token| token.kind())
    }

    pub(crate) fn peek_kind(&self) -> Option<TokenKind> {
        self.tokens.get(self.position + 1).map(|token| token.kind())
    }

    pub(crate) fn advance(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).copied()?;
        self.position += 1;
        Some(token)
    }

    pub(crate) fn consume(&mut self, kind: TokenKind) -> Option<Token> {
        (self.current_kind() == Some(kind))
            .then(|| self.advance())
            .flatten()
    }

    pub(crate) fn expect(
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

    pub(crate) fn error(&self, expectation: &'static str) -> FunctionBodyError {
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

pub(crate) fn ordered_range(start: TextSize, end: TextSize) -> TextRange {
    TextRange::new(start, end).unwrap_or_else(|| TextRange::empty(start))
}
