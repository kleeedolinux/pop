//! Deterministic C11 helper and function emission.
//!
//! Functions here translate already-validated canonical MIR. They may choose C
//! spellings and helper shapes, but may not add source-level or backend-specific
//! semantics to MIR.
use crate::api::CBackendError;
use crate::validation::{c_type, integer_c_type};
use pop_foundation::{BlockId, SymbolId, ValueId};
use pop_mir::{MirBubble, MirFunction, MirInstructionKind, MirTerminator};
use pop_types::{
    FloatKind, FloatValue, IntegerKind, IntegerValue, NumericConversionKind, TypeArena,
};
use std::collections::BTreeSet;
use std::fmt::Write as _;
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum CheckedOperation {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct CheckedHelper {
    operation: CheckedOperation,
    kind: IntegerKind,
}
pub(crate) fn collect_checked_helpers(bubble: &MirBubble) -> BTreeSet<CheckedHelper> {
    let mut helpers = BTreeSet::new();
    for function in bubble.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                let helper = match instruction.kind() {
                    MirInstructionKind::CheckedIntegerAdd { kind, .. } => {
                        Some((CheckedOperation::Add, *kind))
                    }
                    MirInstructionKind::CheckedIntegerSubtract { kind, .. } => {
                        Some((CheckedOperation::Subtract, *kind))
                    }
                    MirInstructionKind::CheckedIntegerMultiply { kind, .. } => {
                        Some((CheckedOperation::Multiply, *kind))
                    }
                    MirInstructionKind::CheckedIntegerDivide { kind, .. } => {
                        Some((CheckedOperation::Divide, *kind))
                    }
                    MirInstructionKind::CheckedIntegerRemainder { kind, .. } => {
                        Some((CheckedOperation::Remainder, *kind))
                    }
                    MirInstructionKind::IntegerNegate { kind, .. } => {
                        Some((CheckedOperation::Negate, *kind))
                    }
                    _ => None,
                };
                if let Some((operation, kind)) = helper {
                    helpers.insert(CheckedHelper { operation, kind });
                }
            }
        }
    }
    helpers
}

pub(crate) fn collect_float_helpers(bubble: &MirBubble) -> BTreeSet<FloatKind> {
    bubble
        .functions()
        .iter()
        .flat_map(MirFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::FloatConstant(value) => Some(value.kind()),
            _ => None,
        })
        .collect()
}

pub(crate) fn contains_explicit_trap(bubble: &MirBubble) -> bool {
    bubble.functions().iter().any(|function| {
        function.blocks().iter().any(|block| {
            matches!(
                block.terminator(),
                MirTerminator::Trap(_) | MirTerminator::Unreachable
            )
        })
    })
}

pub(crate) fn contains_fallible_conversion(bubble: &MirBubble) -> bool {
    bubble
        .functions()
        .iter()
        .flat_map(MirFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .any(|instruction| {
            matches!(
                instruction.kind(),
                MirInstructionKind::ConvertFloatToInteger { .. }
            ) || matches!(
                instruction.kind(),
                MirInstructionKind::ConvertInteger { source, target, .. }
                    if NumericConversionKind::IntegerToInteger {
                        source: *source,
                        target: *target,
                    }.may_trap()
            )
        })
}

pub(crate) fn collect_standard_calls(bubble: &MirBubble) -> BTreeSet<u32> {
    bubble
        .functions()
        .iter()
        .flat_map(MirFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::CallStandard { function, .. } => Some(function.raw()),
            _ => None,
        })
        .collect()
}

pub(crate) fn emit_string_literals(output: &mut String, bubble: &MirBubble) {
    for function in bubble.functions() {
        for block in function.blocks() {
            for instruction in block.instructions() {
                let MirInstructionKind::StringConstant(value) = instruction.kind() else {
                    continue;
                };
                let _ = write!(
                    output,
                    "static const unsigned char pop_b{}_s{}_v{}_bytes[] = {{",
                    bubble.bubble().raw(),
                    function.symbol().raw(),
                    instruction.result().raw()
                );
                if value.is_empty() {
                    output.push_str(" UINT8_C(0)");
                } else {
                    for byte in value.as_bytes() {
                        let _ = write!(output, " UINT8_C(0x{byte:02X}),");
                    }
                }
                output.push_str(" };\n");
            }
        }
    }
    output.push('\n');
}

