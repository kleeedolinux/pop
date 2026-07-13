//! Lowering from accepted typed bodies into restricted compile-time HIR.
//!
//! Unsupported runtime constructs fail closed here. Lowering never parses
//! source, invokes a backend, or turns inference failure into dynamic work.

use pop_foundation::{FunctionId, SourceSpan, TypeId};
use pop_types::{
    ResolvedFunctionSignature, TypeArena, TypedBinaryOperator, TypedBody, TypedCallDispatch,
    TypedExpression, TypedExpressionKind, TypedStatement, TypedStatementKind, TypedUnaryOperator,
};

use crate::model::*;
use crate::program::{is_float_type, is_integer_type};

/// Lowers one already-typed required-constant expression into restricted
/// compile-time HIR.
///
/// This entry point has no parameter environment. Parameter and local reads are
/// rejected, while direct calls remain explicit and are checked when the
/// resulting expressions are assembled into a [`CompileTimeProgram`].
///
/// # Errors
///
/// Returns a typed restriction or invariant error for constructs outside the
/// accepted deterministic subset.
pub fn lower_compile_time_expression(
    expression: &TypedExpression,
    types: &TypeArena,
) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
    TypedCompileTimeLowerer {
        types,
        parameter_types: &[],
    }
    .lower_expression(expression)
}

/// Lowers one accepted typed function body into restricted compile-time HIR.
///
/// The initial boundary supports exactly one result and a single deterministic
/// result expression, including nested `if` statements whose two branches each
/// produce exactly one value. State, loops, mutation, receiver dispatch, and
/// indirect calls are rejected explicitly.
///
/// # Errors
///
/// Returns a typed restriction, arity, canonical-type, or type-consistency
/// error. It never invents behavior for an unsupported body.
pub fn lower_compile_time_function(
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    types: &TypeArena,
) -> Result<CompileTimeFunction, CompileTimeLoweringError> {
    let parameter_types: Result<Vec<_>, _> = signature
        .parameters()
        .iter()
        .map(|parameter| {
            parameter.parameter_type().type_id().ok_or(
                CompileTimeLoweringError::MissingCanonicalType {
                    span: parameter.span(),
                },
            )
        })
        .collect();
    let parameter_types = parameter_types?;
    if signature.results().len() != 1 {
        return Err(CompileTimeLoweringError::UnsupportedResultArity {
            found: signature.results().len(),
        });
    }
    let result =
        signature.results()[0]
            .type_id()
            .ok_or(CompileTimeLoweringError::MissingCanonicalType {
                span: signature.results()[0].span(),
            })?;
    let lowerer = TypedCompileTimeLowerer {
        types,
        parameter_types: &parameter_types,
    };
    let body = lowerer.lower_statements(body.statements())?;
    if body.type_id() != result {
        return Err(CompileTimeLoweringError::TypeMismatch {
            expected: result,
            found: body.type_id(),
            span: body.span(),
        });
    }
    Ok(CompileTimeFunction::new(
        FunctionId::from_raw(signature.symbol().raw()),
        parameter_types,
        result,
        body,
    ))
}

struct TypedCompileTimeLowerer<'types> {
    types: &'types TypeArena,
    parameter_types: &'types [TypeId],
}

