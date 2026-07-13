//! Pure evaluation helpers for MIR operations and aggregate values.
//!
//! Keeping arithmetic, equality, field access, and argument materialization here
//! makes the control-flow engine readable and gives future instruction families a
//! focused home without growing the execution loop.
use crate::interpreter::ExecutionError;
use crate::values::{MirValue, RuntimeValue};
use pop_foundation::{FieldId, SymbolId, ValueId};
use pop_mir::{MirInstructionKind, MirUnwindAction};
use pop_types::{
    FloatKind, FloatValue, IntegerKind, IntegerValue, NumericError, PrimitiveType, SemanticType,
    TypeArena,
};
use std::cmp::Ordering;
use std::collections::BTreeMap;
pub(crate) fn evaluate_numeric_instruction(
    instruction: &MirInstructionKind,
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Option<MirValue>, ExecutionError> {
    let result = match instruction {
        MirInstructionKind::IntegerConstant(value) => Ok(MirValue::Integer(*value)),
        MirInstructionKind::FloatConstant(value) => Ok(MirValue::Float(*value)),
        MirInstructionKind::CheckedIntegerAdd { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_add)
        }
        MirInstructionKind::CheckedIntegerSubtract { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_subtract)
        }
        MirInstructionKind::CheckedIntegerMultiply { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_multiply)
        }
        MirInstructionKind::CheckedIntegerDivide { kind, left, right } => {
            checked_integer_binary(values, *kind, *left, *right, IntegerValue::checked_divide)
        }
        MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => {
            checked_integer_binary(
                values,
                *kind,
                *left,
                *right,
                IntegerValue::checked_remainder,
            )
        }
        MirInstructionKind::FloatAdd { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_add)
        }
        MirInstructionKind::FloatSubtract { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_subtract)
        }
        MirInstructionKind::FloatMultiply { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_multiply)
        }
        MirInstructionKind::FloatDivide { kind, left, right } => {
            checked_float_binary(values, *kind, *left, *right, FloatValue::checked_divide)
        }
        MirInstructionKind::IntegerNegate { kind, operand } => integer(values, *kind, *operand)?
            .checked_negate()
            .map(MirValue::Integer)
            .map_err(execution_numeric_error),
        MirInstructionKind::FloatNegate { kind, operand } => Ok(MirValue::Float(
            float_value(values, *kind, *operand)?.negate(),
        )),
        MirInstructionKind::ConvertInteger {
            source,
            target,
            operand,
        } => integer(values, *source, *operand)?
            .convert(*target)
            .map(MirValue::Integer)
            .map_err(conversion_numeric_error),
        MirInstructionKind::ConvertIntegerToFloat {
            source,
            target,
            operand,
        } => Ok(MirValue::Float(
            integer(values, *source, *operand)?.to_float(*target),
        )),
        MirInstructionKind::ConvertFloatToInteger {
            source,
            target,
            operand,
        } => float_value(values, *source, *operand)?
            .to_integer(*target)
            .map(MirValue::Integer)
            .map_err(conversion_numeric_error),
        MirInstructionKind::ConvertFloat {
            source,
            target,
            operand,
        } => Ok(MirValue::Float(
            float_value(values, *source, *operand)?.convert(*target),
        )),
        MirInstructionKind::CompareIntegerLess { kind, left, right } => {
            compare_integer(values, *kind, *left, *right, Ordering::is_lt)
        }
        MirInstructionKind::CompareIntegerGreater { kind, left, right } => {
            compare_integer(values, *kind, *left, *right, Ordering::is_gt)
        }
        MirInstructionKind::CompareIntegerLessOrEqual { kind, left, right } => {
            compare_integer(values, *kind, *left, *right, Ordering::is_le)
        }
        MirInstructionKind::CompareIntegerGreaterOrEqual { kind, left, right } => {
            compare_integer(values, *kind, *left, *right, Ordering::is_ge)
        }
        MirInstructionKind::CompareFloatLess { kind, left, right } => {
            compare_float(values, *kind, *left, *right, Ordering::is_lt)
        }
        MirInstructionKind::CompareFloatGreater { kind, left, right } => {
            compare_float(values, *kind, *left, *right, Ordering::is_gt)
        }
        MirInstructionKind::CompareFloatLessOrEqual { kind, left, right } => {
            compare_float(values, *kind, *left, *right, Ordering::is_le)
        }
        MirInstructionKind::CompareFloatGreaterOrEqual { kind, left, right } => {
            compare_float(values, *kind, *left, *right, Ordering::is_ge)
        }
        _ => return Ok(None),
    }?;
    Ok(Some(result))
}

pub(crate) fn value(
    values: &BTreeMap<ValueId, RuntimeValue>,
    id: ValueId,
) -> Result<&RuntimeValue, ExecutionError> {
    values.get(&id).ok_or(ExecutionError::MissingValue(id))
}