pub(crate) fn emit_checked_helper(output: &mut String, helper: CheckedHelper) {
    let ty = integer_c_type(helper.kind);
    let suffix = integer_suffix(helper.kind);
    let name = checked_helper_name(helper.operation, helper.kind);
    let minimum = integer_limit(helper.kind, false);
    let maximum = integer_limit(helper.kind, true);
    match (helper.operation, helper.kind.is_signed()) {
        (CheckedOperation::Add, true) => {
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} left, {ty} right)\n{{\n    if ((right > 0 && left > {maximum} - right) || (right < 0 && left < {minimum} - right)) pop_trap();\n    return ({ty})(left + right);\n}}\n"
            );
        }
        (CheckedOperation::Add, false) => {
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} left, {ty} right)\n{{\n    if (left > {maximum} - right) pop_trap();\n    return ({ty})(left + right);\n}}\n"
            );
        }
        (CheckedOperation::Subtract, true) => {
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} left, {ty} right)\n{{\n    if ((right < 0 && left > {maximum} + right) || (right > 0 && left < {minimum} + right)) pop_trap();\n    return ({ty})(left - right);\n}}\n"
            );
        }
        (CheckedOperation::Subtract, false) => {
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} left, {ty} right)\n{{\n    if (left < right) pop_trap();\n    return ({ty})(left - right);\n}}\n"
            );
        }
        (CheckedOperation::Multiply, true) => {
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} left, {ty} right)\n{{\n    if (left == 0 || right == 0) return 0;\n    if ((left == -1 && right == {minimum}) || (right == -1 && left == {minimum})) pop_trap();\n    if (left > 0) {{\n        if ((right > 0 && left > {maximum} / right) || (right < 0 && right < {minimum} / left)) pop_trap();\n    }} else if ((right > 0 && left < {minimum} / right) || (right < 0 && left < {maximum} / right)) {{\n        pop_trap();\n    }}\n    return ({ty})(left * right);\n}}\n"
            );
        }
        (CheckedOperation::Multiply, false) => {
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} left, {ty} right)\n{{\n    if (right != 0 && left > {maximum} / right) pop_trap();\n    return ({ty})(left * right);\n}}\n"
            );
        }
        (CheckedOperation::Divide | CheckedOperation::Remainder, signed) => {
            let operator = if helper.operation == CheckedOperation::Divide {
                "/"
            } else {
                "%"
            };
            let overflow = if signed {
                format!("\n    if (left == {minimum} && right == -1) pop_trap();")
            } else {
                String::new()
            };
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} left, {ty} right)\n{{\n    if (right == 0) pop_trap();{overflow}\n    return ({ty})(left {operator} right);\n}}\n"
            );
        }
        (CheckedOperation::Negate, true) => {
            let _ = writeln!(
                output,
                "static inline {ty} {name}({ty} value)\n{{\n    if (value == {minimum}) pop_trap();\n    return ({ty})(-value);\n}}\n"
            );
        }
        (CheckedOperation::Negate, false) => {
            let _ = suffix;
            unreachable!("verified MIR never negates unsigned integers")
        }
    }
}

const fn integer_suffix(kind: IntegerKind) -> &'static str {
    match kind {
        IntegerKind::Int8 => "i8",
        IntegerKind::Int16 => "i16",
        IntegerKind::Int32 => "i32",
        IntegerKind::Int64 => "i64",
        IntegerKind::UInt8 => "u8",
        IntegerKind::UInt16 => "u16",
        IntegerKind::UInt32 => "u32",
        IntegerKind::UInt64 => "u64",
    }
}

fn integer_limit(kind: IntegerKind, maximum: bool) -> String {
    let suffix = integer_suffix(kind).to_ascii_uppercase();
    if kind.is_signed() {
        format!(
            "INT{width}_{which}",
            width = kind.bit_width(),
            which = if maximum { "MAX" } else { "MIN" }
        )
    } else if maximum {
        format!("UINT{}_MAX", kind.bit_width())
    } else {
        let _ = suffix;
        "0".to_owned()
    }
}

fn checked_helper_name(operation: CheckedOperation, kind: IntegerKind) -> String {
    let operation = match operation {
        CheckedOperation::Add => "add",
        CheckedOperation::Subtract => "subtract",
        CheckedOperation::Multiply => "multiply",
        CheckedOperation::Divide => "divide",
        CheckedOperation::Remainder => "remainder",
        CheckedOperation::Negate => "negate",
    };
    format!("pop_checked_{operation}_{}", integer_suffix(kind))
}

