//! Unary, binary, numeric, equality, and class-member access checking.
//!
//! Operator selection is closed and type-directed. Invalid operands produce
//! structured diagnostics; they never become runtime dynamic operations.

use pop_diagnostics::types as type_diagnostics;
use pop_foundation::{SourceSpan, TypeId};
use pop_syntax::{
    BinaryOperator as SyntaxBinaryOperator, ExpressionSyntax, ExpressionSyntaxKind,
    UnaryOperator as SyntaxUnaryOperator,
};

use crate::body_checking::{
    BodyChecker, ExpectedExpressionType, NumericTarget, binary_text, primitive_name, typed_binary,
    typed_unary, unary_text,
};
use crate::typed_body::*;
use crate::{FloatKind, PrimitiveType, SemanticType};

impl<'resolver, 'index> BodyChecker<'resolver, 'index> {
    pub(crate) fn check_unary(
        &mut self,
        operator: SyntaxUnaryOperator,
        operand: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if operator == SyntaxUnaryOperator::Negate
            && let ExpressionSyntaxKind::Integer(value) = operand.kind()
        {
            return self.numeric_literal_expression(
                value,
                expected.map(|expected| expected.type_id),
                true,
                span,
            );
        }
        let operand_expectation = match operator {
            SyntaxUnaryOperator::Not => self
                .resolver
                .arena()
                .source_type("Boolean")
                .map(ExpectedExpressionType::plain),
            SyntaxUnaryOperator::Negate => {
                expected.filter(|expected| self.is_numeric(expected.type_id))
            }
        };
        let operand = self.check_expression_expected(operand, operand_expectation)?;
        let valid = match operator {
            SyntaxUnaryOperator::Not => {
                self.is_primitive(operand.type_id(), PrimitiveType::Boolean)
            }
            SyntaxUnaryOperator::Negate => self.is_negatable_numeric(operand.type_id()),
        };
        if !valid {
            self.invalid_operator(span, unary_text(operator), &[operand.type_id()]);
            return None;
        }
        let type_id = match operator {
            SyntaxUnaryOperator::Not => self.resolver.arena().source_type("Boolean")?,
            SyntaxUnaryOperator::Negate => operand.type_id(),
        };
        Some(TypedExpression {
            kind: TypedExpressionKind::Unary {
                operator: typed_unary(operator),
                operand: Box::new(operand),
            },
            type_id,
            span,
        })
    }