pub(crate) fn evaluated_arguments(
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Vec<RuntimeValue>, ExecutionError> {
    arguments
        .iter()
        .map(|argument| value(values, *argument).cloned())
        .collect()
}

pub(crate) fn single_result(
    mut returned: Vec<RuntimeValue>,
) -> Result<RuntimeValue, ExecutionError> {
    if returned.len() != 1 {
        return Err(ExecutionError::WrongArity);
    }
    returned.pop().ok_or(ExecutionError::WrongArity)
}

pub(crate) fn require_runtime_numeric_types(
    arena: &TypeArena,
    expected: &[pop_foundation::TypeId],
    values: &[RuntimeValue],
) -> Result<(), ExecutionError> {
    if expected.len() != values.len() {
        return Err(ExecutionError::WrongArity);
    }
    for (expected, value) in expected.iter().zip(values) {
        let matches = match arena.get(*expected) {
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
                matches!(&value.visible, MirValue::Integer(integer) if integer.kind() == *kind)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
                matches!(&value.visible, MirValue::Float(float) if float.kind() == FloatKind::Float32)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
                matches!(&value.visible, MirValue::Float(float) if float.kind() == FloatKind::Float64)
            }
            _ => true,
        };
        if !matches {
            return Err(ExecutionError::TypeMismatch);
        }
    }
    Ok(())
}