fn emit_integer_conversion(
    output: &mut String,
    result: u32,
    source: IntegerKind,
    target: IntegerKind,
    operand: ValueId,
) {
    let value = format!("v{}", operand.raw());
    let condition = match (source.is_signed(), target.is_signed()) {
        (true, true) if target.bit_width() < source.bit_width() => Some(format!(
            "(intmax_t){value} < (intmax_t){} || (intmax_t){value} > (intmax_t){}",
            integer_limit(target, false),
            integer_limit(target, true)
        )),
        (true, false) if target.bit_width() < source.bit_width() => Some(format!(
            "{value} < 0 || (uintmax_t){value} > (uintmax_t){}",
            integer_limit(target, true)
        )),
        (true, false) => Some(format!("{value} < 0")),
        (false, true) if target.bit_width() <= source.bit_width() => Some(format!(
            "(uintmax_t){value} > (uintmax_t){}",
            integer_limit(target, true)
        )),
        (false, false) if target.bit_width() < source.bit_width() => Some(format!(
            "(uintmax_t){value} > (uintmax_t){}",
            integer_limit(target, true)
        )),
        _ => None,
    };
    if let Some(condition) = condition {
        let _ = writeln!(output, "    if ({condition}) pop_trap();");
    }
    let _ = writeln!(
        output,
        "    v{result} = ({}){value};",
        integer_c_type(target)
    );
}

fn emit_float_to_integer_conversion(
    output: &mut String,
    result: u32,
    source: FloatKind,
    target: IntegerKind,
    operand: ValueId,
) {
    let bits = target.bit_width();
    let mantissa_bits = match source {
        FloatKind::Float32 => 24,
        FloatKind::Float64 => 53,
    };
    let (lower_operator, lower) = if target.is_signed() {
        if bits < mantissa_bits {
            (">", format!("-{}.0L", (1_u128 << (bits - 1)) + 1))
        } else {
            (">=", format!("-{}.0L", 1_u128 << (bits - 1)))
        }
    } else {
        (">", "-1.0L".to_owned())
    };
    let upper = if target.is_signed() {
        1_u128 << (bits - 1)
    } else {
        1_u128 << bits
    };
    let value = format!("(long double)v{}", operand.raw());
    let _ = writeln!(
        output,
        "    if (!({value} {lower_operator} {lower} && {value} < {upper}.0L)) pop_trap();\n    v{result} = ({})v{};",
        integer_c_type(target),
        operand.raw()
    );
}

pub(crate) fn emit_function_declaration(
    output: &mut String,
    bubble: &MirBubble,
    function: &MirFunction,
    types: &TypeArena,
) -> Result<(), CBackendError> {
    write_function_header(output, bubble, function, types)?;
    output.push_str(";\n");
    Ok(())
}

fn write_function_header(
    output: &mut String,
    bubble: &MirBubble,
    function: &MirFunction,
    types: &TypeArena,
) -> Result<(), CBackendError> {
    output.push_str("static ");
    output.push_str(if let Some(result) = function.results().first() {
        c_type(*result, types)?
    } else {
        "void"
    });
    let _ = write!(
        output,
        " pop_b{}_s{}(",
        bubble.bubble().raw(),
        function.symbol().raw()
    );
    if function.parameters().is_empty() {
        output.push_str("void");
    } else {
        let entry = &function.blocks()[0];
        for (index, (type_id, argument)) in function
            .parameters()
            .iter()
            .zip(entry.arguments())
            .enumerate()
        {
            if index != 0 {
                output.push_str(", ");
            }
            let _ = write!(
                output,
                "{} v{}",
                c_type(*type_id, types)?,
                argument.value().raw()
            );
        }
    }
    output.push(')');
    Ok(())
}

