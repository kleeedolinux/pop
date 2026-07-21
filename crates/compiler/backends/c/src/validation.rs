//! Capability validation for the experimental runtime-free C subset.
//!
//! Keeping rejection policy separate from text emission makes unsupported MIR
//! explicit and prevents the backend from silently inventing fallback semantics.
use crate::api::{CBackendError, CLoweringOptions};
use pop_foundation::TypeId;
use pop_mir::{MirBubble, MirEffect, MirFunction, MirInstructionKind, MirTerminator};
use pop_types::{IntegerKind, PrimitiveType, SemanticType, TypeArena};
pub(crate) fn validate_bubble(
    bubble: &MirBubble,
    types: &TypeArena,
    options: CLoweringOptions,
) -> Result<(), CBackendError> {
    for function in bubble.functions() {
        for block in function.blocks() {
            if let Some(instruction) = block.instructions().iter().find(|instruction| {
                matches!(
                    instruction.kind(),
                    MirInstructionKind::CheckedDowncast { .. }
                        | MirInstructionKind::CodecEncode { .. }
                        | MirInstructionKind::CodecDecode { .. }
                ) || is_view_instruction(instruction.kind())
            }) {
                return Err(CBackendError::UnsupportedInstruction {
                    function: function.function(),
                    value: instruction.result(),
                });
            }
        }
    }
    if !bubble.declarations().is_empty()
        || !bubble.methods().is_empty()
        || !bubble.nested_functions().is_empty()
    {
        return Err(CBackendError::UnsupportedDeclarations);
    }
    for function in bubble.functions() {
        validate_function(function, types)?;
    }
    if let Some(entry) = options.entry_point {
        let function = bubble
            .functions()
            .iter()
            .find(|function| function.symbol() == entry)
            .ok_or(CBackendError::InvalidEntryPoint(entry))?;
        let int = types.source_type("Int");
        if !function.parameters().is_empty()
            || !(function.results().is_empty()
                || function.results().len() == 1 && function.results().first().copied() == int)
        {
            return Err(CBackendError::UnsupportedEntryPointSignature(entry));
        }
    }
    Ok(())
}

fn is_view_instruction(kind: &MirInstructionKind) -> bool {
    matches!(
        kind,
        MirInstructionKind::ViewCreate { .. }
            | MirInstructionKind::ViewSlice { .. }
            | MirInstructionKind::ViewLength { .. }
            | MirInstructionKind::ViewGetByte { .. }
            | MirInstructionKind::ViewMaterialize { .. }
            | MirInstructionKind::ViewEnd { .. }
    )
}

fn validate_function(function: &MirFunction, types: &TypeArena) -> Result<(), CBackendError> {
    if function.is_async() {
        return Err(CBackendError::UnsupportedAsync(function.function()));
    }
    if function.results().len() > 1 {
        return Err(CBackendError::UnsupportedFunctionSignature(
            function.symbol(),
        ));
    }
    for block in function.blocks() {
        if let Some(instruction) = block
            .instructions()
            .iter()
            .find(|instruction| is_ffi_callback_instruction(instruction.kind()))
        {
            return Err(CBackendError::UnsupportedInstruction {
                function: function.function(),
                value: instruction.result(),
            });
        }
    }
    for type_id in function.parameters().iter().chain(function.results()) {
        c_type(*type_id, types)?;
    }
    for block in function.blocks() {
        for argument in block.arguments() {
            c_type(argument.type_id(), types)?;
        }
        for instruction in block.instructions() {
            if let Some(type_id) = instruction.optional_result_type() {
                c_type(type_id, types)?;
            }
            if !is_supported_instruction(instruction.kind()) {
                return Err(CBackendError::UnsupportedInstruction {
                    function: function.function(),
                    value: instruction.result(),
                });
            }
            if matches!(
                instruction.kind(),
                MirInstructionKind::IntegerNegate { kind, .. } if !kind.is_signed()
            ) {
                return Err(CBackendError::UnsupportedInstruction {
                    function: function.function(),
                    value: instruction.result(),
                });
            }
        }
        if !matches!(
            block.terminator(),
            MirTerminator::Branch { .. }
                | MirTerminator::ConditionalBranch { .. }
                | MirTerminator::Return { .. }
                | MirTerminator::Trap(_)
                | MirTerminator::Unreachable
        ) {
            return Err(CBackendError::UnsupportedTerminator {
                function: function.function(),
                block: block.block(),
            });
        }
    }
    let unsupported_effects = [
        MirEffect::Allocates,
        MirEffect::WritesManagedReference,
        MirEffect::MayUnwind,
        MirEffect::Suspends,
        MirEffect::Blocks,
        MirEffect::UnsafeMemory,
        MirEffect::ForeignFunction,
        MirEffect::CompilerQuery,
        MirEffect::GcSafePoint,
        MirEffect::Roots,
    ];
    if unsupported_effects
        .into_iter()
        .any(|effect| function.effects().contains(effect))
    {
        return Err(CBackendError::UnsupportedEffects(function.function()));
    }
    Ok(())
}