fn integer(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    operand: ValueId,
) -> Result<IntegerValue, ExecutionError> {
    match &value(values, operand)?.visible {
        MirValue::Integer(value) if value.kind() == kind => Ok(*value),
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn integers(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
) -> Result<(IntegerValue, IntegerValue), ExecutionError> {
    Ok((integer(values, kind, left)?, integer(values, kind, right)?))
}

fn checked_integer_binary(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
    operation: fn(IntegerValue, IntegerValue) -> Result<IntegerValue, NumericError>,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = integers(values, kind, left, right)?;
    operation(left, right)
        .map(MirValue::Integer)
        .map_err(execution_numeric_error)
}

fn float_value(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    operand: ValueId,
) -> Result<FloatValue, ExecutionError> {
    match &value(values, operand)?.visible {
        MirValue::Float(value) if value.kind() == kind => Ok(*value),
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn floats(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    left: ValueId,
    right: ValueId,
) -> Result<(FloatValue, FloatValue), ExecutionError> {
    Ok((
        float_value(values, kind, left)?,
        float_value(values, kind, right)?,
    ))
}

fn checked_float_binary(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    left: ValueId,
    right: ValueId,
    operation: fn(FloatValue, FloatValue) -> Result<FloatValue, NumericError>,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = floats(values, kind, left, right)?;
    operation(left, right)
        .map(MirValue::Float)
        .map_err(execution_numeric_error)
}

pub(crate) fn boolean_binary(
    values: &BTreeMap<ValueId, RuntimeValue>,
    left: ValueId,
    right: ValueId,
    operation: impl FnOnce(bool, bool) -> bool,
) -> Result<MirValue, ExecutionError> {
    match (
        &value(values, left)?.visible,
        &value(values, right)?.visible,
    ) {
        (MirValue::Boolean(left), MirValue::Boolean(right)) => {
            Ok(MirValue::Boolean(operation(*left, *right)))
        }
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn compare_integer(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
    comparison: impl FnOnce(Ordering) -> bool,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = integers(values, kind, left, right)?;
    let ordering = left.compare(right).map_err(execution_numeric_error)?;
    Ok(MirValue::Boolean(comparison(ordering)))
}

fn compare_float(
    values: &BTreeMap<ValueId, RuntimeValue>,
    kind: FloatKind,
    left: ValueId,
    right: ValueId,
    comparison: impl FnOnce(Ordering) -> bool,
) -> Result<MirValue, ExecutionError> {
    let (left, right) = floats(values, kind, left, right)?;
    let ordering = left
        .partial_compare(right)
        .map_err(execution_numeric_error)?;
    Ok(MirValue::Boolean(ordering.is_some_and(comparison)))
}

const fn execution_numeric_error(error: NumericError) -> ExecutionError {
    match error {
        NumericError::KindMismatch => ExecutionError::TypeMismatch,
        NumericError::Overflow | NumericError::OutOfRange => ExecutionError::IntegerOverflow,
        NumericError::DivisionByZero => ExecutionError::DivisionByZero,
        NumericError::InvalidLiteral => ExecutionError::InvalidControlFlow,
    }
}

const fn conversion_numeric_error(error: NumericError) -> ExecutionError {
    match error {
        NumericError::OutOfRange | NumericError::Overflow => ExecutionError::NumericConversion,
        NumericError::KindMismatch => ExecutionError::TypeMismatch,
        NumericError::DivisionByZero | NumericError::InvalidLiteral => {
            ExecutionError::InvalidControlFlow
        }
    }
}

pub(crate) fn pop_value_equal(left: &MirValue, right: &MirValue) -> bool {
    match (left, right) {
        (MirValue::Nil, MirValue::Nil) => true,
        (MirValue::Boolean(left), MirValue::Boolean(right)) => left == right,
        (MirValue::Integer(left), MirValue::Integer(right)) => left == right,
        (MirValue::String(left), MirValue::String(right)) => left == right,
        (MirValue::Tuple(left), MirValue::Tuple(right)) => values_equal(left, right),
        (
            MirValue::Record {
                fields: left_fields,
                ..
            },
            MirValue::Record {
                fields: right_fields,
                ..
            },
        ) => record_fields_equal(left_fields, right_fields),
        (MirValue::Class(left), MirValue::Class(right)) => left == right,
        (
            MirValue::Union {
                union: left_union,
                case: left_case,
                arguments: left_arguments,
            },
            MirValue::Union {
                union: right_union,
                case: right_case,
                arguments: right_arguments,
            },
        ) => {
            left_union == right_union
                && left_case == right_case
                && values_equal(left_arguments, right_arguments)
        }
        _ => false,
    }
}

fn values_equal(left: &[MirValue], right: &[MirValue]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .all(|(left, right)| pop_value_equal(left, right))
}

fn record_fields_equal(left: &[(FieldId, MirValue)], right: &[(FieldId, MirValue)]) -> bool {
    left.len() == right.len()
        && left.iter().all(|(field, left_value)| {
            right
                .iter()
                .find(|(candidate, _)| candidate == field)
                .is_some_and(|(_, right_value)| pop_value_equal(left_value, right_value))
        })
}

pub(crate) fn update_record(
    record: SymbolId,
    base: ValueId,
    fields: &[(FieldId, ValueId)],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<RuntimeValue, ExecutionError> {
    let MirValue::Record {
        fields: base_fields,
        ..
    } = &value(values, base)?.visible
    else {
        return Err(ExecutionError::TypeMismatch);
    };
    let mut updated = base_fields.clone();
    for (field, value) in evaluate_visible_fields(fields, values)? {
        if let Some(existing) = updated.iter_mut().find(|(existing, _)| *existing == field) {
            existing.1 = value;
        } else {
            return Err(ExecutionError::InvalidControlFlow);
        }
    }
    Ok(RuntimeValue::visible(MirValue::Record {
        record,
        fields: updated,
    }))
}

pub(crate) fn get_field(
    base: ValueId,
    field: FieldId,
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<RuntimeValue, ExecutionError> {
    match &value(values, base)?.visible {
        MirValue::Record { fields, .. } => {
            find_visible_field(fields, field).map(RuntimeValue::visible)
        }
        MirValue::Class(class) => find_runtime_field(&class.fields.borrow(), field),
        _ => Err(ExecutionError::TypeMismatch),
    }
}

fn find_visible_field(
    fields: &[(FieldId, MirValue)],
    field: FieldId,
) -> Result<MirValue, ExecutionError> {
    fields
        .iter()
        .find(|(candidate, _)| *candidate == field)
        .map(|(_, value)| value.clone())
        .ok_or(ExecutionError::InvalidControlFlow)
}

fn find_runtime_field(
    fields: &[(FieldId, RuntimeValue)],
    field: FieldId,
) -> Result<RuntimeValue, ExecutionError> {
    fields
        .iter()
        .find(|(candidate, _)| *candidate == field)
        .map(|(_, value)| value.clone())
        .ok_or(ExecutionError::InvalidControlFlow)
}

pub(crate) fn set_field(
    base: ValueId,
    field: FieldId,
    new_value: ValueId,
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<RuntimeValue, ExecutionError> {
    let MirValue::Class(class) = &value(values, base)?.visible else {
        return Err(ExecutionError::TypeMismatch);
    };
    let new_value = value(values, new_value)?.clone();
    let mut fields = class.fields.borrow_mut();
    let Some((_, current)) = fields.iter_mut().find(|(candidate, _)| *candidate == field) else {
        return Err(ExecutionError::InvalidControlFlow);
    };
    *current = new_value;
    Ok(RuntimeValue::visible(MirValue::Nil))
}

pub(crate) fn evaluate_fields(
    fields: &[(FieldId, ValueId)],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Vec<(FieldId, RuntimeValue)>, ExecutionError> {
    fields
        .iter()
        .map(|(field, value_id)| Ok((*field, value(values, *value_id)?.clone())))
        .collect()
}

pub(crate) fn evaluate_visible_fields(
    fields: &[(FieldId, ValueId)],
    values: &BTreeMap<ValueId, RuntimeValue>,
) -> Result<Vec<(FieldId, MirValue)>, ExecutionError> {
    fields
        .iter()
        .map(|(field, value_id)| Ok((*field, value(values, *value_id)?.visible.clone())))
        .collect()
}

pub(crate) fn call_cleanup_target(
    instruction: &MirInstructionKind,
) -> Option<pop_foundation::BlockId> {
    match instruction {
        MirInstructionKind::CallDirect {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        }
        | MirInstructionKind::CallDirectMethod {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        }
        | MirInstructionKind::CallIndirect {
            unwind: MirUnwindAction::Cleanup(target),
            ..
        } => Some(*target),
        _ => None,
    }
}
