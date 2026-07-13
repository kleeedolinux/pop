//! Mechanical typed-body traversal that finalizes closure capture modes.
//!
//! Capture identity is resolved by the checker. This focused pass only turns
//! the already-computed written-binding set into `Value` or shared `Cell`
//! modes across every nested typed expression shape.

use std::collections::BTreeSet;

use pop_foundation::BindingId;

use crate::typed_body::*;

pub(crate) fn finalize_capture_modes(body: &mut TypedBody, written: &BTreeSet<BindingId>) {
    for statement in &mut body.statements {
        finalize_statement_captures(statement, written);
    }
}

fn finalize_statement_captures(statement: &mut TypedStatement, written: &BTreeSet<BindingId>) {
    match &mut statement.kind {
        TypedStatementKind::Local { initializer, .. } => {
            finalize_expression_captures(initializer, written);
        }
        TypedStatementKind::MultipleLocal { value, .. } => {
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::LocalSet { value, .. }
        | TypedStatementKind::ParameterSet { value, .. }
        | TypedStatementKind::CaptureSet { value, .. }
        | TypedStatementKind::Expression(value) => finalize_expression_captures(value, written),
        TypedStatementKind::Return { values } => {
            for value in values {
                finalize_expression_captures(value, written);
            }
        }
        TypedStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            finalize_expression_captures(condition, written);
            for statement in then_body.iter_mut().chain(else_body.iter_mut()) {
                finalize_statement_captures(statement, written);
            }
        }
        TypedStatementKind::While { condition, body } => {
            finalize_expression_captures(condition, written);
            for statement in body {
                finalize_statement_captures(statement, written);
            }
        }
        TypedStatementKind::RepeatUntil { body, condition } => {
            for statement in body {
                finalize_statement_captures(statement, written);
            }
            finalize_expression_captures(condition, written);
        }
        TypedStatementKind::NumericFor {
            first,
            last,
            step,
            body,
            ..
        } => {
            finalize_expression_captures(first, written);
            finalize_expression_captures(last, written);
            finalize_expression_captures(step, written);
            for statement in body {
                finalize_statement_captures(statement, written);
            }
        }
        TypedStatementKind::Break | TypedStatementKind::Continue => {}
        TypedStatementKind::Match {
            scrutinee, arms, ..
        } => {
            finalize_expression_captures(scrutinee, written);
            for arm in arms {
                for statement in &mut arm.body {
                    finalize_statement_captures(statement, written);
                }
            }
        }
        TypedStatementKind::FieldSet { base, value, .. } => {
            finalize_expression_captures(base, written);
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::CompoundFieldSet { base, value, .. } => {
            finalize_expression_captures(base, written);
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::ArraySet {
            array,
            index,
            value,
        } => {
            finalize_expression_captures(array, written);
            finalize_expression_captures(index, written);
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::CompoundArraySet {
            array,
            index,
            value,
            ..
        } => {
            finalize_expression_captures(array, written);
            finalize_expression_captures(index, written);
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::MultipleAssignment { targets, value } => {
            for target in targets {
                match target {
                    TypedAssignmentTarget::Local { .. } | TypedAssignmentTarget::Capture { .. } => {
                    }
                    TypedAssignmentTarget::Field { base, .. } => {
                        finalize_expression_captures(base, written);
                    }
                    TypedAssignmentTarget::Array { array, index, .. } => {
                        finalize_expression_captures(array, written);
                        finalize_expression_captures(index, written);
                    }
                }
            }
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::Call(call) => finalize_call_captures(call, written),
    }
}

fn finalize_call_captures(call: &mut TypedCall, written: &BTreeSet<BindingId>) {
    match &mut call.dispatch {
        TypedCallDispatch::Standard { .. }
        | TypedCallDispatch::Direct { .. }
        | TypedCallDispatch::Referenced { .. } => {}
        TypedCallDispatch::DirectMethod { receiver, .. } => {
            if let Some(receiver) = receiver {
                finalize_expression_captures(receiver, written);
            }
        }
        TypedCallDispatch::InterfaceMethod { receiver, .. } => {
            finalize_expression_captures(receiver, written);
        }
        TypedCallDispatch::Indirect { callee } => finalize_expression_captures(callee, written),
    }
    for argument in &mut call.arguments {
        finalize_expression_captures(argument, written);
    }
}

#[allow(clippy::too_many_lines)]
fn finalize_expression_captures(expression: &mut TypedExpression, written: &BTreeSet<BindingId>) {
    match &mut expression.kind {
        TypedExpressionKind::Integer(_)
        | TypedExpressionKind::Float(_)
        | TypedExpressionKind::String(_)
        | TypedExpressionKind::Boolean(_)
        | TypedExpressionKind::Nil
        | TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. }
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::Parameter(_)
        | TypedExpressionKind::Capture(_)
        | TypedExpressionKind::Function(_) => {}
        TypedExpressionKind::Closure(closure) => {
            for capture in &mut closure.captures {
                capture.mode = if written.contains(&capture.binding) {
                    CaptureMode::Cell
                } else {
                    CaptureMode::Value
                };
            }
            finalize_capture_modes(&mut closure.body, written);
        }
        TypedExpressionKind::Field { base, .. } => finalize_expression_captures(base, written),
        TypedExpressionKind::ClassConstruct { fields, .. }
        | TypedExpressionKind::Record { fields, .. } => {
            for field in fields {
                finalize_expression_captures(&mut field.value, written);
            }
        }
        TypedExpressionKind::ArrayGet { array, index }
        | TypedExpressionKind::ArrayGetChecked { array, index } => {
            finalize_expression_captures(array, written);
            finalize_expression_captures(index, written);
        }
        TypedExpressionKind::TupleGet { tuple, .. } => {
            finalize_expression_captures(tuple, written);
        }
        TypedExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            finalize_expression_captures(length, written);
            finalize_expression_captures(initial_value, written);
        }
        TypedExpressionKind::ArrayLength { array } => {
            finalize_expression_captures(array, written);
        }
        TypedExpressionKind::ArrayFill { array, value } => {
            finalize_expression_captures(array, written);
            finalize_expression_captures(value, written);
        }
        TypedExpressionKind::RecordUpdate { base, fields, .. } => {
            finalize_expression_captures(base, written);
            for field in fields {
                finalize_expression_captures(&mut field.value, written);
            }
        }
        TypedExpressionKind::Array(elements) | TypedExpressionKind::Tuple(elements) => {
            for element in elements {
                finalize_expression_captures(element, written);
            }
        }
        TypedExpressionKind::Table(entries) => {
            for entry in entries {
                finalize_expression_captures(&mut entry.key, written);
                finalize_expression_captures(&mut entry.value, written);
            }
        }
        TypedExpressionKind::UnionCase { arguments, .. }
        | TypedExpressionKind::DirectCall { arguments, .. }
        | TypedExpressionKind::ReferencedCall { arguments, .. }
        | TypedExpressionKind::StandardCall { arguments, .. } => {
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::Unary { operand, .. } => {
            finalize_expression_captures(operand, written);
        }
        TypedExpressionKind::Binary { left, right, .. } => {
            finalize_expression_captures(left, written);
            finalize_expression_captures(right, written);
        }
        TypedExpressionKind::StringConcat { left, right } => {
            finalize_expression_captures(left, written);
            finalize_expression_captures(right, written);
        }
        TypedExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            finalize_expression_captures(condition, written);
            finalize_expression_captures(when_true, written);
            finalize_expression_captures(when_false, written);
        }
        TypedExpressionKind::StringFormat { value, .. } => {
            finalize_expression_captures(value, written);
        }
        TypedExpressionKind::IndirectCall { callee, arguments } => {
            finalize_expression_captures(callee, written);
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::DirectMethodCall {
            receiver,
            arguments,
            ..
        } => {
            if let Some(receiver) = receiver {
                finalize_expression_captures(receiver, written);
            }
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::InterfaceMethodCall {
            receiver,
            arguments,
            ..
        } => {
            finalize_expression_captures(receiver, written);
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::InterfaceUpcast { value, .. } => {
            finalize_expression_captures(value, written);
        }
        TypedExpressionKind::NumericConvert { value, .. } => {
            finalize_expression_captures(value, written);
        }
    }
}
