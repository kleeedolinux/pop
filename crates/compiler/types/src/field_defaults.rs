use std::cmp::Ordering;

use pop_diagnostics::{compile_time as compile_time_diagnostics, types as type_diagnostics};
use pop_foundation::{Diagnostic, TypeId};
use pop_syntax::{BinaryOperator, ExpressionSyntax, ExpressionSyntaxKind, UnaryOperator};
use serde::{Deserialize, Serialize};

use crate::{
    FloatKind, FloatValue, IntegerValue, NumericError, PrimitiveType, SemanticType, TypeArena,
};

/// A typed immutable field default retained by semantic analysis.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum FieldDefault {
    Nil,
    Boolean(bool),
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EvaluatedDefault {
    value: FieldDefault,
    type_id: TypeId,
}

pub(crate) fn resolve_field_default(
    arena: &TypeArena,
    field_type: TypeId,
    expression: &ExpressionSyntax,
    declaration_kind: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<FieldDefault> {
    let context = format!("{declaration_kind} field default");
    let numeric_hint = numeric_type_for_expected(arena, field_type);
    let evaluated = evaluate_constant(arena, expression, numeric_hint, &context, diagnostics)?;
    let assignable = field_type == evaluated.type_id
        || matches!(arena.get(field_type), Some(SemanticType::Union(members)) if members.contains(&evaluated.type_id));
    if !assignable {
        diagnostics.push(type_diagnostics::type_mismatch(
            expression.span(),
            format!("type#{}", field_type.raw()),
            format!("type#{}", evaluated.type_id.raw()),
            expression.span(),
        ));
        return None;
    }
    Some(evaluated.value)
}

fn evaluate_constant(
    arena: &TypeArena,
    expression: &ExpressionSyntax,
    numeric_hint: Option<TypeId>,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedDefault> {
    match expression.kind() {
        ExpressionSyntaxKind::Integer(literal) => parse_numeric_constant(
            arena,
            literal,
            numeric_hint,
            expression,
            context,
            diagnostics,
        ),
        ExpressionSyntaxKind::Float(literal) => parse_float_constant(
            arena,
            literal,
            numeric_hint,
            expression,
            context,
            diagnostics,
        ),
        ExpressionSyntaxKind::String(literal) => Some(EvaluatedDefault {
            value: FieldDefault::String(literal.clone()),
            type_id: arena.source_type("String")?,
        }),
        ExpressionSyntaxKind::Boolean(value) => Some(EvaluatedDefault {
            value: FieldDefault::Boolean(*value),
            type_id: arena.source_type("Boolean")?,
        }),
        ExpressionSyntaxKind::Nil => Some(EvaluatedDefault {
            value: FieldDefault::Nil,
            type_id: arena.source_type("nil")?,
        }),
        ExpressionSyntaxKind::Unary { operator, operand } => evaluate_unary(
            arena,
            *operator,
            operand,
            numeric_hint,
            expression,
            context,
            diagnostics,
        ),
        ExpressionSyntaxKind::Binary {
            operator,
            left,
            right,
        } => evaluate_binary(
            arena,
            *operator,
            left,
            right,
            numeric_hint,
            expression,
            context,
            diagnostics,
        ),
        ExpressionSyntaxKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            let condition = evaluate_constant(arena, condition, None, context, diagnostics)?;
            match condition.value {
                FieldDefault::Boolean(true) => {
                    evaluate_constant(arena, when_true, numeric_hint, context, diagnostics)
                }
                FieldDefault::Boolean(false) => {
                    evaluate_constant(arena, when_false, numeric_hint, context, diagnostics)
                }
                value => {
                    invalid_constant_operator(expression, "if", &value, diagnostics);
                    None
                }
            }
        }
        ExpressionSyntaxKind::InterpolatedString(_)
        | ExpressionSyntaxKind::OptionalPropagate { .. }
        | ExpressionSyntaxKind::ResultPropagate { .. }
        | ExpressionSyntaxKind::Await { .. }
        | ExpressionSyntaxKind::Name(_)
        | ExpressionSyntaxKind::Function(_)
        | ExpressionSyntaxKind::Call { .. }
        | ExpressionSyntaxKind::GenericCall { .. }
        | ExpressionSyntaxKind::MethodCall { .. }
        | ExpressionSyntaxKind::Index { .. }
        | ExpressionSyntaxKind::Construct { .. }
        | ExpressionSyntaxKind::Aggregate { .. }
        | ExpressionSyntaxKind::Array(_)
        | ExpressionSyntaxKind::With { .. }
        | ExpressionSyntaxKind::Tuple(_) => {
            diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                expression.span(),
                context,
            ));
            None
        }
    }
}

