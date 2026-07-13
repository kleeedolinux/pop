//! Expression grammar for function bodies.
//!
//! This module owns precedence, unary/postfix forms, calls, indexing, aggregate
//! literals, and capture-function expressions. Statement/block ownership remains
//! in `body`; both use the same token cursor and structured syntax errors.

use pop_foundation::{SourceSpan, TextSize};

use crate::body::{
    BinaryOperator, BodyParser, CaptureFunctionParameterSyntax, CaptureFunctionSyntax,
    ExpressionSyntax, ExpressionSyntaxKind, FieldInitializerSyntax, FunctionBodyError,
    StringSegmentSyntax, StringSegmentSyntaxKind, UnaryOperator, ordered_range,
};
use crate::signature::parse_type_prefix;
use crate::{Token, TokenKind, TypeSyntax};

impl BodyParser<'_> {
    pub(crate) fn parse_capture_function(
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

    pub(crate) fn parse_expression(
        &mut self,
        minimum_precedence: u8,
    ) -> Result<ExpressionSyntax, FunctionBodyError> {
        let mut left = self.parse_unary()?;
        while let Some((operator, precedence)) = self.current_binary_operator() {
            if precedence < minimum_precedence {
                break;
            }
            self.position += 1;
            let right_precedence = if matches!(
                operator,
                BinaryOperator::Concat | BinaryOperator::OptionalDefault
            ) {
                precedence
            } else {
                precedence + 1
            };
            let right = self.parse_expression(right_precedence)?;
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
        if let Some(keyword) = self.consume(TokenKind::Try) {
            let operand = self.parse_unary()?;
            let end = operand.span().range().end();
            return Ok(self.expression(
                ExpressionSyntaxKind::ResultPropagate {
                    operand: Box::new(operand),
                },
                keyword.range().start(),
                end,
            ));
        }
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
            if let Some(question) = self.consume(TokenKind::Question) {
                let start = expression.span().range().start();
                expression = self.expression(
                    ExpressionSyntaxKind::OptionalPropagate {
                        operand: Box::new(expression),
                    },
                    start,
                    question.range().end(),
                );
                continue;
            }
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
            TokenKind::Number => {
                let value = token.text(self.source).to_owned();
                let kind = if value.contains(['.', 'e', 'E']) {
                    ExpressionSyntaxKind::Float(value)
                } else {
                    ExpressionSyntaxKind::Integer(value)
                };
                Ok(self.expression(kind, start, end))
            }
            TokenKind::String => {
                let value =
                    crate::decode_string_literal(token.text(self.source)).map_err(|_| {
                        FunctionBodyError::new(
                            SourceSpan::new(self.file, token.range()),
                            "valid string literal",
                        )
                    })?;
                Ok(self.expression(ExpressionSyntaxKind::String(value), start, end))
            }
            TokenKind::InterpolatedString => self.parse_interpolated_string(token),
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
            TokenKind::If => self.parse_conditional_expression(start),
            TokenKind::Identifier | TokenKind::Attribute => self.parse_name(token),
            TokenKind::LeftParenthesis => self.parse_parenthesized(start),
            TokenKind::LeftBrace => self.parse_brace_literal(start),
            _ => Err(FunctionBodyError {
                span: SourceSpan::new(self.file, token.range()),
                expectation: "expression",
            }),
        }
    }

    fn parse_conditional_expression(
        &mut self,
        start: TextSize,
    ) -> Result<ExpressionSyntax, FunctionBodyError> {
        let condition = self.parse_expression(0)?;
        self.expect(TokenKind::Then, "`then` in conditional expression")?;
        let when_true = self.parse_expression(0)?;
        self.expect(TokenKind::Else, "`else` in conditional expression")?;
        let when_false = self.parse_expression(0)?;
        let end = when_false.span().range().end();
        Ok(self.expression(
            ExpressionSyntaxKind::Conditional {
                condition: Box::new(condition),
                when_true: Box::new(when_true),
                when_false: Box::new(when_false),
            },
            start,
            end,
        ))
    }

    fn parse_interpolated_string(
        &self,
        token: Token,
    ) -> Result<ExpressionSyntax, FunctionBodyError> {
        let start = token.range().start().to_usize();
        let end = token.range().end().to_usize();
        let content_start = start + 1;
        let content_end = end.saturating_sub(1);
        let bytes = self.source.text().as_bytes();
        let mut cursor = content_start;
        let mut text_start = content_start;
        let mut segments = Vec::new();
        while cursor < content_end {
            match bytes[cursor] {
                b'\\' => {
                    cursor =
                        crate::string_literal::scan_escape(bytes, cursor, true).map_err(|_| {
                            self.interpolation_error(cursor, cursor + 1, "valid string escape")
                        })?;
                }
                b'{' => {
                    self.push_string_text_segment(&mut segments, text_start, cursor)?;
                    let expression_start = cursor + 1;
                    let expression_end =
                        self.find_interpolation_end(expression_start, content_end)?;
                    let lexed =
                        crate::lexer::lex_range(self.source, expression_start, expression_end);
                    if !lexed.diagnostics().is_empty() {
                        return Err(self.interpolation_error(
                            expression_start,
                            expression_end,
                            "valid interpolation expression",
                        ));
                    }
                    let tokens = lexed
                        .tokens()
                        .iter()
                        .copied()
                        .filter(|token| !token.kind().is_trivia())
                        .collect::<Vec<_>>();
                    let mut parser = BodyParser {
                        source: self.source,
                        file: self.file,
                        node: self.node,
                        boundary_end: TextSize::try_from_usize(expression_end)
                            .unwrap_or(token.range().end()),
                        tokens,
                        position: 0,
                    };
                    let expression = parser.parse_expression(0)?;
                    if parser.current_kind().is_some() {
                        return Err(parser.error("end of interpolation expression"));
                    }
                    segments.push(StringSegmentSyntax {
                        span: expression.span(),
                        kind: StringSegmentSyntaxKind::Expression(expression),
                    });
                    cursor = expression_end + 1;
                    text_start = cursor;
                }
                b'}' => {
                    return Err(self.interpolation_error(cursor, cursor + 1, "escaped `}`"));
                }
                _ => {
                    cursor += self.source.text()[cursor..content_end]
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8);
                }
            }
        }
        self.push_string_text_segment(&mut segments, text_start, content_end)?;
        Ok(self.expression(
            ExpressionSyntaxKind::InterpolatedString(segments),
            token.range().start(),
            token.range().end(),
        ))
    }

    fn push_string_text_segment(
        &self,
        segments: &mut Vec<StringSegmentSyntax>,
        start: usize,
        end: usize,
    ) -> Result<(), FunctionBodyError> {
        if start == end {
            return Ok(());
        }
        let text =
            crate::string_literal::decode_string_contents(&self.source.text()[start..end], true)
                .map_err(|_| self.interpolation_error(start, end, "valid string text"))?;
        segments.push(StringSegmentSyntax {
            kind: StringSegmentSyntaxKind::Text(text),
            span: SourceSpan::new(
                self.file,
                ordered_range(
                    TextSize::try_from_usize(start).unwrap_or(TextSize::from_u32(0)),
                    TextSize::try_from_usize(end).unwrap_or(TextSize::from_u32(0)),
                ),
            ),
        });
        Ok(())
    }

    fn find_interpolation_end(
        &self,
        mut cursor: usize,
        content_end: usize,
    ) -> Result<usize, FunctionBodyError> {
        let bytes = self.source.text().as_bytes();
        let mut depth = 1_u32;
        while cursor < content_end {
            match bytes[cursor] {
                b'\'' | b'"' | b'`' => {
                    let quote = bytes[cursor];
                    cursor += 1;
                    while cursor < content_end && bytes[cursor] != quote {
                        if bytes[cursor] == b'\\' {
                            cursor =
                                crate::string_literal::scan_escape(bytes, cursor, quote == b'`')
                                    .map_err(|_| {
                                        self.interpolation_error(
                                            cursor,
                                            cursor + 1,
                                            "valid string escape",
                                        )
                                    })?;
                        } else {
                            cursor += self.source.text()[cursor..content_end]
                                .chars()
                                .next()
                                .map_or(1, char::len_utf8);
                        }
                    }
                    if cursor >= content_end {
                        return Err(self.interpolation_error(
                            cursor,
                            cursor,
                            "closing string delimiter",
                        ));
                    }
                    cursor += 1;
                }
                b'{' => {
                    depth += 1;
                    cursor += 1;
                }
                b'}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(cursor);
                    }
                    cursor += 1;
                }
                _ => {
                    cursor += self.source.text()[cursor..content_end]
                        .chars()
                        .next()
                        .map_or(1, char::len_utf8);
                }
            }
        }
        Err(self.interpolation_error(content_end, content_end, "closing interpolation `}`"))
    }

    fn interpolation_error(
        &self,
        start: usize,
        end: usize,
        expectation: &'static str,
    ) -> FunctionBodyError {
        FunctionBodyError::new(
            SourceSpan::new(
                self.file,
                ordered_range(
                    TextSize::try_from_usize(start).unwrap_or(TextSize::from_u32(0)),
                    TextSize::try_from_usize(end).unwrap_or(TextSize::from_u32(0)),
                ),
            ),
            expectation,
        )
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
            TokenKind::QuestionQuestion => (BinaryOperator::OptionalDefault, 2),
            TokenKind::And => (BinaryOperator::And, 3),
            TokenKind::EqualEqual => (BinaryOperator::Equal, 4),
            TokenKind::TildeEqual => (BinaryOperator::NotEqual, 4),
            TokenKind::LessThan => (BinaryOperator::LessThan, 4),
            TokenKind::LessThanEqual => (BinaryOperator::LessThanOrEqual, 4),
            TokenKind::GreaterThan => (BinaryOperator::GreaterThan, 4),
            TokenKind::GreaterThanEqual => (BinaryOperator::GreaterThanOrEqual, 4),
            TokenKind::DotDot => (BinaryOperator::Concat, 5),
            TokenKind::Plus => (BinaryOperator::Add, 6),
            TokenKind::Minus => (BinaryOperator::Subtract, 6),
            TokenKind::Star => (BinaryOperator::Multiply, 7),
            TokenKind::Slash => (BinaryOperator::Divide, 7),
            TokenKind::Percent => (BinaryOperator::Remainder, 7),
            _ => return None,
        })
    }
}
