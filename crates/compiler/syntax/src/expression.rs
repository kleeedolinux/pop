//! Expression grammar for function bodies.
//!
//! This module owns precedence, unary/postfix forms, calls, indexing, aggregate
//! literals, and capture-function expressions. Statement/block ownership remains
//! in `body`; both use the same token cursor and structured syntax errors.

use pop_foundation::{SourceSpan, TextSize};

use crate::body::{
    BinaryOperator, BodyParser, CaptureFunctionParameterSyntax, CaptureFunctionSyntax,
    ExpressionSyntax, ExpressionSyntaxKind, FieldInitializerSyntax, FunctionBodyError,
    UnaryOperator, ordered_range,
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
}