fn parse_float_constant(
    arena: &TypeArena,
    literal: &str,
    numeric_hint: Option<TypeId>,
    expression: &ExpressionSyntax,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedDefault> {
    let type_id = numeric_hint
        .filter(|type_id| {
            matches!(
                arena.get(*type_id),
                Some(SemanticType::Primitive(
                    PrimitiveType::Float32 | PrimitiveType::Float64
                ))
            )
        })
        .or_else(|| arena.source_type("Float"))?;
    let kind = match arena.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => FloatKind::Float32,
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => FloatKind::Float64,
        _ => return None,
    };
    match FloatValue::parse_decimal(literal, kind) {
        Ok(value) => Some(EvaluatedDefault {
            value: FieldDefault::Float(value),
            type_id,
        }),
        Err(error) => {
            push_numeric_error(error, expression, context, diagnostics);
            None
        }
    }
}

fn parse_numeric_constant(
    arena: &TypeArena,
    literal: &str,
    numeric_hint: Option<TypeId>,
    expression: &ExpressionSyntax,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedDefault> {
    let type_id = numeric_hint.or_else(|| arena.source_type("Int"))?;
    let value = match arena.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
            IntegerValue::parse_decimal(literal, *kind).map(FieldDefault::Integer)
        }
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
            FloatValue::parse_decimal(literal, FloatKind::Float32).map(FieldDefault::Float)
        }
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
            FloatValue::parse_decimal(literal, FloatKind::Float64).map(FieldDefault::Float)
        }
        _ => return None,
    };
    match value {
        Ok(value) => Some(EvaluatedDefault { value, type_id }),
        Err(error) => {
            push_numeric_error(error, expression, context, diagnostics);
            None
        }
    }
}