impl TypedCompileTimeLowerer<'_> {
    fn lower_statements(
        &self,
        statements: &[TypedStatement],
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let Some((first, rest)) = statements.split_first() else {
            return Err(CompileTimeLoweringError::BodyDoesNotProduceSingleResult { span: None });
        };
        if let TypedStatementKind::Local {
            local,
            local_type,
            initializer,
            ..
        } = first.kind()
        {
            let initializer = self.lower_expression(initializer)?;
            if initializer.type_id() != *local_type {
                return Err(CompileTimeLoweringError::TypeMismatch {
                    expected: *local_type,
                    found: initializer.type_id(),
                    span: initializer.span(),
                });
            }
            let body = self.lower_statements(rest)?;
            return Ok(CompileTimeExpression::let_binding(
                *local,
                *local_type,
                initializer,
                body,
                first.span(),
            ));
        }
        if let TypedStatementKind::MultipleLocal { bindings, value } = first.kind() {
            let initializer = self.lower_expression(value)?;
            let body = self.lower_statements(rest)?;
            return Ok(CompileTimeExpression::let_tuple(
                bindings
                    .iter()
                    .map(|binding| (binding.local(), binding.local_type()))
                    .collect(),
                initializer,
                body,
                first.span(),
            ));
        }
        if rest.is_empty() {
            return self.lower_result_statement(first);
        }
        if let Some(error) = unsupported_statement_error(first) {
            return Err(error);
        }
        Err(CompileTimeLoweringError::BodyDoesNotProduceSingleResult {
            span: Some(first.span()),
        })
    }

    fn lower_result_statement(
        &self,
        statement: &TypedStatement,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        match statement.kind() {
            TypedStatementKind::Return { values } => {
                if values.len() != 1 {
                    return Err(CompileTimeLoweringError::UnsupportedReturnArity {
                        found: values.len(),
                        span: statement.span(),
                    });
                }
                self.lower_expression(&values[0])
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = self.lower_expression(condition)?;
                let when_true = self.lower_statements(then_body)?;
                let when_false = self.lower_statements(else_body)?;
                if when_true.type_id() != when_false.type_id() {
                    return Err(CompileTimeLoweringError::TypeMismatch {
                        expected: when_true.type_id(),
                        found: when_false.type_id(),
                        span: when_false.span(),
                    });
                }
                Ok(CompileTimeExpression::conditional(
                    condition,
                    when_true.clone(),
                    when_false,
                    when_true.type_id(),
                    statement.span(),
                ))
            }
            _ => Err(unsupported_statement_error(statement).unwrap_or(
                CompileTimeLoweringError::BodyDoesNotProduceSingleResult {
                    span: Some(statement.span()),
                },
            )),
        }
    }

    fn lower_expression(
        &self,
        expression: &TypedExpression,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let type_id = expression.type_id();
        let span = expression.span();
        match expression.kind() {
            TypedExpressionKind::Integer(_)
            | TypedExpressionKind::Float(_)
            | TypedExpressionKind::String(_)
            | TypedExpressionKind::Boolean(_)
            | TypedExpressionKind::Nil => Ok(lower_compile_time_literal(expression)),
            TypedExpressionKind::Parameter(parameter) => {
                self.lower_parameter(parameter.raw(), type_id, span)
            }
            TypedExpressionKind::Tuple(elements) => self.lower_tuple(elements, type_id, span),
            TypedExpressionKind::TupleGet { tuple, index } => Ok(CompileTimeExpression::tuple_get(
                self.lower_expression(tuple)?,
                *index,
                type_id,
                span,
            )),
            TypedExpressionKind::Unary { operator, operand } => {
                let lowered_operand = self.lower_expression(operand)?;
                let operator = self.lower_unary_operator(*operator, operand.type_id(), span)?;
                Ok(CompileTimeExpression::unary(
                    operator,
                    lowered_operand,
                    type_id,
                    span,
                ))
            }
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let operator = self.lower_binary_operator(*operator, left.type_id(), span)?;
                Ok(CompileTimeExpression::binary(
                    operator,
                    self.lower_expression(left)?,
                    self.lower_expression(right)?,
                    type_id,
                    span,
                ))
            }
            TypedExpressionKind::Conditional {
                condition,
                when_true,
                when_false,
            } => Ok(CompileTimeExpression::conditional(
                self.lower_expression(condition)?,
                self.lower_expression(when_true)?,
                self.lower_expression(when_false)?,
                type_id,
                span,
            )),
            TypedExpressionKind::NumericConvert { value, conversion } => {
                Ok(CompileTimeExpression::numeric_convert(
                    *conversion,
                    self.lower_expression(value)?,
                    type_id,
                    span,
                ))
            }
            TypedExpressionKind::DirectCall {
                function,
                arguments,
                ..
            } => Ok(CompileTimeExpression::call(
                FunctionId::from_raw(function.raw()),
                arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect::<Result<Vec<_>, _>>()?,
                type_id,
                span,
            )),
            TypedExpressionKind::Local(local) => {
                Ok(CompileTimeExpression::local(*local, type_id, span))
            }
            TypedExpressionKind::AttributeQuery {
                module,
                attribute,
                subject,
            } => Ok(CompileTimeExpression::attribute_query(
                *module, *attribute, *subject, false, type_id, span,
            )),
            TypedExpressionKind::HasAttributeQuery {
                module,
                attribute,
                subject,
            } => Ok(CompileTimeExpression::attribute_query(
                *module, *attribute, *subject, true, type_id, span,
            )),
            unsupported => Err(unsupported_expression(
                expression,
                unsupported_compile_time_construct(unsupported),
            )),
        }
    }

    fn lower_parameter(
        &self,
        parameter: u32,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let Some(expected) = usize::try_from(parameter)
            .ok()
            .and_then(|index| self.parameter_types.get(index))
            .copied()
        else {
            return Err(CompileTimeLoweringError::UnknownParameter { parameter, span });
        };
        if expected != type_id {
            return Err(CompileTimeLoweringError::TypeMismatch {
                expected,
                found: type_id,
                span,
            });
        }
        Ok(CompileTimeExpression::parameter(parameter, type_id, span))
    }

    fn lower_tuple(
        &self,
        elements: &[TypedExpression],
        type_id: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let elements = elements
            .iter()
            .map(|element| self.lower_expression(element))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CompileTimeExpression::tuple(elements, type_id, span))
    }

    fn lower_unary_operator(
        &self,
        operator: TypedUnaryOperator,
        operand: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeUnaryOperator, CompileTimeLoweringError> {
        match operator {
            TypedUnaryOperator::Not => Ok(CompileTimeUnaryOperator::BooleanNot),
            TypedUnaryOperator::Negate if is_integer_type(self.types, operand) => {
                Ok(CompileTimeUnaryOperator::CheckedIntegerNegate)
            }
            TypedUnaryOperator::Negate if is_float_type(self.types, operand) => {
                Ok(CompileTimeUnaryOperator::FloatNegate)
            }
            TypedUnaryOperator::Negate => {
                Err(CompileTimeLoweringError::InvalidOperatorTypes { span })
            }
        }
    }

    fn lower_binary_operator(
        &self,
        operator: TypedBinaryOperator,
        operand: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeBinaryOperator, CompileTimeLoweringError> {
        let integer = is_integer_type(self.types, operand);
        let float = is_float_type(self.types, operand);
        match operator {
            TypedBinaryOperator::Add if integer => Ok(CompileTimeBinaryOperator::CheckedAdd),
            TypedBinaryOperator::Subtract if integer => {
                Ok(CompileTimeBinaryOperator::CheckedSubtract)
            }
            TypedBinaryOperator::Multiply if integer => {
                Ok(CompileTimeBinaryOperator::CheckedMultiply)
            }
            TypedBinaryOperator::Divide if integer => Ok(CompileTimeBinaryOperator::CheckedDivide),
            TypedBinaryOperator::Remainder if integer => {
                Ok(CompileTimeBinaryOperator::CheckedRemainder)
            }
            TypedBinaryOperator::Add if float => Ok(CompileTimeBinaryOperator::FloatAdd),
            TypedBinaryOperator::Subtract if float => Ok(CompileTimeBinaryOperator::FloatSubtract),
            TypedBinaryOperator::Multiply if float => Ok(CompileTimeBinaryOperator::FloatMultiply),
            TypedBinaryOperator::Divide if float => Ok(CompileTimeBinaryOperator::FloatDivide),
            TypedBinaryOperator::Equal => Ok(CompileTimeBinaryOperator::Equal),
            TypedBinaryOperator::NotEqual => Ok(CompileTimeBinaryOperator::NotEqual),
            TypedBinaryOperator::LessThan => Ok(CompileTimeBinaryOperator::LessThan),
            TypedBinaryOperator::LessThanOrEqual => Ok(CompileTimeBinaryOperator::LessThanOrEqual),
            TypedBinaryOperator::GreaterThan => Ok(CompileTimeBinaryOperator::GreaterThan),
            TypedBinaryOperator::GreaterThanOrEqual => {
                Ok(CompileTimeBinaryOperator::GreaterThanOrEqual)
            }
            TypedBinaryOperator::And => Ok(CompileTimeBinaryOperator::And),
            TypedBinaryOperator::Or => Ok(CompileTimeBinaryOperator::Or),
            TypedBinaryOperator::Add
            | TypedBinaryOperator::Subtract
            | TypedBinaryOperator::Multiply
            | TypedBinaryOperator::Divide
            | TypedBinaryOperator::Remainder => {
                Err(CompileTimeLoweringError::InvalidOperatorTypes { span })
            }
        }
    }
}

fn lower_compile_time_literal(expression: &TypedExpression) -> CompileTimeExpression {
    let value = match expression.kind() {
        TypedExpressionKind::Integer(value) => CompileTimeValue::Integer(*value),
        TypedExpressionKind::Float(value) => CompileTimeValue::Float(*value),
        TypedExpressionKind::String(value) => CompileTimeValue::String(value.clone()),
        TypedExpressionKind::Boolean(value) => CompileTimeValue::Boolean(*value),
        TypedExpressionKind::Nil => CompileTimeValue::Nil,
        _ => unreachable!("literal lowering receives only a typed literal"),
    };
    CompileTimeExpression::constant(value, expression.type_id(), expression.span())
}

fn unsupported_statement_error(statement: &TypedStatement) -> Option<CompileTimeLoweringError> {
    let construct = match statement.kind() {
        TypedStatementKind::While { .. }
        | TypedStatementKind::OptionalWhile { .. }
        | TypedStatementKind::RepeatUntil { .. }
        | TypedStatementKind::NumericFor { .. }
        | TypedStatementKind::Break
        | TypedStatementKind::Continue => UnsupportedCompileTimeConstruct::Loop,
        TypedStatementKind::LocalSet { .. }
        | TypedStatementKind::MultipleAssignment { .. }
        | TypedStatementKind::ParameterSet { .. }
        | TypedStatementKind::CaptureSet { .. }
        | TypedStatementKind::FieldSet { .. }
        | TypedStatementKind::CompoundFieldSet { .. }
        | TypedStatementKind::ArraySet { .. }
        | TypedStatementKind::TableSet { .. }
        | TypedStatementKind::CompoundArraySet { .. } => UnsupportedCompileTimeConstruct::Mutation,
        TypedStatementKind::Match { .. } => UnsupportedCompileTimeConstruct::Match,
        TypedStatementKind::ErrorMatch { .. } | TypedStatementKind::ResultMatch { .. } => {
            UnsupportedCompileTimeConstruct::TypedFailure
        }
        TypedStatementKind::Defer { .. } => UnsupportedCompileTimeConstruct::TypedFailure,
        TypedStatementKind::OptionalIf { .. } => UnsupportedCompileTimeConstruct::OptionalFlow,
        TypedStatementKind::Call(call) => match call.dispatch() {
            TypedCallDispatch::Standard { .. } | TypedCallDispatch::Direct { .. } => {
                UnsupportedCompileTimeConstruct::ResultlessCall
            }
            TypedCallDispatch::Referenced { .. } => UnsupportedCompileTimeConstruct::ReferencedCall,
            TypedCallDispatch::DirectMethod { .. } => UnsupportedCompileTimeConstruct::MethodCall,
            TypedCallDispatch::InterfaceMethod { .. } => {
                UnsupportedCompileTimeConstruct::InterfaceDispatch
            }
            TypedCallDispatch::Indirect { .. } => UnsupportedCompileTimeConstruct::IndirectCall,
        },
        TypedStatementKind::Local { .. }
        | TypedStatementKind::MultipleLocal { .. }
        | TypedStatementKind::Return { .. }
        | TypedStatementKind::If { .. }
        | TypedStatementKind::Expression(_) => return None,
    };
    Some(CompileTimeLoweringError::UnsupportedConstruct {
        construct,
        span: statement.span(),
    })
}

const fn unsupported_expression(
    expression: &TypedExpression,
    construct: UnsupportedCompileTimeConstruct,
) -> CompileTimeLoweringError {
    CompileTimeLoweringError::UnsupportedConstruct {
        construct,
        span: expression.span(),
    }
}

fn unsupported_compile_time_construct(
    expression: &TypedExpressionKind,
) -> UnsupportedCompileTimeConstruct {
    match expression {
        TypedExpressionKind::Closure(_) => UnsupportedCompileTimeConstruct::Closure,
        TypedExpressionKind::Capture(_) => UnsupportedCompileTimeConstruct::Capture,
        TypedExpressionKind::Function(_) => UnsupportedCompileTimeConstruct::FunctionReference,
        TypedExpressionKind::Field { .. } => UnsupportedCompileTimeConstruct::FieldAccess,
        TypedExpressionKind::ArrayGet { .. }
        | TypedExpressionKind::ArrayLength { .. }
        | TypedExpressionKind::ArrayGetChecked { .. } => {
            UnsupportedCompileTimeConstruct::ArrayAccess
        }
        TypedExpressionKind::ArrayCreate { .. } | TypedExpressionKind::Array(_) => {
            UnsupportedCompileTimeConstruct::Array
        }
        TypedExpressionKind::ArrayFill { .. } => UnsupportedCompileTimeConstruct::Mutation,
        TypedExpressionKind::Record { .. } => UnsupportedCompileTimeConstruct::Record,
        TypedExpressionKind::ClassConstruct { .. } => {
            UnsupportedCompileTimeConstruct::ClassConstruction
        }
        TypedExpressionKind::RecordUpdate { .. } => UnsupportedCompileTimeConstruct::RecordUpdate,
        TypedExpressionKind::Table(_) | TypedExpressionKind::TableGet { .. } => {
            UnsupportedCompileTimeConstruct::Table
        }
        TypedExpressionKind::UnionCase { .. } => UnsupportedCompileTimeConstruct::UnionCase,
        TypedExpressionKind::ResultCase { .. }
        | TypedExpressionKind::ErrorCase { .. }
        | TypedExpressionKind::ResultPropagate { .. } => {
            UnsupportedCompileTimeConstruct::TypedFailure
        }
        TypedExpressionKind::EnumCase { .. } => UnsupportedCompileTimeConstruct::UnionCase,
        TypedExpressionKind::DirectMethodCall { .. } => UnsupportedCompileTimeConstruct::MethodCall,
        TypedExpressionKind::InterfaceMethodCall { .. } => {
            UnsupportedCompileTimeConstruct::InterfaceDispatch
        }
        TypedExpressionKind::InterfaceUpcast { .. } => {
            UnsupportedCompileTimeConstruct::InterfaceConversion
        }
        TypedExpressionKind::IndirectCall { .. } => UnsupportedCompileTimeConstruct::IndirectCall,
        TypedExpressionKind::ReferencedCall { .. } => {
            UnsupportedCompileTimeConstruct::ReferencedCall
        }
        TypedExpressionKind::StandardCall { .. } => UnsupportedCompileTimeConstruct::ResultlessCall,
        TypedExpressionKind::StringConcat { .. } | TypedExpressionKind::StringFormat { .. } => {
            UnsupportedCompileTimeConstruct::StringComposition
        }
        TypedExpressionKind::OptionalDefault { .. }
        | TypedExpressionKind::OptionalPropagate { .. }
        | TypedExpressionKind::OptionalNarrow { .. } => {
            UnsupportedCompileTimeConstruct::OptionalFlow
        }
        TypedExpressionKind::Integer(_)
        | TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. }
        | TypedExpressionKind::Float(_)
        | TypedExpressionKind::String(_)
        | TypedExpressionKind::Boolean(_)
        | TypedExpressionKind::Nil
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::Parameter(_)
        | TypedExpressionKind::Tuple(_)
        | TypedExpressionKind::TupleGet { .. }
        | TypedExpressionKind::Unary { .. }
        | TypedExpressionKind::Binary { .. }
        | TypedExpressionKind::Conditional { .. }
        | TypedExpressionKind::NumericConvert { .. }
        | TypedExpressionKind::DirectCall { .. } => {
            unreachable!("supported expression is not routed to the unsupported lowerer")
        }
    }
}