pub(crate) fn emit_function(
    output: &mut String,
    bubble: &MirBubble,
    function: &MirFunction,
    types: &TypeArena,
) -> Result<(), CBackendError> {
    write_function_header(output, bubble, function, types)?;
    output.push_str("\n{\n");
    let parameter_values: BTreeSet<_> = function.blocks()[0]
        .arguments()
        .iter()
        .map(|argument| argument.value())
        .collect();
    for block in function.blocks() {
        for argument in block.arguments() {
            if !parameter_values.contains(&argument.value()) {
                let _ = writeln!(
                    output,
                    "    {} v{};",
                    c_type(argument.type_id(), types)?,
                    argument.value().raw()
                );
            }
        }
        for instruction in block.instructions() {
            if let Some(type_id) = instruction.optional_result_type() {
                let _ = writeln!(
                    output,
                    "    {} v{};",
                    c_type(type_id, types)?,
                    instruction.result().raw()
                );
            }
        }
    }
    output.push_str("    goto pop_b0;\n");
    for block in function.blocks() {
        let _ = writeln!(output, "pop_b{}:", block.block().raw());
        for argument in block.arguments() {
            let _ = writeln!(output, "    (void)v{};", argument.value().raw());
        }
        for instruction in block.instructions() {
            emit_instruction(output, bubble, function, instruction);
        }
        emit_terminator(output, function, block, types)?;
    }
    output.push_str("}\n\n");
    Ok(())
}