fn evaluate_unary(
    arena: &TypeArena,
    operator: UnaryOperator,
    operand: &ExpressionSyntax,
    numeric_hint: Option<TypeId>,
    expression: &ExpressionSyntax,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedDefault> {
    if operator == UnaryOperator::Negate
        && let ExpressionSyntaxKind::Integer(literal) = operand.kind()
    {
        return parse_numeric_constant(
            arena,
            &format!("-{literal}"),
            numeric_hint,
            expression,
            context,
            diagnostics,
        );
    }
    let operand = evaluate_constant(arena, operand, numeric_hint, context, diagnostics)?;
    let value = match (operator, operand.value) {
        (UnaryOperator::Negate, FieldDefault::Integer(value)) => {
            value.checked_negate().map(FieldDefault::Integer)
        }
        (UnaryOperator::Negate, FieldDefault::Float(value)) => {
            Ok(FieldDefault::Float(value.negate()))
        }
        (UnaryOperator::Not, FieldDefault::Boolean(value)) => {
            return boolean_constant(arena, !value);
        }
        (operator, value) => {
            invalid_constant_operator(expression, unary_text(operator), &value, diagnostics);
            return None;
        }
    };
    match value {
        Ok(value) => Some(EvaluatedDefault {
            value,
            type_id: operand.type_id,
        }),
        Err(error) => {
            push_numeric_error(error, expression, context, diagnostics);
            None
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn evaluate_binary(
    arena: &TypeArena,
    operator: BinaryOperator,
    left: &ExpressionSyntax,
    right: &ExpressionSyntax,
    numeric_hint: Option<TypeId>,
    expression: &ExpressionSyntax,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedDefault> {
    let operand_hint = matches!(
        operator,
        BinaryOperator::Add
            | BinaryOperator::Subtract
            | BinaryOperator::Multiply
            | BinaryOperator::Divide
            | BinaryOperator::Remainder
            | BinaryOperator::LessThan
            | BinaryOperator::LessThanOrEqual
            | BinaryOperator::GreaterThan
            | BinaryOperator::GreaterThanOrEqual
    )
    .then_some(numeric_hint)
    .flatten();
    let left = evaluate_constant(arena, left, operand_hint, context, diagnostics)?;
    let right = evaluate_constant(arena, right, operand_hint, context, diagnostics)?;
    if left.type_id != right.type_id {
        diagnostics.push(type_diagnostics::type_mismatch(
            expression.span(),
            format!("type#{}", left.type_id.raw()),
            format!("type#{}", right.type_id.raw()),
            expression.span(),
        ));
        return None;
    }
    evaluate_binary_values(
        arena,
        operator,
        left.type_id,
        left.value,
        right.value,
        expression,
        context,
        diagnostics,
    )
}

#[allow(clippy::too_many_arguments)]
fn evaluate_binary_values(
    arena: &TypeArena,
    operator: BinaryOperator,
    operand_type: TypeId,
    left: FieldDefault,
    right: FieldDefault,
    expression: &ExpressionSyntax,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<EvaluatedDefault> {
    let numeric = match (operator, &left, &right) {
        (BinaryOperator::Add, FieldDefault::Integer(left), FieldDefault::Integer(right)) => {
            Some(left.checked_add(*right).map(FieldDefault::Integer))
        }
        (BinaryOperator::Subtract, FieldDefault::Integer(left), FieldDefault::Integer(right)) => {
            Some(left.checked_subtract(*right).map(FieldDefault::Integer))
        }
        (BinaryOperator::Multiply, FieldDefault::Integer(left), FieldDefault::Integer(right)) => {
            Some(left.checked_multiply(*right).map(FieldDefault::Integer))
        }
        (BinaryOperator::Divide, FieldDefault::Integer(left), FieldDefault::Integer(right)) => {
            Some(left.checked_divide(*right).map(FieldDefault::Integer))
        }
        (BinaryOperator::Remainder, FieldDefault::Integer(left), FieldDefault::Integer(right)) => {
            Some(left.checked_remainder(*right).map(FieldDefault::Integer))
        }
        (BinaryOperator::Add, FieldDefault::Float(left), FieldDefault::Float(right)) => {
            Some(left.checked_add(*right).map(FieldDefault::Float))
        }
        (BinaryOperator::Subtract, FieldDefault::Float(left), FieldDefault::Float(right)) => {
            Some(left.checked_subtract(*right).map(FieldDefault::Float))
        }
        (BinaryOperator::Multiply, FieldDefault::Float(left), FieldDefault::Float(right)) => {
            Some(left.checked_multiply(*right).map(FieldDefault::Float))
        }
        (BinaryOperator::Divide, FieldDefault::Float(left), FieldDefault::Float(right)) => {
            Some(left.checked_divide(*right).map(FieldDefault::Float))
        }
        _ => None,
    };
    if let Some(value) = numeric {
        return match value {
            Ok(value) => Some(EvaluatedDefault {
                value,
                type_id: operand_type,
            }),
            Err(error) => {
                push_numeric_error(error, expression, context, diagnostics);
                None
            }
        };
    }
    match (operator, left, right) {
        (BinaryOperator::Concat, FieldDefault::String(left), FieldDefault::String(right)) => {
            Some(EvaluatedDefault {
                value: FieldDefault::String(left + &right),
                type_id: operand_type,
            })
        }
        (BinaryOperator::LessThan, FieldDefault::Integer(left), FieldDefault::Integer(right)) => {
            boolean_constant(arena, left.compare(right).ok()? == Ordering::Less)
        }
        (
            BinaryOperator::LessThanOrEqual,
            FieldDefault::Integer(left),
            FieldDefault::Integer(right),
        ) => boolean_constant(arena, left.compare(right).ok()?.is_le()),
        (
            BinaryOperator::GreaterThan,
            FieldDefault::Integer(left),
            FieldDefault::Integer(right),
        ) => boolean_constant(arena, left.compare(right).ok()? == Ordering::Greater),
        (
            BinaryOperator::GreaterThanOrEqual,
            FieldDefault::Integer(left),
            FieldDefault::Integer(right),
        ) => boolean_constant(arena, left.compare(right).ok()?.is_ge()),
        (BinaryOperator::LessThan, FieldDefault::Float(left), FieldDefault::Float(right)) => {
            boolean_constant(
                arena,
                left.partial_compare(right)
                    .ok()?
                    .is_some_and(Ordering::is_lt),
            )
        }
        (
            BinaryOperator::LessThanOrEqual,
            FieldDefault::Float(left),
            FieldDefault::Float(right),
        ) => boolean_constant(
            arena,
            left.partial_compare(right)
                .ok()?
                .is_some_and(Ordering::is_le),
        ),
        (BinaryOperator::GreaterThan, FieldDefault::Float(left), FieldDefault::Float(right)) => {
            boolean_constant(
                arena,
                left.partial_compare(right)
                    .ok()?
                    .is_some_and(Ordering::is_gt),
            )
        }
        (
            BinaryOperator::GreaterThanOrEqual,
            FieldDefault::Float(left),
            FieldDefault::Float(right),
        ) => boolean_constant(
            arena,
            left.partial_compare(right)
                .ok()?
                .is_some_and(Ordering::is_ge),
        ),
        (BinaryOperator::And, FieldDefault::Boolean(left), FieldDefault::Boolean(right)) => {
            boolean_constant(arena, left && right)
        }
        (BinaryOperator::Or, FieldDefault::Boolean(left), FieldDefault::Boolean(right)) => {
            boolean_constant(arena, left || right)
        }
        (BinaryOperator::Equal, left, right)
            if !matches!(left, FieldDefault::Float(_))
                && !matches!(right, FieldDefault::Float(_)) =>
        {
            boolean_constant(arena, left == right)
        }
        (BinaryOperator::NotEqual, left, right)
            if !matches!(left, FieldDefault::Float(_))
                && !matches!(right, FieldDefault::Float(_)) =>
        {
            boolean_constant(arena, left != right)
        }
        (operator, left, right) => {
            diagnostics.push(type_diagnostics::invalid_operator(
                expression.span(),
                binary_text(operator),
                format!("{} and {}", default_name(&left), default_name(&right)),
            ));
            None
        }
    }
}

fn push_numeric_error(
    error: NumericError,
    expression: &ExpressionSyntax,
    context: &str,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let diagnostic = match error {
        NumericError::DivisionByZero => {
            compile_time_diagnostics::constant_division_by_zero(expression.span(), context)
        }
        NumericError::InvalidLiteral
        | NumericError::OutOfRange
        | NumericError::Overflow
        | NumericError::KindMismatch => {
            compile_time_diagnostics::constant_integer_overflow(expression.span(), context)
        }
    };
    diagnostics.push(diagnostic);
}

fn boolean_constant(arena: &TypeArena, value: bool) -> Option<EvaluatedDefault> {
    Some(EvaluatedDefault {
        value: FieldDefault::Boolean(value),
        type_id: arena.source_type("Boolean")?,
    })
}

fn numeric_type_for_expected(arena: &TypeArena, expected: TypeId) -> Option<TypeId> {
    if is_numeric(arena, expected) {
        return Some(expected);
    }
    let Some(SemanticType::Union(members)) = arena.get(expected) else {
        return None;
    };
    let mut numerics = members
        .iter()
        .copied()
        .filter(|member| is_numeric(arena, *member));
    let numeric = numerics.next()?;
    numerics.next().is_none().then_some(numeric)
}

fn is_numeric(arena: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        arena.get(type_id),
        Some(SemanticType::Primitive(
            PrimitiveType::Integer(_) | PrimitiveType::Float32 | PrimitiveType::Float64
        ))
    )
}

fn invalid_constant_operator(
    expression: &ExpressionSyntax,
    operator: &str,
    value: &FieldDefault,
    diagnostics: &mut Vec<Diagnostic>,
) {
    diagnostics.push(type_diagnostics::invalid_operator(
        expression.span(),
        operator,
        default_name(value),
    ));
}

const fn unary_text(operator: UnaryOperator) -> &'static str {
    match operator {
        UnaryOperator::Not => "not",
        UnaryOperator::Negate => "-",
    }
}

const fn binary_text(operator: BinaryOperator) -> &'static str {
    match operator {
        BinaryOperator::Or => "or",
        BinaryOperator::OptionalDefault => "??",
        BinaryOperator::And => "and",
        BinaryOperator::Equal => "==",
        BinaryOperator::NotEqual => "~=",
        BinaryOperator::LessThan => "<",
        BinaryOperator::LessThanOrEqual => "<=",
        BinaryOperator::GreaterThan => ">",
        BinaryOperator::GreaterThanOrEqual => ">=",
        BinaryOperator::Concat => "..",
        BinaryOperator::Add => "+",
        BinaryOperator::Subtract => "-",
        BinaryOperator::Multiply => "*",
        BinaryOperator::Divide => "/",
        BinaryOperator::Remainder => "%",
    }
}

const fn default_name(value: &FieldDefault) -> &'static str {
    match value {
        FieldDefault::Nil => "nil",
        FieldDefault::Boolean(_) => "Boolean",
        FieldDefault::Integer(_) => "integer",
        FieldDefault::Float(_) => "float",
        FieldDefault::String(_) => "String",
    }
}
