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
        TypedStatementKind::OptionalIf {
            initializer,
            then_body,
            else_body,
            ..
        } => {
            finalize_expression_captures(initializer, written);
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
        TypedStatementKind::OptionalWhile {
            initializer, body, ..
        } => {
            finalize_expression_captures(initializer, written);
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
        TypedStatementKind::GeneralizedFor { iterable, body, .. } => {
            finalize_expression_captures(iterable, written);
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
        TypedStatementKind::ErrorMatch {
            scrutinee, arms, ..
        } => {
            finalize_expression_captures(scrutinee, written);
            for arm in arms {
                for statement in &mut arm.body {
                    finalize_statement_captures(statement, written);
                }
            }
        }
        TypedStatementKind::ResultMatch {
            scrutinee, arms, ..
        } => {
            finalize_expression_captures(scrutinee, written);
            for arm in arms {
                for statement in &mut arm.body {
                    finalize_statement_captures(statement, written);
                }
            }
        }
        TypedStatementKind::Defer { body } | TypedStatementKind::AsyncDefer { body } => {
            for statement in body {
                finalize_statement_captures(statement, written);
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
        TypedStatementKind::ListSet { list, index, value } => {
            finalize_expression_captures(list, written);
            finalize_expression_captures(index, written);
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::TableSet { table, key, value } => {
            finalize_expression_captures(table, written);
            finalize_expression_captures(key, written);
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
                    TypedAssignmentTarget::List { list, index, .. } => {
                        finalize_expression_captures(list, written);
                        finalize_expression_captures(index, written);
                    }
                    TypedAssignmentTarget::Table { table, key, .. } => {
                        finalize_expression_captures(table, written);
                        finalize_expression_captures(key, written);
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
        TypedCallDispatch::InterfaceMethod { receiver, .. }
        | TypedCallDispatch::BuiltinInterfaceMethod { receiver, .. } => {
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
        | TypedExpressionKind::Function(_)
        | TypedExpressionKind::TaskCancellationSource
        | TypedExpressionKind::EnumCase { .. } => {}
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
        | TypedExpressionKind::ArrayGetChecked { array, index }
        | TypedExpressionKind::ListGet { list: array, index }
        | TypedExpressionKind::ListGetChecked { list: array, index } => {
            finalize_expression_captures(array, written);
            finalize_expression_captures(index, written);
        }
        TypedExpressionKind::TableGet { table, key } => {
            finalize_expression_captures(table, written);
            finalize_expression_captures(key, written);
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
        TypedExpressionKind::ListCreate { capacity } => {
            if let Some(capacity) = capacity {
                finalize_expression_captures(capacity, written);
            }
        }
        TypedExpressionKind::RangeCreate { first, last, step } => {
            finalize_expression_captures(first, written);
            finalize_expression_captures(last, written);
            finalize_expression_captures(step, written);
        }
        TypedExpressionKind::ListLength { list } => {
            finalize_expression_captures(list, written);
        }
        TypedExpressionKind::ListAdd { list, value } => {
            finalize_expression_captures(list, written);
            finalize_expression_captures(value, written);
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
        | TypedExpressionKind::ResultCase { arguments, .. }
        | TypedExpressionKind::IterationCase { arguments, .. }
        | TypedExpressionKind::ErrorCase { arguments, .. }
        | TypedExpressionKind::DirectCall { arguments, .. }
        | TypedExpressionKind::ReferencedCall { arguments, .. }
        | TypedExpressionKind::StandardCall { arguments, .. } => {
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::Unary { operand, .. }
        | TypedExpressionKind::Await { task: operand }
        | TypedExpressionKind::TaskCancelToken { source: operand }
        | TypedExpressionKind::TaskCancel { source: operand }
        | TypedExpressionKind::FfiHandleOpen { value: operand }
        | TypedExpressionKind::FfiHandleGet { handle: operand }
        | TypedExpressionKind::FfiHandleClose { handle: operand } => {
            finalize_expression_captures(operand, written);
        }
        TypedExpressionKind::TaskGroup { cancel, body } => {
            finalize_expression_captures(cancel, written);
            finalize_expression_captures(body, written);
        }
        TypedExpressionKind::TaskStart { group, task } => {
            finalize_expression_captures(group, written);
            finalize_expression_captures(task, written);
        }
        TypedExpressionKind::Binary { left, right, .. } => {
            finalize_expression_captures(left, written);
            finalize_expression_captures(right, written);
        }
        TypedExpressionKind::OptionalDefault { optional, fallback } => {
            finalize_expression_captures(optional, written);
            finalize_expression_captures(fallback, written);
        }
        TypedExpressionKind::OptionalPropagate { optional, .. } => {
            finalize_expression_captures(optional, written);
        }
        TypedExpressionKind::ResultPropagate { result, .. } => {
            finalize_expression_captures(result, written);
        }
        TypedExpressionKind::OptionalNarrow { optional } => {
            finalize_expression_captures(optional, written);
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
        TypedExpressionKind::IndirectCall {
            callee, arguments, ..
        } => {
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
        }
        | TypedExpressionKind::BuiltinInterfaceMethodCall {
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