    pub(crate) fn check_binary(
        &mut self,
        operator: SyntaxBinaryOperator,
        left: &ExpressionSyntax,
        right: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if operator == SyntaxBinaryOperator::OptionalDefault {
            let optional = self.check_expression(left)?;
            let Some(inner_type) = self.optional_inner(optional.type_id()) else {
                self.invalid_operator(span, "??", &[optional.type_id()]);
                return None;
            };
            let fallback = self.check_expression_expected(
                right,
                Some(ExpectedExpressionType::plain(inner_type)),
            )?;
            self.require_same_type(
                inner_type,
                fallback.type_id(),
                fallback.span(),
                right.span(),
            );
            return Some(TypedExpression {
                kind: TypedExpressionKind::OptionalDefault {
                    optional: Box::new(optional),
                    fallback: Box::new(fallback),
                },
                type_id: inner_type,
                span,
            });
        }
        let (left, right) = self.check_binary_operands(operator, left, right, expected)?;
        let operands_match = left.type_id() == right.type_id();
        let valid = match operator {
            SyntaxBinaryOperator::OptionalDefault => unreachable!("handled above"),
            SyntaxBinaryOperator::Or | SyntaxBinaryOperator::And => {
                operands_match && self.is_primitive(left.type_id(), PrimitiveType::Boolean)
            }
            SyntaxBinaryOperator::Concat => {
                operands_match && self.is_primitive(left.type_id(), PrimitiveType::String)
            }
            SyntaxBinaryOperator::Equal | SyntaxBinaryOperator::NotEqual => {
                self.equality_comparable(left.type_id(), right.type_id())
            }
            SyntaxBinaryOperator::LessThan
            | SyntaxBinaryOperator::LessThanOrEqual
            | SyntaxBinaryOperator::GreaterThan
            | SyntaxBinaryOperator::GreaterThanOrEqual => {
                operands_match && self.is_numeric(left.type_id())
            }
            SyntaxBinaryOperator::Add
            | SyntaxBinaryOperator::Subtract
            | SyntaxBinaryOperator::Multiply
            | SyntaxBinaryOperator::Divide => operands_match && self.is_numeric(left.type_id()),
            SyntaxBinaryOperator::Remainder => operands_match && self.is_integer(left.type_id()),
        };
        if !valid {
            self.invalid_operator(
                span,
                binary_text(operator),
                &[left.type_id(), right.type_id()],
            );
            return None;
        }
        let type_id = match operator {
            SyntaxBinaryOperator::Equal
            | SyntaxBinaryOperator::NotEqual
            | SyntaxBinaryOperator::LessThan
            | SyntaxBinaryOperator::LessThanOrEqual
            | SyntaxBinaryOperator::GreaterThan => self.resolver.arena().source_type("Boolean")?,
            SyntaxBinaryOperator::GreaterThanOrEqual => {
                self.resolver.arena().source_type("Boolean")?
            }
            _ => left.type_id(),
        };
        let kind = if operator == SyntaxBinaryOperator::Concat {
            TypedExpressionKind::StringConcat {
                left: Box::new(left),
                right: Box::new(right),
            }
        } else {
            TypedExpressionKind::Binary {
                operator: typed_binary(operator),
                left: Box::new(left),
                right: Box::new(right),
            }
        };
        Some(TypedExpression {
            kind,
            type_id,
            span,
        })
    }

    pub(crate) fn check_binary_operands(
        &mut self,
        operator: SyntaxBinaryOperator,
        left: &ExpressionSyntax,
        right: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
    ) -> Option<(TypedExpression, TypedExpression)> {
        if matches!(
            operator,
            SyntaxBinaryOperator::And | SyntaxBinaryOperator::Or
        ) {
            let boolean = self
                .resolver
                .arena()
                .source_type("Boolean")
                .map(ExpectedExpressionType::plain);
            return Some((
                self.check_expression_expected(left, boolean)?,
                self.check_expression_expected(right, boolean)?,
            ));
        }
        if operator == SyntaxBinaryOperator::Concat {
            return Some((self.check_expression(left)?, self.check_expression(right)?));
        }
        let arithmetic = matches!(
            operator,
            SyntaxBinaryOperator::Add
                | SyntaxBinaryOperator::Subtract
                | SyntaxBinaryOperator::Multiply
                | SyntaxBinaryOperator::Divide
                | SyntaxBinaryOperator::Remainder
        );
        let outer_numeric = arithmetic
            .then_some(expected)
            .flatten()
            .filter(|expected| self.is_numeric(expected.type_id));
        if let Some(expected) = outer_numeric {
            return Some((
                self.check_expression_expected(left, Some(expected))?,
                self.check_expression_expected(right, Some(expected))?,
            ));
        }
        if matches!(left.kind(), ExpressionSyntaxKind::Integer(_))
            && !matches!(right.kind(), ExpressionSyntaxKind::Integer(_))
        {
            let right = self.check_expression(right)?;
            let expectation = self
                .is_numeric(right.type_id())
                .then_some(ExpectedExpressionType::plain(right.type_id()));
            let left = self.check_expression_expected(left, expectation)?;
            return Some((left, right));
        }
        let left = self.check_expression(left)?;
        let expectation = self
            .is_numeric(left.type_id())
            .then_some(ExpectedExpressionType::plain(left.type_id()));
        let right = self.check_expression_expected(right, expectation)?;
        Some((left, right))
    }