#[allow(clippy::too_many_lines)]
fn emit_instruction(
    output: &mut String,
    bubble: &MirBubble,
    function: &MirFunction,
    instruction: &pop_mir::MirInstruction,
) {
    let result = instruction.result().raw();
    match instruction.kind() {
        MirInstructionKind::IntegerConstant(value) => {
            let _ = writeln!(output, "    v{result} = {};", integer_literal(*value));
        }
        MirInstructionKind::FloatConstant(value) => {
            let _ = writeln!(output, "    v{result} = {};", float_literal(*value));
        }
        MirInstructionKind::StringConstant(value) => {
            let _ = writeln!(
                output,
                "    v{result} = (pop_string){{ pop_b{}_s{}_v{result}_bytes, (size_t){} }};",
                bubble.bubble().raw(),
                function.symbol().raw(),
                value.len()
            );
        }
        MirInstructionKind::BooleanConstant(value) => {
            let _ = writeln!(
                output,
                "    v{result} = {};",
                if *value { "true" } else { "false" }
            );
        }
        MirInstructionKind::CheckedIntegerAdd { kind, left, right }
        | MirInstructionKind::CheckedIntegerSubtract { kind, left, right }
        | MirInstructionKind::CheckedIntegerMultiply { kind, left, right }
        | MirInstructionKind::CheckedIntegerDivide { kind, left, right }
        | MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => {
            let operation = match instruction.kind() {
                MirInstructionKind::CheckedIntegerAdd { .. } => CheckedOperation::Add,
                MirInstructionKind::CheckedIntegerSubtract { .. } => CheckedOperation::Subtract,
                MirInstructionKind::CheckedIntegerMultiply { .. } => CheckedOperation::Multiply,
                MirInstructionKind::CheckedIntegerDivide { .. } => CheckedOperation::Divide,
                _ => CheckedOperation::Remainder,
            };
            let _ = writeln!(
                output,
                "    v{result} = {}(v{}, v{});",
                checked_helper_name(operation, *kind),
                left.raw(),
                right.raw()
            );
        }
        MirInstructionKind::IntegerNegate { kind, operand } => {
            let _ = writeln!(
                output,
                "    v{result} = {}(v{});",
                checked_helper_name(CheckedOperation::Negate, *kind),
                operand.raw()
            );
        }
        MirInstructionKind::ConvertInteger {
            source,
            target,
            operand,
        } => emit_integer_conversion(output, result, *source, *target, *operand),
        MirInstructionKind::ConvertIntegerToFloat {
            target, operand, ..
        } => {
            let _ = writeln!(
                output,
                "    v{result} = ({})v{};",
                match target {
                    FloatKind::Float32 => "float",
                    FloatKind::Float64 => "double",
                },
                operand.raw()
            );
        }
        MirInstructionKind::ConvertFloatToInteger {
            source,
            target,
            operand,
        } => emit_float_to_integer_conversion(output, result, *source, *target, *operand),
        MirInstructionKind::ConvertFloat {
            target, operand, ..
        } => {
            let _ = writeln!(
                output,
                "    v{result} = ({})v{};",
                match target {
                    FloatKind::Float32 => "float",
                    FloatKind::Float64 => "double",
                },
                operand.raw()
            );
        }
        MirInstructionKind::FloatAdd { left, right, .. } => {
            emit_binary(output, result, *left, "+", *right);
        }
        MirInstructionKind::FloatSubtract { left, right, .. } => {
            emit_binary(output, result, *left, "-", *right);
        }
        MirInstructionKind::FloatMultiply { left, right, .. } => {
            emit_binary(output, result, *left, "*", *right);
        }
        MirInstructionKind::FloatDivide { left, right, .. } => {
            emit_binary(output, result, *left, "/", *right);
        }
        MirInstructionKind::BooleanAnd { left, right } => {
            emit_binary(output, result, *left, "&&", *right);
        }
        MirInstructionKind::BooleanOr { left, right } => {
            emit_binary(output, result, *left, "||", *right);
        }
        MirInstructionKind::CompareEqual { left, right } => {
            emit_binary(output, result, *left, "==", *right);
        }
        MirInstructionKind::CompareNotEqual { left, right } => {
            emit_binary(output, result, *left, "!=", *right);
        }
        MirInstructionKind::CompareIntegerLess { left, right, .. }
        | MirInstructionKind::CompareIntegerLessOrEqual { left, right, .. }
        | MirInstructionKind::CompareFloatLess { left, right, .. } => {
            emit_binary(
                output,
                result,
                *left,
                if matches!(
                    instruction.kind(),
                    MirInstructionKind::CompareIntegerLessOrEqual { .. }
                        | MirInstructionKind::CompareFloatLessOrEqual { .. }
                ) {
                    "<="
                } else {
                    "<"
                },
                *right,
            );
        }
        MirInstructionKind::CompareFloatLessOrEqual { left, right, .. } => {
            emit_binary(output, result, *left, "<=", *right);
        }
        MirInstructionKind::CompareIntegerGreater { left, right, .. }
        | MirInstructionKind::CompareIntegerGreaterOrEqual { left, right, .. }
        | MirInstructionKind::CompareFloatGreater { left, right, .. } => {
            emit_binary(
                output,
                result,
                *left,
                if matches!(
                    instruction.kind(),
                    MirInstructionKind::CompareIntegerGreaterOrEqual { .. }
                        | MirInstructionKind::CompareFloatGreaterOrEqual { .. }
                ) {
                    ">="
                } else {
                    ">"
                },
                *right,
            );
        }
        MirInstructionKind::CompareFloatGreaterOrEqual { left, right, .. } => {
            emit_binary(output, result, *left, ">=", *right);
        }
        MirInstructionKind::BooleanNot { operand } => {
            let _ = writeln!(output, "    v{result} = !v{};", operand.raw());
        }
        MirInstructionKind::FloatNegate { operand, .. } => {
            let _ = writeln!(output, "    v{result} = -v{};", operand.raw());
        }
        MirInstructionKind::CallDirect {
            function,
            arguments,
            ..
        } => {
            output.push_str("    ");
            if instruction.has_result() {
                let _ = write!(output, "v{result} = ");
            }
            let _ = write!(
                output,
                "pop_b{}_s{}(",
                bubble.bubble().raw(),
                function.raw()
            );
            emit_values(output, arguments);
            output.push_str(");\n");
        }
        MirInstructionKind::CallStandard {
            function,
            arguments,
            ..
        } => {
            let adapter = match function.raw() {
                0 => "pop_print_int",
                1 => "pop_print_string",
                _ => unreachable!("C standard call was validated before rendering"),
            };
            let _ = write!(output, "    {adapter}(");
            emit_values(output, arguments);
            output.push_str(");\n");
        }
        _ => unreachable!("C instruction was validated before rendering"),
    }
    if instruction.has_result() {
        let _ = writeln!(output, "    (void)v{result};");
    }
}

fn emit_binary(output: &mut String, result: u32, left: ValueId, operator: &str, right: ValueId) {
    let _ = writeln!(
        output,
        "    v{result} = v{} {operator} v{};",
        left.raw(),
        right.raw()
    );
}

fn emit_terminator(
    output: &mut String,
    function: &MirFunction,
    block: &pop_mir::MirBlock,
    types: &TypeArena,
) -> Result<(), CBackendError> {
    match block.terminator() {
        MirTerminator::Branch { target, arguments } => {
            emit_branch(output, block.block(), *target, arguments, function, types)?;
        }
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => {
            let _ = writeln!(
                output,
                "    if (v{}) goto pop_b{};",
                condition.raw(),
                when_true.raw()
            );
            let _ = writeln!(output, "    goto pop_b{};", when_false.raw());
        }
        MirTerminator::Return { values } => {
            if let Some(value) = values.first() {
                let _ = writeln!(output, "    return v{};", value.raw());
            } else {
                output.push_str("    return;\n");
            }
        }
        MirTerminator::Trap(_) | MirTerminator::Unreachable => output.push_str("    pop_trap();\n"),
        _ => unreachable!("C terminator was validated before rendering"),
    }
    Ok(())
}