fn is_supported_instruction(kind: &MirInstructionKind) -> bool {
    if is_ffi_callback_instruction(kind) {
        return false;
    }
    if let MirInstructionKind::CallStandard {
        function,
        arguments,
        ..
    } = kind
    {
        return matches!(function.raw(), 0 | 1) && arguments.len() == 1;
    }
    matches!(
        kind,
        MirInstructionKind::IntegerConstant(_)
            | MirInstructionKind::FloatConstant(_)
            | MirInstructionKind::StringConstant(_)
            | MirInstructionKind::BooleanConstant(_)
            | MirInstructionKind::CheckedIntegerAdd { .. }
            | MirInstructionKind::CheckedIntegerSubtract { .. }
            | MirInstructionKind::CheckedIntegerMultiply { .. }
            | MirInstructionKind::CheckedIntegerDivide { .. }
            | MirInstructionKind::CheckedIntegerRemainder { .. }
            | MirInstructionKind::FloatAdd { .. }
            | MirInstructionKind::FloatSubtract { .. }
            | MirInstructionKind::FloatMultiply { .. }
            | MirInstructionKind::FloatDivide { .. }
            | MirInstructionKind::ConvertInteger { .. }
            | MirInstructionKind::ConvertIntegerToFloat { .. }
            | MirInstructionKind::ConvertFloatToInteger { .. }
            | MirInstructionKind::ConvertFloat { .. }
            | MirInstructionKind::BooleanNot { .. }
            | MirInstructionKind::IntegerNegate { .. }
            | MirInstructionKind::FloatNegate { .. }
            | MirInstructionKind::BooleanAnd { .. }
            | MirInstructionKind::BooleanOr { .. }
            | MirInstructionKind::CompareEqual { .. }
            | MirInstructionKind::CompareNotEqual { .. }
            | MirInstructionKind::CompareIntegerLess { .. }
            | MirInstructionKind::CompareIntegerLessOrEqual { .. }
            | MirInstructionKind::CompareIntegerGreater { .. }
            | MirInstructionKind::CompareIntegerGreaterOrEqual { .. }
            | MirInstructionKind::CompareFloatLess { .. }
            | MirInstructionKind::CompareFloatLessOrEqual { .. }
            | MirInstructionKind::CompareFloatGreater { .. }
            | MirInstructionKind::CompareFloatGreaterOrEqual { .. }
            | MirInstructionKind::CallDirect { .. }
    )
}

fn is_ffi_callback_instruction(kind: &MirInstructionKind) -> bool {
    matches!(
        kind,
        MirInstructionKind::FfiCallbackOpenScoped { .. }
            | MirInstructionKind::FfiCallbackOpenOwned { .. }
            | MirInstructionKind::CallCallbackPair { .. }
            | MirInstructionKind::FfiCallbackCloseScoped { .. }
            | MirInstructionKind::FfiCallbackCloseOwned { .. }
    )
}

pub(crate) fn c_type(type_id: TypeId, types: &TypeArena) -> Result<&'static str, CBackendError> {
    match types.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Boolean)) => Ok("bool"),
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Ok(integer_c_type(*kind)),
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => Ok("float"),
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => Ok("double"),
        Some(SemanticType::Primitive(PrimitiveType::String)) => Ok("pop_string"),
        _ => Err(CBackendError::UnsupportedType(type_id)),
    }
}

pub(crate) const fn integer_c_type(kind: IntegerKind) -> &'static str {
    match kind {
        IntegerKind::Int8 => "int8_t",
        IntegerKind::Int16 => "int16_t",
        IntegerKind::Int32 => "int32_t",
        IntegerKind::Int64 => "int64_t",
        IntegerKind::UInt8 => "uint8_t",
        IntegerKind::UInt16 => "uint16_t",
        IntegerKind::UInt32 => "uint32_t",
        IntegerKind::UInt64 => "uint64_t",
    }
}