    pub(crate) fn require_same_type(
        &mut self,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
        expected_origin: SourceSpan,
    ) {
        if expected != found {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                span,
                self.type_name(expected),
                self.type_name(found),
                expected_origin,
            ));
        }
    }

    pub(crate) fn invalid_operator(
        &mut self,
        span: SourceSpan,
        operator: &str,
        operands: &[TypeId],
    ) {
        let operands = operands
            .iter()
            .map(|type_id| self.type_name(*type_id))
            .collect::<Vec<_>>()
            .join(", ");
        self.diagnostics
            .push(type_diagnostics::invalid_operator(span, operator, operands));
    }

    pub(crate) fn is_numeric(&self, type_id: TypeId) -> bool {
        self.numeric_target(type_id).is_some()
    }

    pub(crate) fn is_integer(&self, type_id: TypeId) -> bool {
        matches!(
            self.resolver.arena().get(type_id),
            Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
        )
    }

    pub(crate) fn is_negatable_numeric(&self, type_id: TypeId) -> bool {
        matches!(
            self.numeric_target(type_id),
            Some(NumericTarget::Integer(kind)) if kind.is_signed()
        ) || matches!(self.numeric_target(type_id), Some(NumericTarget::Float(_)))
    }

    pub(crate) fn numeric_target(&self, type_id: TypeId) -> Option<NumericTarget> {
        match self.resolver.arena().get(type_id) {
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
                Some(NumericTarget::Integer(*kind))
            }
            Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
                Some(NumericTarget::Float(FloatKind::Float32))
            }
            Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
                Some(NumericTarget::Float(FloatKind::Float64))
            }
            _ => None,
        }
    }

    pub(crate) fn equality_comparable(&self, left: TypeId, right: TypeId) -> bool {
        if left == right {
            return self.supports_default_equality(left);
        }
        let Some(nil) = self.resolver.arena().source_type("nil") else {
            return false;
        };
        let optional = if left == nil {
            right
        } else if right == nil {
            left
        } else {
            return false;
        };
        matches!(
            self.resolver.arena().get(optional),
            Some(SemanticType::Union(members)) if members.contains(&nil)
        )
    }

    pub(crate) fn supports_default_equality(&self, type_id: TypeId) -> bool {
        match self.resolver.arena().get(type_id) {
            Some(
                SemanticType::Primitive(
                    PrimitiveType::Nil
                    | PrimitiveType::Boolean
                    | PrimitiveType::Integer(_)
                    | PrimitiveType::String,
                )
                | SemanticType::Class { .. }
                | SemanticType::Enum { .. },
            ) => true,
            Some(SemanticType::Tuple(elements) | SemanticType::Union(elements)) => elements
                .iter()
                .all(|element| self.supports_default_equality(*element)),
            Some(SemanticType::Record(fields)) => fields
                .iter()
                .all(|(_, field_type)| self.supports_default_equality(*field_type)),
            _ => false,
        }
    }

    pub(crate) fn can_access_class_member(
        &self,
        definition: &crate::ClassDefinition,
        visibility: pop_resolve::Visibility,
    ) -> bool {
        match visibility {
            pop_resolve::Visibility::Public => true,
            pop_resolve::Visibility::Private => self.module == definition.module(),
            pop_resolve::Visibility::Internal => self
                .resolver
                .database()
                .index()
                .module(self.module)
                .is_some_and(|module| module.bubble() == definition.bubble()),
        }
    }

    pub(crate) fn is_primitive(&self, type_id: TypeId, primitive: PrimitiveType) -> bool {
        self.resolver.arena().get(type_id) == Some(&SemanticType::Primitive(primitive))
    }

    pub(crate) fn type_name(&self, type_id: TypeId) -> String {
        match self.resolver.arena().get(type_id) {
            Some(SemanticType::Primitive(primitive)) => primitive_name(*primitive).to_owned(),
            Some(SemanticType::Tuple(elements)) => format!("tuple/{}", elements.len()),
            Some(SemanticType::Function { .. }) => "function".to_owned(),
            Some(_) => format!("type#{}", type_id.raw()),
            None => format!("invalid-type#{}", type_id.raw()),
        }
    }
}