fn emit_branch(
    output: &mut String,
    source: BlockId,
    target: BlockId,
    arguments: &[ValueId],
    function: &MirFunction,
    types: &TypeArena,
) -> Result<(), CBackendError> {
    let target_block = &function.blocks()[target.raw() as usize];
    if arguments.is_empty() {
        let _ = writeln!(output, "    goto pop_b{};", target.raw());
        return Ok(());
    }
    output.push_str("    {\n");
    for (index, (value, argument)) in arguments.iter().zip(target_block.arguments()).enumerate() {
        let _ = writeln!(
            output,
            "        {} edge_b{}_b{}_{} = v{};",
            c_type(argument.type_id(), types)?,
            source.raw(),
            target.raw(),
            index,
            value.raw()
        );
    }
    for (index, argument) in target_block.arguments().iter().enumerate() {
        let _ = writeln!(
            output,
            "        v{} = edge_b{}_b{}_{};",
            argument.value().raw(),
            source.raw(),
            target.raw(),
            index
        );
    }
    let _ = writeln!(output, "        goto pop_b{};", target.raw());
    output.push_str("    }\n");
    Ok(())
}

pub(crate) fn emit_entry(
    output: &mut String,
    bubble: &MirBubble,
    types: &TypeArena,
    entry: SymbolId,
) -> Result<(), CBackendError> {
    let function = bubble
        .functions()
        .iter()
        .find(|function| function.symbol() == entry)
        .ok_or(CBackendError::InvalidEntryPoint(entry))?;

    if function.results().is_empty() {
        output.push_str("int main(void)\n{\n");

        let _ = writeln!(
            output,
            "    pop_b{}_s{}();",
            bubble.bubble().raw(),
            entry.raw()
        );

        output.push_str("    return 0;\n}\n");
    } else {
        let int = types
            .source_type("Int")
            .ok_or(CBackendError::UnsupportedEntryPointSignature(entry))?;

        if function.results() != [int] {
            return Err(CBackendError::UnsupportedEntryPointSignature(entry));
        }

        output.push_str(
            "static int pop_process_status(int64_t status)\n\
             {\n\
                 uint32_t bits = (uint32_t)(uint64_t)status;\n\
                 if (bits <= (uint32_t)INT32_MAX) return (int)bits;\n\
                 return -1 - (int)(UINT32_MAX - bits);\n\
             }\n\n\
             int main(void)\n\
             {\n",
        );

        let _ = writeln!(
            output,
            "    return pop_process_status(pop_b{}_s{}());",
            bubble.bubble().raw(),
            entry.raw()
        );

        output.push_str("}\n");
    }

    Ok(())
}

fn emit_values(output: &mut String, values: &[ValueId]) {
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "v{}", value.raw());
    }
}

fn integer_literal(value: IntegerValue) -> String {
    if let Some(signed) = value.signed() {
        let minimum = -(1_i128 << (value.kind().bit_width() - 1));
        if i128::from(signed) == minimum {
            return integer_limit(value.kind(), false);
        }
        if value.kind() == IntegerKind::Int64 {
            format!("INT64_C({signed})")
        } else {
            format!(
                "({})INT{}_C({signed})",
                integer_c_type(value.kind()),
                value.kind().bit_width()
            )
        }
    } else {
        let unsigned = value.unsigned().expect("unsigned integer value");
        if value.kind() == IntegerKind::UInt64 {
            format!("UINT64_C({unsigned})")
        } else {
            format!(
                "({})UINT{}_C({unsigned})",
                integer_c_type(value.kind()),
                value.kind().bit_width()
            )
        }
    }
}

fn float_literal(value: FloatValue) -> String {
    match value.kind() {
        FloatKind::Float32 => format!("pop_float32_from_bits(UINT32_C(0x{:08X}))", value.bits()),
        FloatKind::Float64 => format!("pop_float64_from_bits(UINT64_C(0x{:016X}))", value.bits()),
    }
}
