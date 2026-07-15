//! Canonical MIR instruction and terminator lowering into private LLVM text.
//!
//! Checked arithmetic, runtime calls, aggregates, collections, closures, GC
//! operations, and physical types are isolated here. Nothing in this module
//! is a canonical MIR instruction or a source-language semantic rule.

use pop_foundation::{BubbleId, ClassId, FieldId, FunctionId, SymbolId, TypeId, ValueId};
use pop_mir::{MirInstructionKind, MirTerminator};
use pop_runtime_interface::{ArrayElementMap, RuntimeOperation};
use pop_runtime_native_abi::{IterationCollectionKind, IterationStatus};
use pop_types::{FloatKind, IntegerKind, PrimitiveType, SemanticType, TypeArena};
use std::collections::{BTreeMap, BTreeSet};

use crate::api::{LlvmLoweringError, LlvmLoweringOptions};
use crate::lowering::*;

pub(crate) fn lower_instruction(
    bubble: BubbleId,
    instruction: &pop_mir::MirInstruction,
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    string_literals: &BTreeMap<String, String>,
    environment: Option<(&str, &BTreeSet<u32>)>,
    proven_non_overflow_adds: &BTreeSet<ValueId>,
    direct_scalar_arrays: &DirectScalarArrays,
    options: LlvmLoweringOptions,
) -> Result<String, LlvmLoweringError> {
    let result = format!("%v{}", instruction.result().raw());
    let result_type = instruction.optional_result_type();
    let line = match instruction.kind() {
        MirInstructionKind::IntegerConstant(value) => format!(
            "{result} = add i{} 0, {}",
            value.kind().bit_width(),
            integer_literal(*value)
        ),
        MirInstructionKind::FloatConstant(value) => format!(
            "{result} = fadd {} 0.0, 0x{:016X}",
            float_type(value.kind()),
            value.as_f64().to_bits()
        ),
        MirInstructionKind::BooleanConstant(value) => {
            format!("{result} = xor i1 0, {}", u8::from(*value))
        }
        MirInstructionKind::NilConstant => {
            if let Some(inner) = optional_inner_type(types, instruction.result_type()) {
                let inner = llvm_type(inner, types)?;
                format!("{result} = insertvalue {{ i1, {inner} }} zeroinitializer, i1 false, 0")
            } else {
                format!("{result} = add i64 0, 0")
            }
        }
        MirInstructionKind::OptionalIsPresent { optional } => {
            let optional_type = value_types
                .get(optional)
                .copied()
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let inner = optional_inner_type(types, optional_type)
                .ok_or(LlvmLoweringError::InvalidType(optional_type))?;
            let inner = llvm_type(inner, types)?;
            format!(
                "{result} = extractvalue {{ i1, {inner} }} %v{}, 0",
                optional.raw()
            )
        }
        MirInstructionKind::OptionalGet { optional } => {
            let optional_type = value_types
                .get(optional)
                .copied()
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let inner = optional_inner_type(types, optional_type)
                .ok_or(LlvmLoweringError::InvalidType(optional_type))?;
            let inner = llvm_type(inner, types)?;
            format!(
                "{result} = extractvalue {{ i1, {inner} }} %v{}, 1",
                optional.raw()
            )
        }
        MirInstructionKind::ResultMake {
            case, arguments, ..
        } => lower_union_make(
            &result,
            pop_foundation::UnionCaseId::from_raw(case.raw()),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::IterationMake {
            case, arguments, ..
        } => lower_union_make(
            &result,
            pop_foundation::UnionCaseId::from_raw(case.raw()),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::ErrorMake {
            case, arguments, ..
        } => lower_union_make(
            &result,
            pop_foundation::UnionCaseId::from_raw(case.raw()),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::ResultIsOk { result: value, .. } => format!(
            "{result}_tag = call i64 @{}(i64 %v{}, i64 1)\n{result} = icmp eq i64 {result}_tag, 0",
            native_runtime_symbol(RuntimeOperation::FieldGet),
            value.raw()
        ),
        MirInstructionKind::ResultGetOk { result: value, .. }
        | MirInstructionKind::ResultGetError { result: value, .. } => lower_runtime_slot_load_from(
            instruction.result(),
            instruction.result_type(),
            &format!("%v{}", value.raw()),
            2,
            types,
        )?
        .join("\n"),
        MirInstructionKind::EnumConstant { discriminant, .. } => {
            format!("{result} = add i32 0, {discriminant}")
        }
        MirInstructionKind::StringConstant(value) => {
            let symbol = string_literals
                .get(value)
                .ok_or(LlvmLoweringError::InvalidType(instruction.result_type()))?;
            format!(
                "{result} = call i64 @pop_rt_string_literal(ptr {symbol}, i64 {})",
                value.len()
            )
        }
        MirInstructionKind::StringConcat { left, right } => format!(
            "{result} = call i64 @{}(i64 %v{}, i64 %v{})",
            native_runtime_symbol(RuntimeOperation::StringConcat),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::StringFormat { kind, value } => {
            lower_string_format(&result, instruction.result(), *kind, *value)
        }
        MirInstructionKind::CheckedIntegerAdd { kind, left, right } => {
            if proven_non_overflow_adds.contains(&instruction.result()) {
                format!(
                    "{result} = add nsw i{} %v{}, %v{}",
                    kind.bit_width(),
                    left.raw(),
                    right.raw()
                )
            } else {
                lower_checked_integer_binary(&result, "add", *kind, *left, *right)
            }
        }
        MirInstructionKind::CheckedIntegerSubtract { kind, left, right } => {
            lower_checked_integer_binary(&result, "sub", *kind, *left, *right)
        }
        MirInstructionKind::CheckedIntegerMultiply { kind, left, right } => {
            lower_checked_integer_binary(&result, "mul", *kind, *left, *right)
        }
        MirInstructionKind::CheckedIntegerDivide { kind, left, right } => {
            lower_checked_integer_division(&result, "div", *kind, *left, *right)
        }
        MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => {
            lower_checked_integer_division(&result, "rem", *kind, *left, *right)
        }
        MirInstructionKind::IntegerNegate { kind, operand } => {
            lower_checked_integer_negate(&result, *kind, *operand)
        }
        MirInstructionKind::FloatAdd { kind, left, right } => format!(
            "{result} = fadd {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatSubtract { kind, left, right } => format!(
            "{result} = fsub {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatMultiply { kind, left, right } => format!(
            "{result} = fmul {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatDivide { kind, left, right } => format!(
            "{result} = fdiv {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatNegate { kind, operand } => {
            format!("{result} = fneg {} %v{}", float_type(*kind), operand.raw())
        }
        MirInstructionKind::ConvertInteger {
            source,
            target,
            operand,
        } => lower_integer_conversion(&result, *source, *target, *operand),
        MirInstructionKind::ConvertIntegerToFloat {
            source,
            target,
            operand,
        } => format!(
            "{result} = {} i{} %v{} to {}",
            if source.is_signed() {
                "sitofp"
            } else {
                "uitofp"
            },
            source.bit_width(),
            operand.raw(),
            float_type(*target)
        ),
        MirInstructionKind::ConvertFloatToInteger {
            source,
            target,
            operand,
        } => lower_float_to_integer_conversion(&result, *source, *target, *operand),
        MirInstructionKind::ConvertFloat {
            source,
            target,
            operand,
        } => lower_float_conversion(&result, *source, *target, *operand),
        MirInstructionKind::BooleanNot { operand } => {
            format!("{result} = xor i1 %v{}, true", operand.raw())
        }
        MirInstructionKind::BooleanAnd { left, right } => {
            format!("{result} = and i1 %v{}, %v{}", left.raw(), right.raw())
        }
        MirInstructionKind::BooleanOr { left, right } => {
            format!("{result} = or i1 %v{}, %v{}", left.raw(), right.raw())
        }
        MirInstructionKind::CompareEqual { left, right } => lower_equality(
            &result,
            *left,
            *right,
            false,
            value_types,
            types,
            record_field_types,
        )?,
        MirInstructionKind::CompareNotEqual { left, right } => lower_equality(
            &result,
            *left,
            *right,
            true,
            value_types,
            types,
            record_field_types,
        )?,
        MirInstructionKind::CompareIntegerLess { kind, left, right } => format!(
            "{result} = icmp {} i{} %v{}, %v{}",
            if kind.is_signed() { "slt" } else { "ult" },
            kind.bit_width(),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareIntegerGreater { kind, left, right } => format!(
            "{result} = icmp {} i{} %v{}, %v{}",
            if kind.is_signed() { "sgt" } else { "ugt" },
            kind.bit_width(),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareIntegerLessOrEqual { kind, left, right } => format!(
            "{result} = icmp {} i{} %v{}, %v{}",
            if kind.is_signed() { "sle" } else { "ule" },
            kind.bit_width(),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareIntegerGreaterOrEqual { kind, left, right } => format!(
            "{result} = icmp {} i{} %v{}, %v{}",
            if kind.is_signed() { "sge" } else { "uge" },
            kind.bit_width(),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareFloatLess { kind, left, right } => format!(
            "{result} = fcmp olt {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareFloatGreater { kind, left, right } => format!(
            "{result} = fcmp ogt {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareFloatLessOrEqual { kind, left, right } => format!(
            "{result} = fcmp ole {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareFloatGreaterOrEqual { kind, left, right } => format!(
            "{result} = fcmp oge {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FunctionReference(symbol) => {
            let mut lines = lower_mapped_allocation(&result, 1, &[]);
            lines.push(format!(
                "call i8 @{}(i64 {result}, i64 1, i64 {})",
                native_runtime_symbol(RuntimeOperation::FieldSet),
                direct_function_tag(*symbol)
            ));
            lines.join("\n")
        }
        MirInstructionKind::TaskCreate {
            dispatch,
            arguments,
            ..
        } => {
            let mut call_arguments = Vec::new();
            let callee = match dispatch {
                pop_mir::MirTaskDispatch::Direct(function) => {
                    async_function_create_name(bubble, *function)
                }
                pop_mir::MirTaskDispatch::Referenced(function) => {
                    async_function_create_name(function.bubble(), function.symbol())
                }
                pop_mir::MirTaskDispatch::Indirect(callee) => {
                    let function_type = value_types
                        .get(callee)
                        .copied()
                        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
                    call_arguments.push(format!("i64 %v{}", callee.raw()));
                    async_indirect_create_name(bubble, function_type)
                }
            };
            call_arguments.extend(
                arguments
                    .iter()
                    .map(|argument| {
                        llvm_value_type(value_types, *argument, types)
                            .map(|ty| format!("{ty} %v{}", argument.raw()))
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            );
            call_arguments.push("i64 0".to_owned());
            let label = result.trim_start_matches('%');
            format!(
                "{result}_created = call i64 @{callee}({})\n{result}_valid = icmp ne i64 {result}_created, 0\nbr i1 {result}_valid, label %{label}_ready, label %{label}_trap\n{label}_trap:\n  call void @{}()\n  unreachable\n{label}_ready:\n  {result} = add i64 {result}_created, 0",
                call_arguments.join(", "),
                native_runtime_symbol(RuntimeOperation::Trap),
            )
        }
        MirInstructionKind::CancelSourceCreate => format!(
            "{result} = call i64 @{}()",
            native_runtime_symbol(RuntimeOperation::CancelSourceCreate)
        ),
        MirInstructionKind::CancelSourceToken { source } => format!(
            "{result} = call i64 @{}(i64 %v{})",
            native_runtime_symbol(RuntimeOperation::CancelSourceToken),
            source.raw()
        ),
        MirInstructionKind::CancelRequest { source } => format!(
            "{result}_requested = call i8 @{}(i64 %v{})\n{result} = add i64 0, 0",
            native_runtime_symbol(RuntimeOperation::TaskCancel),
            source.raw()
        ),
        MirInstructionKind::TaskGroupCreate {
            cancel,
            body,
            completion_type,
            ..
        } => {
            let body_type = value_types
                .get(body)
                .copied()
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let create = async_indirect_create_name(bubble, body_type);
            let label = result.trim_start_matches('%');
            format!(
                "{result}_group = call i64 @{}(i64 %v{})\n{result}_group_valid = icmp ne i64 {result}_group, 0\nbr i1 {result}_group_valid, label %{label}_group_ready, label %{label}_trap\n{label}_group_ready:\n{result}_group_root = call i64 @{}(i64 {result}_group)\n{result}_group_root_valid = icmp ne i64 {result}_group_root, 0\nbr i1 {result}_group_root_valid, label %{label}_body_create, label %{label}_trap\n{label}_body_create:\n{result}_body = call i64 @{create}(i64 %v{}, i64 {result}_group, i64 %v{})\n{result}_body_valid = icmp ne i64 {result}_body, 0\nbr i1 {result}_body_valid, label %{label}_body_ready, label %{label}_trap\n{label}_body_ready:\n{result}_body_root = call i64 @{}(i64 {result}_body)\n{result}_body_root_valid = icmp ne i64 {result}_body_root, 0\nbr i1 {result}_body_root_valid, label %{label}_wrap, label %{label}_trap\n{label}_wrap:\n{result}_wrapped = call i64 @{}(i64 {result}_group, i64 {result}_body, i8 {})\n{result}_body_root_released = call i8 @{}(i64 {result}_body_root)\n{result}_group_root_released = call i8 @{}(i64 {result}_group_root)\n{result}_wrapped_valid = icmp ne i64 {result}_wrapped, 0\n{result}_body_release_valid = icmp eq i8 {result}_body_root_released, 1\n{result}_group_release_valid = icmp eq i8 {result}_group_root_released, 1\n{result}_release_valid = and i1 {result}_body_release_valid, {result}_group_release_valid\n{result}_all_valid = and i1 {result}_wrapped_valid, {result}_release_valid\nbr i1 {result}_all_valid, label %{label}_ready, label %{label}_trap\n{label}_trap:\n  call void @{}()\n  unreachable\n{label}_ready:\n  {result} = add i64 {result}_wrapped, 0",
                native_runtime_symbol(RuntimeOperation::TaskGroupCreate),
                cancel.raw(),
                native_runtime_symbol(RuntimeOperation::RetainRoot),
                body.raw(),
                cancel.raw(),
                native_runtime_symbol(RuntimeOperation::RetainRoot),
                native_runtime_symbol(RuntimeOperation::TaskGroupWrap),
                u8::from(is_managed_type(*completion_type, types)),
                native_runtime_symbol(RuntimeOperation::ReleaseRoot),
                native_runtime_symbol(RuntimeOperation::ReleaseRoot),
                native_runtime_symbol(RuntimeOperation::Trap),
            )
        }
        MirInstructionKind::TaskStart { group, task } => {
            let label = format!("v{}_task_start", instruction.result().raw());
            format!(
                "{result}_started = call i8 @{}(i64 %v{}, i64 %v{})\n{result}_valid = icmp eq i8 {result}_started, 1\nbr i1 {result}_valid, label %{label}_valid, label %{label}_trap\n{label}_trap:\n  call void @{}()\n  unreachable\n{label}_valid:\n  {result} = add i64 %v{}, 0",
                native_runtime_symbol(RuntimeOperation::TaskStartGroup),
                group.raw(),
                task.raw(),
                native_runtime_symbol(RuntimeOperation::Trap),
                task.raw(),
            )
        }
        MirInstructionKind::CallDirect {
            function: callee,
            arguments,
            ..
        } => call_line(
            &result,
            result_type,
            &format!("@{}", function_name(bubble, *callee)),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallReferenced {
            function: callee,
            arguments,
            ..
        } => call_line(
            &result,
            result_type,
            &format!("@{}", function_name(callee.bubble(), callee.symbol())),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallStandard {
            function,
            arguments,
            ..
        } => {
            if arguments.len() != 1 {
                return Err(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                });
            }
            match function.raw() {
                0 => format!("call void @pop_std_print_int(i64 %v{})", arguments[0].raw()),
                1 => format!(
                    "call void @pop_std_print_string(i64 %v{})",
                    arguments[0].raw()
                ),
                _ => {
                    return Err(LlvmLoweringError::UnsupportedInstruction {
                        function: FunctionId::from_raw(u32::MAX),
                        value: instruction.result(),
                    });
                }
            }
        }
        MirInstructionKind::GcSafePoint {
            safe_point, roots, ..
        } => lower_gc_safe_point(
            &result,
            safe_point.raw(),
            roots,
            direct_scalar_arrays,
            matches!(
                options.runtime_profile,
                pop_backend_api::RuntimeProfile::ProductionGenerational
            ),
            options.gc_poll_interval.get(),
        ),
        MirInstructionKind::RetainRoot { value } => format!(
            "{result} = call i64 @{}(i64 %v{})",
            native_runtime_symbol(RuntimeOperation::RetainRoot),
            value.raw()
        ),
        MirInstructionKind::ReleaseRoot { handle } => format!(
            "call i8 @{}(i64 %v{})",
            native_runtime_symbol(RuntimeOperation::ReleaseRoot),
            handle.raw()
        ),
        MirInstructionKind::Pin { value } => format!(
            "{result} = call i64 @{}(i64 %v{})",
            native_runtime_symbol(RuntimeOperation::Pin),
            value.raw()
        ),
        MirInstructionKind::Unpin { handle } => format!(
            "call i8 @{}(i64 %v{})",
            native_runtime_symbol(RuntimeOperation::Unpin),
            handle.raw()
        ),
        MirInstructionKind::WriteBarrier { proof: Some(_), .. } => {
            "; verified managed write barrier elided".to_owned()
        }
        MirInstructionKind::WriteBarrier {
            owner, proof: None, ..
        } => format!(
            "call void @{}(i64 %v{})",
            native_runtime_symbol(RuntimeOperation::SatbWriteBarrier),
            owner.raw()
        ),
        MirInstructionKind::CaptureCellAllocate { initial, .. } => {
            lower_capture_cell_allocate(&result, *initial, value_types, types)?
        }
        MirInstructionKind::ClosureEnvironmentAllocate {
            owner,
            function,
            captures,
            ..
        } => lower_closure_environment_allocate(
            &result,
            *owner,
            *function,
            captures,
            value_types,
            types,
        )?,
        MirInstructionKind::ArrayMake {
            elements,
            element_map,
        } => lower_array_make(&result, elements, *element_map, value_types, types)?,
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            element_map,
        } => {
            if let Some((origin, allocation)) =
                direct_scalar_arrays.allocation(instruction.result())
            {
                debug_assert_eq!(origin, instruction.result());
                lower_direct_array_create(bubble, &result, allocation, value_types, types)?
            } else {
                lower_array_create(
                    &result,
                    *length,
                    *initial_value,
                    *element_map,
                    value_types,
                    types,
                )?
            }
        }
        MirInstructionKind::TableMake {
            entries,
            key_map,
            value_map,
        } => lower_table_make(&result, entries, *key_map, *value_map, value_types, types)?,
        MirInstructionKind::TableGet { table, key } => lower_table_get(
            &result,
            *table,
            *key,
            instruction.result_type(),
            value_types,
            types,
        )?,
        MirInstructionKind::TableSet {
            table,
            key,
            value,
            key_map,
            value_map,
        } => lower_table_set(
            &result,
            *table,
            *key,
            *value,
            *key_map,
            *value_map,
            value_types,
            types,
        )?,
        MirInstructionKind::RecordMake { fields, .. } => {
            let slot_count = u32::try_from(fields.len())
                .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            lower_object_make(
                &result,
                fields,
                slot_count,
                value_types,
                types,
                field_layout,
            )?
        }
        MirInstructionKind::ClassMake {
            class,
            fields,
            object_map,
        } => lower_class_make(
            &result,
            *class,
            fields,
            object_map.slot_count() + 1,
            value_types,
            types,
            field_layout,
        )?,
        MirInstructionKind::CallDirectMethod {
            method, arguments, ..
        } => call_line(
            &result,
            result_type,
            &format!("@{}", method_name(bubble, *method)),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallInterface {
            interface,
            method,
            arguments,
            ..
        } => call_line(
            &result,
            result_type,
            &format!("@{}", interface_name(bubble, *interface, *method)),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallBuiltinInterface {
            method, arguments, ..
        } => {
            let receiver_type = arguments
                .first()
                .and_then(|receiver| value_types.get(receiver))
                .copied();
            let protocol = pop_types::embedded_bootstrap_schema()
                .ok()
                .and_then(|schema| schema.iteration_protocol());
            if receiver_type.is_some_and(|receiver| {
                matches!(
                    (types.get(receiver), protocol),
                    (Some(SemanticType::Builtin { definition, .. }), Some(protocol))
                        if *definition == protocol.iterator()
                )
            }) {
                call_line(
                    &result,
                    result_type,
                    &format!(
                        "@{}",
                        builtin_interface_name(bubble, receiver_type.expect("checked"), *method)
                    ),
                    arguments,
                    value_types,
                    types,
                )?
            } else {
                lower_builtin_iteration_call(
                    &result,
                    instruction.result_type(),
                    *method,
                    arguments,
                    value_types,
                    types,
                )?
            }
        }
        MirInstructionKind::CallIndirect {
            callee, arguments, ..
        } => {
            let callee_type = value_types
                .get(callee)
                .copied()
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let arguments = std::iter::once(*callee)
                .chain(arguments.iter().copied())
                .collect::<Vec<_>>();
            call_line(
                &result,
                result_type,
                &format!("@{}", indirect_name(bubble, callee_type)),
                &arguments,
                value_types,
                types,
            )?
        }
        MirInstructionKind::TupleMake(elements) => {
            lower_tuple_make(&result, elements, value_types, types)?
        }
        MirInstructionKind::TupleGet { tuple, index } => lower_runtime_slot_load_from(
            instruction.result(),
            instruction.result_type(),
            &format!("%v{}", tuple.raw()),
            usize::try_from(*index).unwrap_or(usize::MAX) + 1,
            types,
        )?
        .join("\n"),
        MirInstructionKind::ArrayGet { array, index } => lower_optional_array_get(
            &result,
            *array,
            *index,
            instruction.result_type(),
            value_types,
            types,
        )?,
        MirInstructionKind::ArrayLength { array } => {
            if let Some((_, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_length(&result, allocation)
            } else {
                lower_array_output_call(
                    &result,
                    instruction.result_type(),
                    RuntimeOperation::ArrayLength,
                    &[*array],
                    value_types,
                    types,
                )?
            }
        }
        MirInstructionKind::ArrayGetChecked { array, index } => {
            if let Some((origin, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_get(
                    &result,
                    origin,
                    allocation,
                    *index,
                    instruction.result_type(),
                    types,
                )?
            } else {
                lower_array_output_call(
                    &result,
                    instruction.result_type(),
                    RuntimeOperation::ArrayGetChecked,
                    &[*array, *index],
                    value_types,
                    types,
                )?
            }
        }
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            ..
        } => {
            if let Some((origin, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_set(
                    &result,
                    origin,
                    allocation,
                    *index,
                    *value,
                    value_types,
                    types,
                )?
            } else {
                lower_array_set(&result, *array, *index, *value, value_types, types)?
            }
        }
        MirInstructionKind::ArrayFill { array, value, .. } => {
            if let Some((origin, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_fill(
                    bubble,
                    &result,
                    origin,
                    allocation,
                    *value,
                    value_types,
                    types,
                )?
            } else {
                lower_array_fill(&result, *array, *value, value_types, types)?
            }
        }
        MirInstructionKind::ListCreate {
            capacity,
            element_map,
        } => lower_list_create(&result, *capacity, *element_map),
        MirInstructionKind::ListLength { list } => lower_array_output_call(
            &result,
            instruction.result_type(),
            RuntimeOperation::ListLength,
            &[*list],
            value_types,
            types,
        )?,
        MirInstructionKind::ListGet { list, index } => lower_optional_collection_get(
            &result,
            *list,
            *index,
            instruction.result_type(),
            RuntimeOperation::ListGet,
            value_types,
            types,
        )?,
        MirInstructionKind::ListGetChecked { list, index } => lower_array_output_call(
            &result,
            instruction.result_type(),
            RuntimeOperation::ListGetChecked,
            &[*list, *index],
            value_types,
            types,
        )?,
        MirInstructionKind::ListSet {
            list,
            index,
            value,
            element_map,
        } => lower_list_mutation(
            &result,
            RuntimeOperation::ListSet,
            *list,
            Some(*index),
            *value,
            *element_map,
            value_types,
            types,
        )?,
        MirInstructionKind::ListAdd {
            list,
            value,
            element_map,
        } => lower_list_mutation(
            &result,
            RuntimeOperation::ListAdd,
            *list,
            None,
            *value,
            *element_map,
            value_types,
            types,
        )?,
        MirInstructionKind::RangeCreate { first, last, step } => lower_range_create(
            &result,
            instruction.result_type(),
            *first,
            *last,
            *step,
            types,
        )?,
        MirInstructionKind::RecordUpdate {
            record,
            base,
            fields,
        } => lower_record_update(
            &result,
            *record,
            *base,
            fields,
            record_fields,
            record_field_types,
            field_layout,
            value_types,
            types,
        )?,
        MirInstructionKind::FieldGet { base, field } => runtime_field_call(
            &result,
            result_type,
            RuntimeOperation::FieldGet,
            *base,
            *field,
            None,
            value_types,
            types,
            field_layout,
        )?,
        MirInstructionKind::FieldSet { base, field, value } => runtime_field_call(
            &result,
            result_type,
            RuntimeOperation::FieldSet,
            *base,
            *field,
            Some(*value),
            value_types,
            types,
            field_layout,
        )?,
        MirInstructionKind::UnionMake {
            case, arguments, ..
        } => lower_union_make(&result, *case, arguments, value_types, types)?,
        MirInstructionKind::IterationIsItem { iteration, .. } => format!(
            "{result}_tag = call i64 @{}(i64 %v{}, i64 1)\n{result} = icmp eq i64 {result}_tag, 0",
            native_runtime_symbol(RuntimeOperation::FieldGet),
            iteration.raw()
        ),
        MirInstructionKind::IterationGetItem { iteration, .. } => lower_runtime_slot_load_named(
            &result,
            instruction.result_type(),
            &format!("%v{}", iteration.raw()),
            2,
            types,
        )?
        .join("\n"),
        MirInstructionKind::InterfaceUpcast { value, .. } => {
            format!("{result} = add i64 %v{}, 0", value.raw())
        }
        MirInstructionKind::CaptureCellLoad { cell } => lower_runtime_slot_load_from(
            instruction.result(),
            instruction.result_type(),
            &format!("%v{}", cell.raw()),
            1,
            types,
        )?
        .join("\n"),
        MirInstructionKind::CaptureCellStore { cell, value } => {
            lower_capture_store(&format!("%v{}", cell.raw()), *value, value_types, types)?
        }
        MirInstructionKind::CaptureLoad { slot, mode, .. } => lower_capture_load(
            instruction.result(),
            instruction.result_type(),
            environment
                .ok_or(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                })?
                .0,
            *slot,
            *mode,
            environment.is_some_and(|(_, self_slots)| self_slots.contains(slot)),
            types,
        )?,
        MirInstructionKind::CaptureCellReference { slot, .. } => lower_runtime_slot_load_from(
            instruction.result(),
            instruction.result_type(),
            environment
                .ok_or(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                })?
                .0,
            *slot as usize + 2,
            types,
        )?
        .join("\n"),
        MirInstructionKind::CaptureStore { slot, value, .. } => lower_nested_capture_store(
            environment
                .ok_or(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                })?
                .0,
            *slot,
            *value,
            value_types,
            types,
        )?,
    };
    Ok(line)
}

pub(crate) fn lower_builtin_iteration_call(
    result: &str,
    result_type: TypeId,
    method: pop_foundation::IterationProtocolMethodId,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let [receiver] = arguments else {
        return Err(LlvmLoweringError::InvalidType(result_type));
    };
    let receiver_type = *values
        .get(receiver)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    if method.raw() == 0 {
        let protocol = pop_types::embedded_bootstrap_schema()
            .ok()
            .and_then(|schema| schema.iteration_protocol())
            .ok_or(LlvmLoweringError::InvalidType(receiver_type))?;
        let kind = match types.get(receiver_type) {
            Some(SemanticType::Array(_)) => IterationCollectionKind::Array,
            Some(SemanticType::Table { .. }) => IterationCollectionKind::Table,
            Some(SemanticType::Builtin { definition, .. }) if *definition == protocol.list() => {
                IterationCollectionKind::List
            }
            Some(SemanticType::Builtin { definition, .. }) if *definition == protocol.range() => {
                IterationCollectionKind::Range
            }
            Some(SemanticType::Builtin { definition, .. })
                if *definition == protocol.iterator() =>
            {
                return Ok(format!("{result} = add i64 %v{}, 0", receiver.raw()));
            }
            _ => return Err(LlvmLoweringError::InvalidType(receiver_type)),
        };
        return Ok(format!(
            "{result} = call i64 @{}(i64 %v{}, i8 {})",
            native_runtime_symbol(RuntimeOperation::IterationAcquire),
            receiver.raw(),
            kind as u8
        ));
    }
    if method.raw() != 1 {
        return Err(LlvmLoweringError::InvalidType(result_type));
    }
    let item_type = match types.get(result_type) {
        Some(SemanticType::Builtin { arguments, .. }) if arguments.len() == 1 => arguments[0],
        _ => return Err(LlvmLoweringError::InvalidType(result_type)),
    };
    let output = format!("{result}_iteration_output");
    let status = format!("{result}_iteration_status");
    let item = format!("{result}_iteration_item");
    let end = format!("{result}_iteration_end");
    let valid = format!("{result}_iteration_valid");
    let trap = format!("{}_iteration_trap", result.trim_start_matches('%'));
    let continuation = format!("{}_iteration_continue", result.trim_start_matches('%'));
    let mut lines = vec![
        format!("{output} = alloca i64"),
        format!(
            "{status} = call i8 @{}(i64 %v{}, ptr {output})",
            native_runtime_symbol(RuntimeOperation::IterationNext),
            receiver.raw()
        ),
        format!(
            "{item} = icmp eq i8 {status}, {}",
            IterationStatus::Item as u8
        ),
        format!(
            "{end} = icmp eq i8 {status}, {}",
            IterationStatus::End as u8
        ),
        format!("{valid} = or i1 {item}, {end}"),
        format!("br i1 {valid}, label %{continuation}, label %{trap}"),
        format!("{trap}:"),
        format!(
            "call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "unreachable".to_owned(),
        format!("{continuation}:"),
    ];
    let reference_slots = if is_managed_type(item_type, types) {
        vec![1]
    } else {
        Vec::new()
    };
    lines.extend(lower_mapped_allocation(result, 2, &reference_slots));
    lines.extend([
        format!("{result}_iteration_tag_i8 = sub i8 {status}, 1"),
        format!("{result}_iteration_tag = zext i8 {result}_iteration_tag_i8 to i64"),
        format!("{result}_iteration_payload = load i64, ptr {output}"),
        format!(
            "call i8 @{}(i64 {result}, i64 1, i64 {result}_iteration_tag)",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
        format!(
            "call i8 @{}(i64 {result}, i64 2, i64 {result}_iteration_payload)",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
    ]);
    Ok(lines.join("\n"))
}

fn lower_string_format(
    result: &str,
    result_id: ValueId,
    kind: pop_types::StringFormatKind,
    value: ValueId,
) -> String {
    use pop_runtime_native_abi::StringFormatTag;

    let temporary = format!("%string_format_bits_{}", result_id.raw());
    let (tag, conversion, bits) = match kind {
        pop_types::StringFormatKind::Boolean => (
            StringFormatTag::Boolean,
            Some(format!("{temporary} = zext i1 %v{} to i64", value.raw())),
            temporary.clone(),
        ),
        pop_types::StringFormatKind::Integer(kind) => {
            let tag = match kind {
                IntegerKind::Int8 => StringFormatTag::Int8,
                IntegerKind::Int16 => StringFormatTag::Int16,
                IntegerKind::Int32 => StringFormatTag::Int32,
                IntegerKind::Int64 => StringFormatTag::Int64,
                IntegerKind::UInt8 => StringFormatTag::UInt8,
                IntegerKind::UInt16 => StringFormatTag::UInt16,
                IntegerKind::UInt32 => StringFormatTag::UInt32,
                IntegerKind::UInt64 => StringFormatTag::UInt64,
            };
            if kind.bit_width() == 64 {
                (tag, None, format!("%v{}", value.raw()))
            } else {
                let operation = if kind.is_signed() { "sext" } else { "zext" };
                (
                    tag,
                    Some(format!(
                        "{temporary} = {operation} i{} %v{} to i64",
                        kind.bit_width(),
                        value.raw()
                    )),
                    temporary.clone(),
                )
            }
        }
        pop_types::StringFormatKind::Float(FloatKind::Float32) => {
            let raw = format!("%string_format_raw_{}", result_id.raw());
            (
                StringFormatTag::Float32,
                Some(format!(
                    "{raw} = bitcast float %v{} to i32\n{temporary} = zext i32 {raw} to i64",
                    value.raw()
                )),
                temporary.clone(),
            )
        }
        pop_types::StringFormatKind::Float(FloatKind::Float64) => (
            StringFormatTag::Float64,
            Some(format!(
                "{temporary} = bitcast double %v{} to i64",
                value.raw()
            )),
            temporary.clone(),
        ),
    };
    let call = format!(
        "{result} = call i64 @{}(i32 {}, i64 {bits})",
        native_runtime_symbol(RuntimeOperation::StringFormat),
        tag as u32
    );
    conversion.map_or(call.clone(), |conversion| format!("{conversion}\n{call}"))
}

pub(crate) fn lower_terminator(
    terminator: &MirTerminator,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    direct_scalar_arrays: &DirectScalarArrays,
) -> Result<String, LlvmLoweringError> {
    let lowered = match terminator {
        MirTerminator::Branch { target, .. } => format!("br label %b{}", target.raw()),
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => format!(
            "br i1 %v{}, label %b{}, label %b{}",
            condition.raw(),
            when_true.raw(),
            when_false.raw()
        ),
        MirTerminator::Return { values: returned } if returned.is_empty() => "ret void".to_owned(),
        MirTerminator::Return { values: returned } => {
            let value = returned[0];
            format!(
                "ret {} %v{}",
                llvm_value_type(values, value, types)?,
                value.raw()
            )
        }
        MirTerminator::Trap(_) => format!(
            "call void @{}()\n  unreachable",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::ResumeUnwind => format!(
            "call void @{}()\n  unreachable",
            native_runtime_symbol(RuntimeOperation::ContinueUnwind)
        ),
        MirTerminator::Unreachable | MirTerminator::Missing => "unreachable".to_owned(),
        MirTerminator::UnionSwitch {
            scrutinee, arms, ..
        } => {
            let tag = format!("%v{}_union_tag", scrutinee.raw());
            let cases = arms
                .iter()
                .map(|arm| {
                    format!(
                        "    i64 {}, label %b{}",
                        arm.case().raw(),
                        arm.target().raw()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{tag} = call i64 @{}(i64 %v{}, i64 1)\n  switch i64 {tag}, label %pop_invalid_union [\n{cases}\n  ]",
                native_runtime_symbol(RuntimeOperation::FieldGet),
                scrutinee.raw()
            )
        }
        MirTerminator::ErrorSwitch {
            scrutinee, arms, ..
        } => {
            let tag = format!("%v{}_error_tag", scrutinee.raw());
            let cases = arms
                .iter()
                .map(|arm| {
                    format!(
                        "    i64 {}, label %b{}",
                        arm.case().raw(),
                        arm.target().raw()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{tag} = call i64 @{}(i64 %v{}, i64 1)\n  switch i64 {tag}, label %pop_invalid_union [\n{cases}\n  ]",
                native_runtime_symbol(RuntimeOperation::FieldGet),
                scrutinee.raw()
            )
        }
        MirTerminator::Suspend { .. } => return Err(LlvmLoweringError::UnsupportedAsync),
    };
    if matches!(terminator, MirTerminator::Return { .. })
        && !direct_scalar_arrays.allocations.is_empty()
    {
        let releases = direct_scalar_arrays
            .allocations
            .iter()
            .filter(|(_, allocation)| allocation.storage == DirectScalarArrayStorage::Native)
            .map(|(origin, _)| {
                format!(
                    "%pop_direct_array_{}_storage = inttoptr i64 %v{} to ptr\n  call void @free(ptr %pop_direct_array_{}_storage)",
                    origin.raw(),
                    origin.raw(),
                    origin.raw()
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        Ok(format!("{releases}\n  {lowered}"))
    } else {
        Ok(lowered)
    }
}

pub(crate) fn lower_checked_integer_binary(
    result: &str,
    operation: &str,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
) -> String {
    let bits = kind.bit_width();
    let signed = if kind.is_signed() { 's' } else { 'u' };
    let pair = format!("{result}_checked");
    let overflow = format!("{result}_overflow");
    format!(
        "{pair} = call {{ i{bits}, i1 }} @llvm.{signed}{operation}.with.overflow.i{bits}(i{bits} %v{}, i{bits} %v{})\n{result} = extractvalue {{ i{bits}, i1 }} {pair}, 0\n{overflow} = extractvalue {{ i{bits}, i1 }} {pair}, 1\n{}",
        left.raw(),
        right.raw(),
        lower_trap_edge(result, &overflow)
    )
}

pub(crate) fn lower_checked_integer_division(
    result: &str,
    operation: &str,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
) -> String {
    let bits = kind.bit_width();
    let zero = format!("{result}_zero");
    let mut lines = vec![format!("{zero} = icmp eq i{bits} %v{}, 0", right.raw())];
    let invalid = if kind.is_signed() {
        let minimum = -(1_i128 << (bits - 1));
        let minimum_value = format!("{result}_minimum");
        let negative_one = format!("{result}_negative_one");
        let overflow = format!("{result}_overflow");
        let invalid = format!("{result}_invalid");
        lines.extend([
            format!(
                "{minimum_value} = icmp eq i{bits} %v{}, {minimum}",
                left.raw()
            ),
            format!("{negative_one} = icmp eq i{bits} %v{}, -1", right.raw()),
            format!("{overflow} = and i1 {minimum_value}, {negative_one}"),
            format!("{invalid} = or i1 {zero}, {overflow}"),
        ]);
        invalid
    } else {
        zero
    };
    lines.push(lower_trap_edge(result, &invalid));
    lines.push(format!(
        "{result} = {} i{bits} %v{}, %v{}",
        if kind.is_signed() {
            format!("s{operation}")
        } else {
            format!("u{operation}")
        },
        left.raw(),
        right.raw()
    ));
    lines.join("\n")
}

pub(crate) fn lower_checked_integer_negate(
    result: &str,
    kind: IntegerKind,
    operand: ValueId,
) -> String {
    let bits = kind.bit_width();
    let signed = if kind.is_signed() { 's' } else { 'u' };
    let pair = format!("{result}_checked");
    let overflow = format!("{result}_overflow");
    format!(
        "{pair} = call {{ i{bits}, i1 }} @llvm.{signed}sub.with.overflow.i{bits}(i{bits} 0, i{bits} %v{})\n{result} = extractvalue {{ i{bits}, i1 }} {pair}, 0\n{overflow} = extractvalue {{ i{bits}, i1 }} {pair}, 1\n{}",
        operand.raw(),
        lower_trap_edge(result, &overflow)
    )
}

pub(crate) fn lower_integer_conversion(
    result: &str,
    source: IntegerKind,
    target: IntegerKind,
    operand: ValueId,
) -> String {
    let source_bits = source.bit_width();
    let target_bits = target.bit_width();
    let value = format!("%v{}", operand.raw());
    let conversion = if source_bits == target_bits {
        format!("{result} = add i{target_bits} 0, {value}")
    } else if source_bits < target_bits {
        format!(
            "{result} = {} i{source_bits} {value} to i{target_bits}",
            if source.is_signed() { "sext" } else { "zext" }
        )
    } else {
        format!("{result} = trunc i{source_bits} {value} to i{target_bits}")
    };

    let invalid = match (source.is_signed(), target.is_signed()) {
        (true, true) if target_bits < source_bits => {
            let below = format!("{result}_below");
            let above = format!("{result}_above");
            let invalid = format!("{result}_invalid");
            let minimum = -(1_i128 << (target_bits - 1));
            let maximum = (1_i128 << (target_bits - 1)) - 1;
            Some((
                vec![
                    format!("{below} = icmp slt i{source_bits} {value}, {minimum}"),
                    format!("{above} = icmp sgt i{source_bits} {value}, {maximum}"),
                    format!("{invalid} = or i1 {below}, {above}"),
                ],
                invalid,
            ))
        }
        (false, false) if target_bits < source_bits => {
            let invalid = format!("{result}_invalid");
            let maximum = (1_u128 << target_bits) - 1;
            Some((
                vec![format!(
                    "{invalid} = icmp ugt i{source_bits} {value}, {maximum}"
                )],
                invalid,
            ))
        }
        (true, false) => {
            let negative = format!("{result}_negative");
            let invalid = format!("{result}_invalid");
            let mut lines = vec![format!("{negative} = icmp slt i{source_bits} {value}, 0")];
            if target_bits < source_bits {
                let above = format!("{result}_above");
                let maximum = (1_u128 << target_bits) - 1;
                lines.extend([
                    format!("{above} = icmp sgt i{source_bits} {value}, {maximum}"),
                    format!("{invalid} = or i1 {negative}, {above}"),
                ]);
            } else {
                lines.push(format!("{invalid} = xor i1 {negative}, false"));
            }
            Some((lines, invalid))
        }
        (false, true) if target_bits <= source_bits => {
            let invalid = format!("{result}_invalid");
            let maximum = (1_u128 << (target_bits - 1)) - 1;
            Some((
                vec![format!(
                    "{invalid} = icmp ugt i{source_bits} {value}, {maximum}"
                )],
                invalid,
            ))
        }
        _ => None,
    };
    if let Some((mut lines, invalid)) = invalid {
        lines.push(lower_trap_edge(result, &invalid));
        lines.push(conversion);
        lines.join("\n")
    } else {
        conversion
    }
}

pub(crate) fn lower_float_to_integer_conversion(
    result: &str,
    source: FloatKind,
    target: IntegerKind,
    operand: ValueId,
) -> String {
    let float = float_type(source);
    let intrinsic_suffix = match source {
        FloatKind::Float32 => "f32",
        FloatKind::Float64 => "f64",
    };
    let bits = target.bit_width();
    let truncated = format!("{result}_truncated");
    let below_limit = format!("{result}_below_limit");
    let above_limit = format!("{result}_above_limit");
    let in_range = format!("{result}_in_range");
    let invalid = format!("{result}_invalid");
    let lower = if target.is_signed() {
        format!("-{}", 1_u128 << (bits - 1))
    } else {
        "0".to_owned()
    };
    let upper_exclusive = if target.is_signed() {
        1_u128 << (bits - 1)
    } else {
        1_u128 << bits
    };
    let conversion = if target.is_signed() {
        "fptosi"
    } else {
        "fptoui"
    };
    [
        format!(
            "{truncated} = call {float} @llvm.trunc.{intrinsic_suffix}({float} %v{})",
            operand.raw()
        ),
        format!("{below_limit} = fcmp oge {float} {truncated}, {lower}.0"),
        format!("{above_limit} = fcmp olt {float} {truncated}, {upper_exclusive}.0"),
        format!("{in_range} = and i1 {below_limit}, {above_limit}"),
        format!("{invalid} = xor i1 {in_range}, true"),
        lower_trap_edge(result, &invalid),
        format!("{result} = {conversion} {float} {truncated} to i{bits}"),
    ]
    .join("\n")
}

pub(crate) fn lower_float_conversion(
    result: &str,
    source: FloatKind,
    target: FloatKind,
    operand: ValueId,
) -> String {
    match (source, target) {
        (FloatKind::Float32, FloatKind::Float64) => {
            format!("{result} = fpext float %v{} to double", operand.raw())
        }
        (FloatKind::Float64, FloatKind::Float32) => {
            format!("{result} = fptrunc double %v{} to float", operand.raw())
        }
        _ => format!(
            "{result} = fadd {} %v{}, 0.0",
            float_type(source),
            operand.raw()
        ),
    }
}

pub(crate) fn lower_trap_edge(result: &str, condition: &str) -> String {
    let label = result.trim_start_matches('%');
    let expected = format!("{condition}_expected");
    format!(
        "{expected} = call i1 @llvm.expect.i1(i1 {condition}, i1 false)\nbr i1 {expected}, label %{label}_trap, label %{label}_continue\n{label}_trap:\n  call void @{}()\n  unreachable\n{label}_continue:",
        native_runtime_symbol(RuntimeOperation::Trap)
    )
}

pub(crate) fn lower_equality(
    result: &str,
    left: ValueId,
    right: ValueId,
    negated: bool,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
) -> Result<String, LlvmLoweringError> {
    let type_id = *values
        .get(&left)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    if types.get(type_id) == Some(&SemanticType::Primitive(PrimitiveType::String)) {
        let equal = format!("{result}_string_equal");
        return Ok(format!(
            "{equal} = call i8 @pop_rt_string_equal(i64 %v{}, i64 %v{})\n{result} = icmp {} i8 {equal}, 0",
            left.raw(),
            right.raw(),
            if negated { "eq" } else { "ne" }
        ));
    }
    if matches!(
        types.get(type_id),
        Some(SemanticType::Tuple(_) | SemanticType::Record(_))
    ) {
        return lower_aggregate_equality(
            result,
            left,
            right,
            type_id,
            negated,
            types,
            record_field_types,
        );
    }
    let ty = llvm_value_type(values, left, types)?;
    let operator = match (ty.as_str(), negated) {
        ("float" | "double", false) => "fcmp oeq",
        ("float" | "double", true) => "fcmp une",
        (_, false) => "icmp eq",
        (_, true) => "icmp ne",
    };
    Ok(format!(
        "{result} = {operator} {ty} %v{}, %v{}",
        left.raw(),
        right.raw()
    ))
}

pub(crate) fn lower_aggregate_equality(
    result: &str,
    left: ValueId,
    right: ValueId,
    type_id: TypeId,
    negated: bool,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
) -> Result<String, LlvmLoweringError> {
    let mut lines = Vec::new();
    let condition = emit_aggregate_equality(
        &mut lines,
        result.trim_start_matches('%'),
        &format!("%v{}", left.raw()),
        &format!("%v{}", right.raw()),
        type_id,
        types,
        record_field_types,
    )?;
    lines.push(if negated {
        format!("{result} = xor i1 {condition}, true")
    } else {
        format!("{result} = xor i1 {condition}, false")
    });
    Ok(lines.join("\n"))
}

pub(crate) fn emit_aggregate_equality(
    lines: &mut Vec<String>,
    prefix: &str,
    left: &str,
    right: &str,
    type_id: TypeId,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
) -> Result<String, LlvmLoweringError> {
    let field_types = match types
        .get(type_id)
        .ok_or(LlvmLoweringError::InvalidType(type_id))?
    {
        SemanticType::Tuple(elements) => elements.clone(),
        SemanticType::Record(_) => record_field_types
            .get(&type_id)
            .cloned()
            .ok_or(LlvmLoweringError::InvalidType(type_id))?,
        _ => return Err(LlvmLoweringError::InvalidType(type_id)),
    };
    let mut conditions = Vec::new();
    for (index, field_type) in field_types.into_iter().enumerate() {
        let left_field = format!("%{prefix}_{index}_left");
        let right_field = format!("%{prefix}_{index}_right");
        lines.extend([
            format!(
                "{left_field} = call i64 @{}(i64 {left}, i64 {})",
                native_runtime_symbol(RuntimeOperation::FieldGet),
                index + 1
            ),
            format!(
                "{right_field} = call i64 @{}(i64 {right}, i64 {})",
                native_runtime_symbol(RuntimeOperation::FieldGet),
                index + 1
            ),
        ]);
        conditions.push(emit_stored_value_equality(
            lines,
            &format!("{prefix}_{index}"),
            &left_field,
            &right_field,
            field_type,
            types,
            record_field_types,
        )?);
    }
    if conditions.is_empty() {
        let condition = format!("%{prefix}_empty");
        lines.push(format!("{condition} = xor i1 0, true"));
        return Ok(condition);
    }
    let mut combined = conditions[0].clone();
    for (index, condition) in conditions.into_iter().enumerate().skip(1) {
        let next = format!("%{prefix}_and_{index}");
        lines.push(format!("{next} = and i1 {combined}, {condition}"));
        combined = next;
    }
    Ok(combined)
}

pub(crate) fn emit_stored_value_equality(
    lines: &mut Vec<String>,
    prefix: &str,
    left: &str,
    right: &str,
    type_id: TypeId,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
) -> Result<String, LlvmLoweringError> {
    let semantic = types
        .get(type_id)
        .ok_or(LlvmLoweringError::InvalidType(type_id))?;
    if matches!(semantic, SemanticType::Tuple(_) | SemanticType::Record(_)) {
        return emit_aggregate_equality(
            lines,
            prefix,
            left,
            right,
            type_id,
            types,
            record_field_types,
        );
    }
    let condition = format!("%{prefix}_equal");
    match semantic {
        SemanticType::Primitive(PrimitiveType::String) => {
            let raw = format!("%{prefix}_string_equal");
            lines.extend([
                format!("{raw} = call i8 @pop_rt_string_equal(i64 {left}, i64 {right})"),
                format!("{condition} = icmp ne i8 {raw}, 0"),
            ]);
        }
        SemanticType::Primitive(PrimitiveType::Float32) => {
            let left_bits = format!("%{prefix}_left_bits");
            let right_bits = format!("%{prefix}_right_bits");
            let left_float = format!("%{prefix}_left_float");
            let right_float = format!("%{prefix}_right_float");
            lines.extend([
                format!("{left_bits} = trunc i64 {left} to i32"),
                format!("{right_bits} = trunc i64 {right} to i32"),
                format!("{left_float} = bitcast i32 {left_bits} to float"),
                format!("{right_float} = bitcast i32 {right_bits} to float"),
                format!("{condition} = fcmp oeq float {left_float}, {right_float}"),
            ]);
        }
        SemanticType::Primitive(PrimitiveType::Float64) => {
            let left_float = format!("%{prefix}_left_float");
            let right_float = format!("%{prefix}_right_float");
            lines.extend([
                format!("{left_float} = bitcast i64 {left} to double"),
                format!("{right_float} = bitcast i64 {right} to double"),
                format!("{condition} = fcmp oeq double {left_float}, {right_float}"),
            ]);
        }
        _ => lines.push(format!("{condition} = icmp eq i64 {left}, {right}")),
    }
    Ok(condition)
}

pub(crate) fn call_line(
    result: &str,
    result_type: Option<TypeId>,
    callee: &str,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let args = arguments
        .iter()
        .map(|value| {
            llvm_value_type(values, *value, types).map(|ty| format!("{ty} %v{}", value.raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let assignment = result_type.map_or_else(String::new, |_| format!("{result} = "));
    let return_type =
        result_type.map_or_else(|| Ok("void".to_owned()), |id| llvm_type(id, types))?;
    Ok(format!(
        "{assignment}call {return_type} {callee}({})",
        args.join(", ")
    ))
}

pub(crate) fn lower_array_create(
    result: &str,
    length: ValueId,
    initial_value: ValueId,
    element_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let initial_type = *values
        .get(&initial_value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) = lower_runtime_slot_store(
        initial_value,
        initial_type,
        &llvm_type(initial_type, types)?,
    )?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!("{result}_length_valid = icmp sge i64 %v{}, 0", length.raw()),
        format!(
            "{result}_length_expected = call i1 @llvm.expect.i1(i1 {result}_length_valid, i1 true)"
        ),
        format!(
            "br i1 {result}_length_expected, label %{label}_create, label %{label}_length_trap"
        ),
        format!("{label}_length_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_create:"),
        format!(
            "  {result} = call i64 @{}(i64 %v{}, i1 {}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::AllocateArrayFilled),
            length.raw(),
            u8::from(element_map == ArrayElementMap::ManagedReference)
        ),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_direct_array_create(
    bubble: BubbleId,
    result: &str,
    allocation: DirectScalarArray,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let initial_type = *values
        .get(&allocation.initial_value)
        .ok_or(LlvmLoweringError::InvalidType(allocation.element_type))?;
    if initial_type != allocation.element_type {
        return Err(LlvmLoweringError::InvalidType(allocation.element_type));
    }
    if allocation.storage == DirectScalarArrayStorage::ScalarReplaced {
        return Ok(lower_scalar_replaced_array_create(result, allocation));
    }
    let (mut lines, stored) = lower_runtime_slot_store(
        allocation.initial_value,
        initial_type,
        &llvm_type(initial_type, types)?,
    )?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result}_size_pair = call {{ i64, i1 }} @llvm.umul.with.overflow.i64(i64 %v{}, i64 8)",
            allocation.length.raw()
        ),
        format!("{result}_size = extractvalue {{ i64, i1 }} {result}_size_pair, 0"),
        format!("{result}_size_overflow = extractvalue {{ i64, i1 }} {result}_size_pair, 1"),
        format!(
            "{result}_length_nonnegative = icmp sge i64 %v{}, 0",
            allocation.length.raw()
        ),
        format!("{result}_size_valid = xor i1 {result}_size_overflow, true"),
        format!(
            "{result}_shape_valid = and i1 {result}_length_nonnegative, {result}_size_valid"
        ),
        format!(
            "{result}_shape_expected = call i1 @llvm.expect.i1(i1 {result}_shape_valid, i1 true)"
        ),
        format!(
            "br i1 {result}_shape_expected, label %{label}_allocate, label %{label}_length_trap"
        ),
        format!("{label}_length_trap:"),
        format!("  call void @{}()", native_runtime_symbol(RuntimeOperation::Trap)),
        "  unreachable".to_owned(),
        format!("{label}_allocate:"),
        format!("  {result}_storage = call noalias ptr @malloc(i64 {result}_size)"),
        format!(
            "  {result}_empty = icmp eq i64 %v{}, 0",
            allocation.length.raw()
        ),
        format!("  {result}_allocated = icmp ne ptr {result}_storage, null"),
        format!(
            "  {result}_allocation_valid = or i1 {result}_empty, {result}_allocated"
        ),
        format!(
            "  {result}_allocation_expected = call i1 @llvm.expect.i1(i1 {result}_allocation_valid, i1 true)"
        ),
        format!(
            "  br i1 {result}_allocation_expected, label %{label}_initialize, label %{label}_allocation_trap"
        ),
        format!("{label}_allocation_trap:"),
        format!("  call void @{}()", native_runtime_symbol(RuntimeOperation::Trap)),
        "  unreachable".to_owned(),
        format!("{label}_initialize:"),
        format!(
            "  call void @{}(ptr {result}_storage, i64 %v{}, i64 {stored})",
            crate::lowering::direct_scalar_array_fill_name(bubble),
            allocation.length.raw()
        ),
        format!("  br label %{label}_create"),
        format!("{label}_create:"),
        format!("  {result} = ptrtoint ptr {result}_storage to i64"),
    ]);
    Ok(lines.join("\n"))
}

fn lower_scalar_replaced_array_create(result: &str, allocation: DirectScalarArray) -> String {
    let label = result.trim_start_matches('%');
    [
        format!(
            "{result}_size_pair = call {{ i64, i1 }} @llvm.umul.with.overflow.i64(i64 %v{}, i64 8)",
            allocation.length.raw()
        ),
        format!("{result}_size_overflow = extractvalue {{ i64, i1 }} {result}_size_pair, 1"),
        format!(
            "{result}_length_nonnegative = icmp sge i64 %v{}, 0",
            allocation.length.raw()
        ),
        format!("{result}_size_valid = xor i1 {result}_size_overflow, true"),
        format!("{result}_shape_valid = and i1 {result}_length_nonnegative, {result}_size_valid"),
        format!(
            "{result}_shape_expected = call i1 @llvm.expect.i1(i1 {result}_shape_valid, i1 true)"
        ),
        format!("br i1 {result}_shape_expected, label %{label}_create, label %{label}_length_trap"),
        format!("{label}_length_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_create:"),
        format!("  {result} = add i64 %v{}, 0", allocation.length.raw()),
    ]
    .join("\n")
}

pub(crate) fn lower_direct_array_length(result: &str, allocation: DirectScalarArray) -> String {
    let label = result.trim_start_matches('%');
    format!(
        "br label %{label}_load\n{label}_load:\n  {result} = add i64 %v{}, 0",
        allocation.length.raw()
    )
}

pub(crate) fn lower_direct_array_get(
    result: &str,
    origin: ValueId,
    allocation: DirectScalarArray,
    index: ValueId,
    result_type: TypeId,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let element_type = llvm_type(result_type, types)?;
    let expected_type = llvm_type(allocation.element_type, types)?;
    if element_type != expected_type {
        return Err(LlvmLoweringError::InvalidType(result_type));
    }
    if allocation.storage == DirectScalarArrayStorage::ScalarReplaced {
        return Ok(lower_scalar_replaced_array_get(
            result,
            allocation,
            index,
            &element_type,
        ));
    }
    let label = result.trim_start_matches('%');
    let mut lines = vec![
        format!("{result}_zero_index = sub i64 %v{}, 1", index.raw()),
        format!(
            "{result}_in_bounds = icmp ult i64 {result}_zero_index, %v{}",
            allocation.length.raw()
        ),
        format!(
            "{result}_in_bounds_expected = call i1 @llvm.expect.i1(i1 {result}_in_bounds, i1 true)"
        ),
        format!("br i1 {result}_in_bounds_expected, label %{label}_load, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_load:"),
        format!(
            "  {result}_storage = inttoptr i64 %v{} to ptr",
            origin.raw()
        ),
        format!(
            "  {result}_slot = getelementptr i64, ptr {result}_storage, i64 {result}_zero_index"
        ),
    ];
    lines.extend(lower_array_output_load(
        result,
        result_type,
        &format!("{result}_slot"),
        types,
    )?);
    Ok(lines.join("\n"))
}

fn lower_scalar_replaced_array_get(
    result: &str,
    allocation: DirectScalarArray,
    index: ValueId,
    element_type: &str,
) -> String {
    let label = result.trim_start_matches('%');
    [
        format!("{result}_zero_index = sub i64 %v{}, 1", index.raw()),
        format!(
            "{result}_in_bounds = icmp ult i64 {result}_zero_index, %v{}",
            allocation.length.raw()
        ),
        format!(
            "{result}_in_bounds_expected = call i1 @llvm.expect.i1(i1 {result}_in_bounds, i1 true)"
        ),
        format!("br i1 {result}_in_bounds_expected, label %{label}_load, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_load:"),
        format!(
            "  {result} = select i1 true, {element_type} %v{}, {element_type} %v{}",
            allocation.initial_value.raw(),
            allocation.initial_value.raw()
        ),
    ]
    .join("\n")
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_direct_array_set(
    result: &str,
    origin: ValueId,
    allocation: DirectScalarArray,
    index: ValueId,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(allocation.element_type))?;
    if value_type != allocation.element_type {
        return Err(LlvmLoweringError::InvalidType(allocation.element_type));
    }
    let (mut conversion, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    conversion.extend([
        format!("{result}_zero_index = sub i64 %v{}, 1", index.raw()),
        format!(
            "{result}_in_bounds = icmp ult i64 {result}_zero_index, %v{}",
            allocation.length.raw()
        ),
        format!(
            "{result}_in_bounds_expected = call i1 @llvm.expect.i1(i1 {result}_in_bounds, i1 true)"
        ),
        format!("br i1 {result}_in_bounds_expected, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!(
            "  {result}_storage = inttoptr i64 %v{} to ptr",
            origin.raw()
        ),
        format!(
            "  {result}_slot = getelementptr i64, ptr {result}_storage, i64 {result}_zero_index"
        ),
        format!("  store i64 {stored}, ptr {result}_slot, align 8"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(conversion.join("\n"))
}

pub(crate) fn lower_direct_array_fill(
    bubble: BubbleId,
    result: &str,
    origin: ValueId,
    allocation: DirectScalarArray,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(allocation.element_type))?;
    if value_type != allocation.element_type {
        return Err(LlvmLoweringError::InvalidType(allocation.element_type));
    }
    let (mut lines, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!("{result}_storage = inttoptr i64 %v{} to ptr", origin.raw()),
        format!(
            "call void @{}(ptr {result}_storage, i64 %v{}, i64 {stored})",
            crate::lowering::direct_scalar_array_fill_name(bubble),
            allocation.length.raw()
        ),
        format!("br label %{label}_continue"),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_array_output_call(
    result: &str,
    result_type: TypeId,
    operation: RuntimeOperation,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let output = format!("{result}_output");
    let success = format!("{result}_success");
    let expected = format!("{result}_success_expected");
    let label = result.trim_start_matches('%');
    let arguments = arguments
        .iter()
        .map(|value| {
            llvm_value_type(values, *value, types).map(|ty| format!("{ty} %v{}", value.raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut lines = Vec::new();
    lines.extend([
        format!(
            "{success} = call i8 @{}({}, ptr {output})",
            native_runtime_symbol(operation),
            arguments.join(", ")
        ),
        format!("{success}_condition = icmp ne i8 {success}, 0"),
        format!("{expected} = call i1 @llvm.expect.i1(i1 {success}_condition, i1 true)"),
        format!("br i1 {expected}, label %{label}_load, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_load:"),
    ]);
    lines.extend(lower_array_output_load(
        result,
        result_type,
        &output,
        types,
    )?);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_optional_array_get(
    result: &str,
    array: ValueId,
    index: ValueId,
    result_type: TypeId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let inner = optional_inner_type(types, result_type)
        .ok_or(LlvmLoweringError::InvalidType(result_type))?;
    let inner_type = llvm_type(inner, types)?;
    let output = format!("{result}_output");
    let status = format!("{result}_status");
    let present = format!("{result}_present");
    let payload = format!("{result}_payload");
    let partial = format!("{result}_partial");
    let array_type = llvm_value_type(values, array, types)?;
    let index_type = llvm_value_type(values, index, types)?;
    let mut lines = vec![
        format!("store i64 0, ptr {output}"),
        format!(
            "{status} = call i8 @{}({array_type} %v{}, {index_type} %v{}, ptr {output})",
            native_runtime_symbol(RuntimeOperation::ArrayGetChecked),
            array.raw(),
            index.raw(),
        ),
        format!("{present} = icmp ne i8 {status}, 0"),
    ];
    lines.extend(lower_array_output_load(&payload, inner, &output, types)?);
    lines.extend([
        format!("{partial} = insertvalue {{ i1, {inner_type} }} zeroinitializer, i1 {present}, 0"),
        format!(
            "{result} = insertvalue {{ i1, {inner_type} }} {partial}, {inner_type} {payload}, 1"
        ),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_optional_collection_get(
    result: &str,
    collection: ValueId,
    index: ValueId,
    result_type: TypeId,
    operation: RuntimeOperation,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let inner = optional_inner_type(types, result_type)
        .ok_or(LlvmLoweringError::InvalidType(result_type))?;
    let inner_type = llvm_type(inner, types)?;
    let output = format!("{result}_output");
    let status = format!("{result}_status");
    let present = format!("{result}_present");
    let payload = format!("{result}_payload");
    let partial = format!("{result}_partial");
    let collection_type = llvm_value_type(values, collection, types)?;
    let index_type = llvm_value_type(values, index, types)?;
    let mut lines = vec![
        format!("store i64 0, ptr {output}"),
        format!(
            "{status} = call i8 @{}({collection_type} %v{}, {index_type} %v{}, ptr {output})",
            native_runtime_symbol(operation),
            collection.raw(),
            index.raw(),
        ),
        format!("{present} = icmp ne i8 {status}, 0"),
    ];
    lines.extend(lower_array_output_load(&payload, inner, &output, types)?);
    lines.extend([
        format!("{partial} = insertvalue {{ i1, {inner_type} }} zeroinitializer, i1 {present}, 0"),
        format!(
            "{result} = insertvalue {{ i1, {inner_type} }} {partial}, {inner_type} {payload}, 1"
        ),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_list_create(
    result: &str,
    capacity: Option<ValueId>,
    element_map: ArrayElementMap,
) -> String {
    let label = result.trim_start_matches('%');
    let capacity_value =
        capacity.map_or_else(|| "0".to_owned(), |value| format!("%v{}", value.raw()));
    let mut lines = Vec::new();
    if capacity.is_some() {
        lines.extend([
            format!("{result}_nonnegative = icmp sge i64 {capacity_value}, 0"),
            format!(
                "{result}_nonnegative_expected = call i1 @llvm.expect.i1(i1 {result}_nonnegative, i1 true)"
            ),
            format!(
                "br i1 {result}_nonnegative_expected, label %{label}_allocate, label %{label}_trap"
            ),
            format!("{label}_allocate:"),
        ]);
    }
    lines.extend([
        format!(
            "{result} = call i64 @{}(i64 {capacity_value}, i1 {})",
            native_runtime_symbol(RuntimeOperation::ListCreate),
            u8::from(element_map == ArrayElementMap::ManagedReference)
        ),
        format!("{result}_allocated = icmp ne i64 {result}, 0"),
        format!(
            "{result}_allocated_expected = call i1 @llvm.expect.i1(i1 {result}_allocated, i1 true)"
        ),
        format!("br i1 {result}_allocated_expected, label %{label}_create, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_create:"),
    ]);
    lines.join("\n")
}

fn lower_range_create(
    result: &str,
    result_type: TypeId,
    first: ValueId,
    last: ValueId,
    step: ValueId,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let protocol = pop_types::embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol())
        .ok_or(LlvmLoweringError::InvalidType(result_type))?;
    let integer_type = match types.get(result_type) {
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) if *definition == protocol.range() && arguments.len() == 1 => arguments[0],
        _ => return Err(LlvmLoweringError::InvalidType(result_type)),
    };
    let Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) = types.get(integer_type)
    else {
        return Err(LlvmLoweringError::InvalidType(integer_type));
    };
    let bits = kind.bit_width();
    let mut lines = Vec::new();
    let raw = |name: &str, value: ValueId, lines: &mut Vec<String>| {
        if bits == 64 {
            format!("%v{}", value.raw())
        } else {
            let converted = format!("{result}_{name}");
            lines.push(format!(
                "{converted} = zext i{bits} %v{} to i64",
                value.raw()
            ));
            converted
        }
    };
    let first = raw("first", first, &mut lines);
    let last = raw("last", last, &mut lines);
    let step = raw("step", step, &mut lines);
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result} = call i64 @{}(i64 {first}, i64 {last}, i64 {step}, i1 {}, i8 {bits})",
            native_runtime_symbol(RuntimeOperation::RangeCreate),
            kind.is_signed()
        ),
        format!("{result}_allocated = icmp ne i64 {result}, 0"),
        format!("br i1 {result}_allocated, label %{label}_create, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_create:"),
    ]);
    Ok(lines.join("\n"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_list_mutation(
    result: &str,
    operation: RuntimeOperation,
    list: ValueId,
    index: Option<ValueId>,
    value: ValueId,
    element_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    let index = index.map_or_else(String::new, |index| format!(", i64 %v{}", index.raw()));
    lines.extend([
        format!(
            "{result}_status = call i8 @{}(i64 %v{}{index}, i64 {stored}, i1 {})",
            native_runtime_symbol(operation),
            list.raw(),
            u8::from(element_map == ArrayElementMap::ManagedReference)
        ),
        format!("{result}_success = icmp ne i8 {result}_status, 0"),
        format!("{result}_expected = call i1 @llvm.expect.i1(i1 {result}_success, i1 true)"),
        format!("br i1 {result}_expected, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_array_output_load(
    result: &str,
    result_type: TypeId,
    output: &str,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    let ty = llvm_type(result_type, types)?;
    let loaded = format!("{result}_slot");
    Ok(match ty.as_str() {
        "i64" => vec![format!("  {result} = load i64, ptr {output}")],
        "i1" | "i8" | "i16" | "i32" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {result} = trunc i64 {loaded} to {ty}"),
        ],
        "float" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {loaded}_bits = trunc i64 {loaded} to i32"),
            format!("  {result} = bitcast i32 {loaded}_bits to float"),
        ],
        "double" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {result} = bitcast i64 {loaded} to double"),
        ],
        "ptr" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {result} = inttoptr i64 {loaded} to ptr"),
        ],
        _ => return Err(LlvmLoweringError::InvalidType(result_type)),
    })
}

pub(crate) fn lower_array_fill(
    result: &str,
    array: ValueId,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result}_filled = call i8 @{}(i64 %v{}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::ArrayFill),
            array.raw()
        ),
        format!("{result}_success = icmp ne i8 {result}_filled, 0"),
        format!("{result}_expected = call i1 @llvm.expect.i1(i1 {result}_success, i1 true)"),
        format!("br i1 {result}_expected, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_array_set(
    result: &str,
    array: ValueId,
    index: ValueId,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result}_stored = call i8 @{}(i64 %v{}, i64 %v{}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::ArraySet),
            array.raw(),
            index.raw()
        ),
        format!("{result}_in_bounds = icmp ne i8 {result}_stored, 0"),
        format!(
            "{result}_in_bounds_expected = call i1 @llvm.expect.i1(i1 {result}_in_bounds, i1 true)"
        ),
        format!("br i1 {result}_in_bounds_expected, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_object_make(
    result: &str,
    fields: &[(FieldId, ValueId)],
    slot_count: u32,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    lower_initialized_object(
        result,
        fields,
        slot_count,
        None,
        values,
        types,
        field_layout,
    )
}

#[derive(Clone, Copy)]
enum ObjectInitializer {
    Constant(u64),
    Value(ValueId),
}

#[allow(clippy::too_many_arguments)]
fn lower_initialized_object(
    result: &str,
    fields: &[(FieldId, ValueId)],
    slot_count: u32,
    class: Option<ClassId>,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    let mut reference_slots = Vec::new();
    for (field, value) in fields {
        if values
            .get(value)
            .copied()
            .is_some_and(|type_id| is_managed_type(type_id, types))
        {
            let slot = field_layout
                .get(field)
                .copied()
                .and_then(|slot| slot.checked_sub(1))
                .ok_or(LlvmLoweringError::InvalidFieldLayout(*field))?;
            reference_slots.push(slot);
        }
    }
    let slot_count_usize = usize::try_from(slot_count)
        .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let mut initializers = vec![None; slot_count_usize];
    if let Some(class) = class {
        let Some(slot) = initializers.first_mut() else {
            return Err(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)));
        };
        *slot = Some(ObjectInitializer::Constant(u64::from(class.raw())));
    }
    for (field, value) in fields {
        let slot = field_layout
            .get(field)
            .ok_or(LlvmLoweringError::InvalidFieldLayout(*field))?;
        let index = slot
            .checked_sub(1)
            .and_then(|slot| usize::try_from(slot).ok())
            .ok_or(LlvmLoweringError::InvalidFieldLayout(*field))?;
        let Some(initializer) = initializers.get_mut(index) else {
            return Err(LlvmLoweringError::InvalidFieldLayout(*field));
        };
        *initializer = Some(ObjectInitializer::Value(*value));
    }
    if initializers.iter().any(Option::is_none) {
        return Err(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)));
    }

    let map = format!("{result}_object_map");
    let map_pointer = if reference_slots.is_empty() {
        "null".to_owned()
    } else {
        format!("{map}_pointer")
    };
    let mut lines = Vec::new();
    if !reference_slots.is_empty() {
        lines.push(format!("{map} = alloca [{} x i32]", reference_slots.len()));
        for (index, slot) in reference_slots.iter().enumerate() {
            let entry = format!("{map}_{index}");
            lines.extend([
                format!(
                    "{entry} = getelementptr [{} x i32], ptr {map}, i64 0, i64 {index}",
                    reference_slots.len()
                ),
                format!("store i32 {slot}, ptr {entry}"),
            ]);
        }
        lines.push(format!(
            "{map_pointer} = getelementptr [{} x i32], ptr {map}, i64 0, i64 0",
            reference_slots.len()
        ));
    }

    let payload = format!("{result}_initial_values");
    let payload_pointer = if slot_count == 0 {
        "null".to_owned()
    } else {
        format!("{payload}_pointer")
    };
    if slot_count != 0 {
        lines.push(format!("{payload} = alloca [{slot_count} x i64]"));
        for (index, initializer) in initializers.into_iter().enumerate() {
            let entry = format!("{payload}_{index}");
            lines.push(format!(
                "{entry} = getelementptr [{slot_count} x i64], ptr {payload}, i64 0, i64 {index}"
            ));
            let stored = match initializer.expect("complete initializers were validated") {
                ObjectInitializer::Constant(value) => value.to_string(),
                ObjectInitializer::Value(value) => {
                    let type_id = *values
                        .get(&value)
                        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
                    let (conversions, stored) =
                        lower_runtime_slot_store(value, type_id, &llvm_type(type_id, types)?)?;
                    lines.extend(conversions);
                    stored
                }
            };
            lines.push(format!("store i64 {stored}, ptr {entry}"));
        }
        lines.push(format!(
            "{payload_pointer} = getelementptr [{slot_count} x i64], ptr {payload}, i64 0, i64 0"
        ));
    }
    lines.push(format!(
        "{result} = call i64 @{}(i64 {slot_count}, ptr {map_pointer}, i64 {}, ptr {payload_pointer}, i64 {slot_count})",
        native_runtime_symbol(RuntimeOperation::AllocateObjectInitialized),
        reference_slots.len()
    ));
    Ok(lines.join("\n"))
}

pub(crate) fn lower_mapped_allocation(
    result: &str,
    slot_count: u32,
    reference_slots: &[u32],
) -> Vec<String> {
    if reference_slots.is_empty() {
        return vec![format!(
            "{result} = call i64 @pop_rt_allocate_mapped_object(i64 {slot_count}, ptr null, i64 0)"
        )];
    }
    let map = format!("{result}_object_map");
    let mut lines = vec![format!("{map} = alloca [{} x i32]", reference_slots.len())];
    for (index, slot) in reference_slots.iter().enumerate() {
        let entry = format!("{map}_{index}");
        lines.extend([
            format!(
                "{entry} = getelementptr [{} x i32], ptr {map}, i64 0, i64 {index}",
                reference_slots.len()
            ),
            format!("store i32 {slot}, ptr {entry}"),
        ]);
    }
    lines.push(format!(
        "{result} = call i64 @pop_rt_allocate_mapped_object(i64 {slot_count}, ptr {map}, i64 {})",
        reference_slots.len()
    ));
    lines
}

pub(crate) fn lower_gc_safe_point(
    result: &str,
    safe_point: u32,
    roots: &[ValueId],
    direct_scalar_arrays: &DirectScalarArrays,
    writable_roots: bool,
    poll_interval: u32,
) -> String {
    let roots = roots
        .iter()
        .copied()
        .filter(|root| direct_scalar_arrays.origin(*root).is_none())
        .collect::<Vec<_>>();
    let label = result.trim_start_matches('%');
    let budget = format!("{result}_poll_budget");
    let remaining = format!("{result}_poll_remaining");
    let expired = format!("{result}_poll_expired");
    let expected = format!("{result}_poll_expired_expected");
    let slow = format!("{label}_poll_slow");
    let continuation = format!("{label}_poll_continue");
    let root_array = format!("{result}_roots");
    let mut lines = Vec::new();
    if !roots.is_empty() {
        lines.push(format!("{root_array} = alloca [{} x i64]", roots.len()));
        for (index, root) in roots.iter().enumerate() {
            let entry = format!("{root_array}_{index}");
            lines.extend([
                format!(
                    "{entry} = getelementptr [{} x i64], ptr {root_array}, i64 0, i64 {index}",
                    roots.len()
                ),
                format!("store i64 %v{}, ptr {entry}", root.raw()),
            ]);
        }
    }
    lines.extend([
        format!("{budget} = load i32, ptr {GC_POLL_BUDGET}, align 4"),
        format!("{remaining} = sub i32 {budget}, 1"),
        format!("store i32 {remaining}, ptr {GC_POLL_BUDGET}, align 4"),
        format!("{expired} = icmp eq i32 {remaining}, 0"),
        format!("{expected} = call i1 @llvm.expect.i1(i1 {expired}, i1 false)"),
        format!("br i1 {expected}, label %{slow}, label %{continuation}"),
        format!("{slow}:"),
        format!("store i32 {poll_interval}, ptr {GC_POLL_BUDGET}, align 4"),
    ]);
    let safe_point_symbol = if writable_roots {
        pop_runtime_native_abi::GC_SAFE_POINT_V2_SYMBOL
    } else {
        native_runtime_symbol(RuntimeOperation::GcSafePoint)
    };
    if roots.is_empty() {
        if writable_roots {
            let status = format!("{result}_gc_status");
            let accepted = format!("{result}_gc_accepted");
            let rejected = format!("{label}_gc_rejected");
            lines.extend([
                format!(
                    "{status} = call i8 @{safe_point_symbol}(i32 {safe_point}, ptr null, i64 0)"
                ),
                format!("{accepted} = icmp eq i8 {status}, 1"),
                format!("br i1 {accepted}, label %{continuation}, label %{rejected}"),
                format!("{rejected}:"),
                format!(
                    "call void @{}()",
                    native_runtime_symbol(RuntimeOperation::Trap)
                ),
                "unreachable".to_owned(),
                format!("{continuation}:"),
            ]);
        } else {
            lines.extend([
                format!("call i8 @{safe_point_symbol}(i32 {safe_point}, ptr null, i64 0)"),
                format!("br label %{continuation}"),
                format!("{continuation}:"),
            ]);
        }
        return lines.join("\n");
    }
    if writable_roots {
        let status = format!("{result}_gc_status");
        let accepted = format!("{result}_gc_accepted");
        let rejected = format!("{label}_gc_rejected");
        lines.extend([
            format!(
                "{status} = call i8 @{}(i32 {safe_point}, ptr {root_array}, i64 {})",
                safe_point_symbol,
                roots.len()
            ),
            format!("{accepted} = icmp eq i8 {status}, 1"),
            format!("br i1 {accepted}, label %{continuation}, label %{rejected}"),
            format!("{rejected}:"),
            format!(
                "call void @{}()",
                native_runtime_symbol(RuntimeOperation::Trap)
            ),
            "unreachable".to_owned(),
            format!("{continuation}:"),
        ]);
    } else {
        lines.extend([
            format!(
                "call i8 @{}(i32 {safe_point}, ptr {root_array}, i64 {})",
                safe_point_symbol,
                roots.len()
            ),
            format!("br label %{continuation}"),
            format!("{continuation}:"),
        ]);
    }
    if writable_roots {
        for (index, root) in roots.iter().enumerate() {
            let entry = format!("{root_array}_{index}_reload");
            lines.extend([
                format!(
                    "{entry} = getelementptr [{} x i64], ptr {root_array}, i64 0, i64 {index}",
                    roots.len()
                ),
                format!("%v{}_after_{} = load i64, ptr {entry}", root.raw(), label),
            ]);
        }
    }
    lines.join("\n")
}

pub(crate) fn is_managed_type(type_id: TypeId, types: &TypeArena) -> bool {
    !matches!(
        types.get(type_id),
        Some(SemanticType::Primitive(
            PrimitiveType::Nil
                | PrimitiveType::Boolean
                | PrimitiveType::Integer(_)
                | PrimitiveType::Float32
                | PrimitiveType::Float64
                | PrimitiveType::Never
        ))
    )
}

pub(crate) fn lower_tuple_make(
    result: &str,
    elements: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let reference_slots = elements
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            values
                .get(value)
                .copied()
                .filter(|type_id| is_managed_type(*type_id, types))
                .and_then(|_| u32::try_from(index).ok())
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(elements.len())
            .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        &reference_slots,
    );
    for (index, value) in elements.iter().enumerate() {
        let type_id = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (conversions, stored) =
            lower_runtime_slot_store(*value, type_id, &llvm_type(type_id, types)?)?;
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::FieldSet),
            index + 1
        ));
    }
    Ok(lines.join("\n"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_record_update(
    result: &str,
    record: SymbolId,
    base: ValueId,
    updates: &[(FieldId, ValueId)],
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    field_layout: &BTreeMap<FieldId, u32>,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let fields = record_fields
        .get(&record)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let base_type = *values
        .get(&base)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let reference_slots = record_field_types
        .get(&base_type)
        .ok_or(LlvmLoweringError::InvalidType(base_type))?
        .iter()
        .enumerate()
        .filter_map(|(index, type_id)| {
            is_managed_type(*type_id, types)
                .then(|| u32::try_from(index).ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(fields.len()).map_err(|_| LlvmLoweringError::InvalidType(base_type))?,
        &reference_slots,
    );
    for field in fields {
        let slot = *field_layout
            .get(field)
            .ok_or(LlvmLoweringError::InvalidFieldLayout(*field))?;
        let stored = if let Some((_, value)) = updates.iter().find(|(updated, _)| updated == field)
        {
            let type_id = *values
                .get(value)
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let (conversions, stored) =
                lower_runtime_slot_store(*value, type_id, &llvm_type(type_id, types)?)?;
            lines.extend(conversions);
            stored
        } else {
            let loaded = format!("{result}_field_{slot}");
            lines.push(format!(
                "{loaded} = call i64 @{}(i64 %v{}, i64 {slot})",
                native_runtime_symbol(RuntimeOperation::FieldGet),
                base.raw()
            ));
            loaded
        };
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {slot}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn lower_class_make(
    result: &str,
    class: ClassId,
    fields: &[(FieldId, ValueId)],
    slot_count: u32,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    lower_initialized_object(
        result,
        fields,
        slot_count,
        Some(class),
        values,
        types,
        field_layout,
    )
}

pub(crate) fn lower_union_make(
    result: &str,
    case: pop_foundation::UnionCaseId,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let reference_slots = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            values
                .get(value)
                .copied()
                .filter(|type_id| is_managed_type(*type_id, types))
                .and_then(|_| u32::try_from(index + 1).ok())
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(arguments.len() + 1)
            .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        &reference_slots,
    );
    lines.push(format!(
        "call i8 @{}(i64 {result}, i64 1, i64 {})",
        native_runtime_symbol(RuntimeOperation::FieldSet),
        case.raw()
    ));
    for (index, value) in arguments.iter().enumerate() {
        let type_id = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let ty = llvm_type(type_id, types)?;
        let (conversions, stored) = lower_runtime_slot_store(*value, type_id, &ty)?;
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::FieldSet),
            index + 2
        ));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn direct_function_tag(symbol: SymbolId) -> u64 {
    (1_u64 << 63) | u64::from(symbol.raw())
}

pub(crate) fn nested_function_tag(
    owner: SymbolId,
    function: pop_foundation::NestedFunctionId,
) -> u64 {
    ((u64::from(owner.raw()) << 32) | u64::from(function.raw())).saturating_add(1)
}

pub(crate) fn lower_capture_cell_allocate(
    result: &str,
    initial: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let type_id = *values
        .get(&initial)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (conversions, stored) =
        lower_runtime_slot_store(initial, type_id, &llvm_type(type_id, types)?)?;
    let reference_slots = is_managed_type(type_id, types)
        .then_some(0)
        .into_iter()
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(result, 1, &reference_slots);
    lines.extend(conversions);
    lines.push(format!(
        "call i8 @{}(i64 {result}, i64 1, i64 {stored})",
        native_runtime_symbol(RuntimeOperation::FieldSet)
    ));
    Ok(lines.join("\n"))
}

pub(crate) fn lower_closure_environment_allocate(
    result: &str,
    owner: SymbolId,
    function: pop_foundation::NestedFunctionId,
    captures: &[pop_mir::MirClosureCapture],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let reference_slots = captures
        .iter()
        .filter_map(|capture| {
            (capture.self_reference()
                || capture.mode() == pop_mir::MirCaptureMode::Cell
                || is_managed_type(capture.type_id(), types))
            .then_some(capture.slot() + 1)
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(captures.len() + 1)
            .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        &reference_slots,
    );
    lines.push(format!(
        "call i8 @{}(i64 {result}, i64 1, i64 {})",
        native_runtime_symbol(RuntimeOperation::FieldSet),
        nested_function_tag(owner, function)
    ));
    for capture in captures {
        let (conversions, stored) = if capture.self_reference() {
            (Vec::new(), result.to_owned())
        } else {
            let value = capture.value();
            let type_id = *values
                .get(&value)
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            lower_runtime_slot_store(value, type_id, &llvm_type(type_id, types)?)?
        };
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::FieldSet),
            capture.slot() + 2
        ));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn lower_capture_store(
    owner: &str,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let type_id = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) =
        lower_runtime_slot_store(value, type_id, &llvm_type(type_id, types)?)?;
    lines.push(format!(
        "call i8 @{}(i64 {owner}, i64 1, i64 {stored})",
        native_runtime_symbol(RuntimeOperation::FieldSet)
    ));
    Ok(lines.join("\n"))
}

pub(crate) fn lower_capture_load(
    result: ValueId,
    result_type: TypeId,
    environment: &str,
    slot: u32,
    mode: pop_mir::MirCaptureMode,
    self_reference: bool,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    if mode == pop_mir::MirCaptureMode::Value || self_reference {
        return lower_runtime_slot_load_from(
            result,
            result_type,
            environment,
            slot as usize + 2,
            types,
        )
        .map(|lines| lines.join("\n"));
    }
    let cell = format!("%v{}_cell", result.raw());
    let mut lines = vec![format!(
        "{cell} = call i64 @{}(i64 {environment}, i64 {})",
        native_runtime_symbol(RuntimeOperation::FieldGet),
        slot + 2
    )];
    lines.extend(lower_runtime_slot_load_from(
        result,
        result_type,
        &cell,
        1,
        types,
    )?);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_nested_capture_store(
    environment: &str,
    slot: u32,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let cell = format!("%capture_cell_{}", value.raw());
    let mut lines = vec![format!(
        "{cell} = call i64 @{}(i64 {environment}, i64 {})",
        native_runtime_symbol(RuntimeOperation::FieldGet),
        slot + 2
    )];
    lines.push(lower_capture_store(&cell, value, values, types)?);
    Ok(lines.join("\n"))
}

pub(crate) fn lower_runtime_slot_store(
    value: ValueId,
    type_id: TypeId,
    ty: &str,
) -> Result<(Vec<String>, String), LlvmLoweringError> {
    let source = format!("%v{}", value.raw());
    let converted = format!("%v{}_slot", value.raw());
    match ty {
        "i64" => Ok((Vec::new(), source)),
        "i1" | "i8" | "i16" | "i32" => Ok((
            vec![format!("{converted} = zext {ty} {source} to i64")],
            converted,
        )),
        "float" => Ok((
            vec![
                format!("{converted}_bits = bitcast float {source} to i32"),
                format!("{converted} = zext i32 {converted}_bits to i64"),
            ],
            converted,
        )),
        "double" => Ok((
            vec![format!("{converted} = bitcast double {source} to i64")],
            converted,
        )),
        "ptr" => Ok((
            vec![format!("{converted} = ptrtoint ptr {source} to i64")],
            converted,
        )),
        _ => Err(LlvmLoweringError::InvalidType(type_id)),
    }
}

pub(crate) fn lower_runtime_slot_load(
    result: ValueId,
    result_type: TypeId,
    owner: ValueId,
    slot: usize,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    lower_runtime_slot_load_from(
        result,
        result_type,
        &format!("%v{}", owner.raw()),
        slot,
        types,
    )
}

pub(crate) fn lower_runtime_slot_load_from(
    result: ValueId,
    result_type: TypeId,
    owner: &str,
    slot: usize,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    let result = format!("%v{}", result.raw());
    lower_runtime_slot_load_named(&result, result_type, owner, slot, types)
}

pub(crate) fn lower_runtime_slot_load_named(
    result: &str,
    result_type: TypeId,
    owner: &str,
    slot: usize,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    let ty = llvm_type(result_type, types)?;
    let loaded = format!("{result}_slot");
    let call = format!(
        "call i64 @{}(i64 {owner}, i64 {slot})",
        native_runtime_symbol(RuntimeOperation::FieldGet),
    );
    Ok(match ty.as_str() {
        "i64" => vec![format!("{result} = {call}")],
        "i1" | "i8" | "i16" | "i32" => vec![
            format!("{loaded} = {call}"),
            format!("{result} = trunc i64 {loaded} to {ty}"),
        ],
        "float" => vec![
            format!("{loaded} = {call}"),
            format!("{loaded}_bits = trunc i64 {loaded} to i32"),
            format!("{result} = bitcast i32 {loaded}_bits to float"),
        ],
        "double" => vec![
            format!("{loaded} = {call}"),
            format!("{result} = bitcast i64 {loaded} to double"),
        ],
        "ptr" => vec![
            format!("{loaded} = {call}"),
            format!("{result} = inttoptr i64 {loaded} to ptr"),
        ],
        _ => return Err(LlvmLoweringError::InvalidType(result_type)),
    })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn runtime_field_call(
    result: &str,
    result_type: Option<TypeId>,
    operation: RuntimeOperation,
    base: ValueId,
    field: FieldId,
    value: Option<ValueId>,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    let slot = field_layout
        .get(&field)
        .ok_or(LlvmLoweringError::InvalidFieldLayout(field))?;
    let base_type = llvm_value_type(values, base, types)?;
    if base_type != "i64" {
        return Err(LlvmLoweringError::InvalidType(*values.get(&base).ok_or(
            LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)),
        )?));
    }
    if let Some(value) = value {
        let type_id = *values
            .get(&value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (mut lines, stored) =
            lower_runtime_slot_store(value, type_id, &llvm_type(type_id, types)?)?;
        lines.push(format!(
            "call i8 @{}(i64 %v{}, i64 {}, i64 {stored})",
            native_runtime_symbol(operation),
            base.raw(),
            slot
        ));
        return Ok(lines.join("\n"));
    }
    let result_type =
        result_type.ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    lower_runtime_slot_load_named(
        result,
        result_type,
        &format!("%v{}", base.raw()),
        *slot as usize,
        types,
    )
    .map(|lines| lines.join("\n"))
}

pub(crate) fn lower_array_make(
    result: &str,
    elements: &[ValueId],
    element_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 {}, {})",
        native_runtime_symbol(RuntimeOperation::AllocateArray),
        elements.len(),
        if matches!(element_map, ArrayElementMap::ManagedReference) {
            "i1 1"
        } else {
            "i1 0"
        }
    )];
    for (index, value) in elements.iter().enumerate() {
        let type_id = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (conversions, stored) =
            lower_runtime_slot_store(*value, type_id, &llvm_type(type_id, types)?)?;
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            native_runtime_symbol(RuntimeOperation::ArraySet),
            index + 1
        ));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn lower_table_make(
    result: &str,
    entries: &[(ValueId, ValueId)],
    key_map: ArrayElementMap,
    value_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 {}, i1 {}, i1 {})",
        native_runtime_symbol(RuntimeOperation::AllocateTable),
        entries.len(),
        u8::from(key_map == ArrayElementMap::ManagedReference),
        u8::from(value_map == ArrayElementMap::ManagedReference),
    )];
    for (key, value) in entries {
        let key_type = *values
            .get(key)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let value_type = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (key_conversions, stored_key) =
            lower_runtime_slot_store(*key, key_type, &llvm_type(key_type, types)?)?;
        let (value_conversions, stored_value) =
            lower_runtime_slot_store(*value, value_type, &llvm_type(value_type, types)?)?;
        lines.extend(key_conversions);
        lines.extend(value_conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {stored_key}, i64 {stored_value}, i1 {}, i1 {})",
            native_runtime_symbol(RuntimeOperation::TableSet),
            u8::from(key_map == ArrayElementMap::ManagedReference),
            u8::from(value_map == ArrayElementMap::ManagedReference),
        ));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn lower_table_get(
    result: &str,
    table: ValueId,
    key: ValueId,
    result_type: TypeId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let inner = optional_inner_type(types, result_type)
        .ok_or(LlvmLoweringError::InvalidType(result_type))?;
    let inner_type = llvm_type(inner, types)?;
    let key_type = *values
        .get(&key)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored_key) =
        lower_runtime_slot_store(key, key_type, &llvm_type(key_type, types)?)?;
    let output = format!("{result}_output");
    let status = format!("{result}_status");
    let present = format!("{result}_present");
    let payload = format!("{result}_payload");
    let partial = format!("{result}_partial");
    lines.extend([
        format!("store i64 0, ptr {output}"),
        format!(
            "{status} = call i8 @{}(i64 %v{}, i64 {stored_key}, i1 {}, ptr {output})",
            pop_runtime_native_abi::TABLE_GET_CHECKED_SYMBOL,
            table.raw(),
            u8::from(is_managed_type(key_type, types)),
        ),
        format!("{present} = icmp ne i8 {status}, 0"),
    ]);
    lines.extend(lower_array_output_load(&payload, inner, &output, types)?);
    lines.extend([
        format!("{partial} = insertvalue {{ i1, {inner_type} }} zeroinitializer, i1 {present}, 0"),
        format!(
            "{result} = insertvalue {{ i1, {inner_type} }} {partial}, {inner_type} {payload}, 1"
        ),
    ]);
    Ok(lines.join("\n"))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_table_set(
    result: &str,
    table: ValueId,
    key: ValueId,
    value: ValueId,
    key_map: ArrayElementMap,
    value_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let key_type = *values
        .get(&key)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored_key) =
        lower_runtime_slot_store(key, key_type, &llvm_type(key_type, types)?)?;
    let (value_conversions, stored_value) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    lines.extend(value_conversions);
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result}_stored = call i8 @{}(i64 %v{}, i64 {stored_key}, i64 {stored_value}, i1 {}, i1 {})",
            native_runtime_symbol(RuntimeOperation::TableSet),
            table.raw(),
            u8::from(key_map == ArrayElementMap::ManagedReference),
            u8::from(value_map == ArrayElementMap::ManagedReference),
        ),
        format!("{result}_valid = icmp ne i8 {result}_stored, 0"),
        format!("br i1 {result}_valid, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

pub(crate) fn llvm_results(
    results: &[TypeId],
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    match results {
        [] => Ok("void".to_owned()),
        [result] => llvm_type(*result, types),
        _ => Ok(format!(
            "{{ {} }}",
            results
                .iter()
                .map(|id| llvm_type(*id, types))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )),
    }
}

pub(crate) fn llvm_value_type(
    values: &BTreeMap<ValueId, TypeId>,
    value: ValueId,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    llvm_type(
        *values
            .get(&value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        types,
    )
}

pub(crate) fn llvm_type(type_id: TypeId, types: &TypeArena) -> Result<String, LlvmLoweringError> {
    if let Some(inner) = optional_inner_type(types, type_id) {
        return Ok(format!("{{ i1, {} }}", llvm_type(inner, types)?));
    }
    match types
        .get(type_id)
        .ok_or(LlvmLoweringError::InvalidType(type_id))?
    {
        SemanticType::Primitive(PrimitiveType::Boolean) => Ok("i1".to_owned()),
        SemanticType::Primitive(PrimitiveType::Integer(kind)) => {
            Ok(format!("i{}", kind.bit_width()))
        }
        SemanticType::Primitive(PrimitiveType::Float32) => Ok("float".to_owned()),
        SemanticType::Primitive(PrimitiveType::Float64) => Ok("double".to_owned()),
        SemanticType::Primitive(PrimitiveType::Never) => Ok("void".to_owned()),
        SemanticType::Enum { .. } => Ok("i32".to_owned()),
        _ => Ok("i64".to_owned()),
    }
}

pub(crate) fn optional_inner_type(types: &TypeArena, optional: TypeId) -> Option<TypeId> {
    let nil = types.source_type("nil")?;
    let SemanticType::Union(members) = types.get(optional)? else {
        return None;
    };
    if !members.contains(&nil) {
        return None;
    }
    let present = members
        .iter()
        .copied()
        .filter(|member| *member != nil)
        .collect::<Vec<_>>();
    match present.as_slice() {
        [inner] => Some(*inner),
        [] => None,
        _ => types.find(&SemanticType::Union(present)),
    }
}

pub(crate) fn integer_literal(value: pop_types::IntegerValue) -> String {
    if value.kind().is_signed() {
        value.signed().unwrap_or_default().to_string()
    } else {
        value.unsigned().unwrap_or_default().to_string()
    }
}
pub(crate) fn float_type(kind: FloatKind) -> &'static str {
    match kind {
        FloatKind::Float32 => "float",
        FloatKind::Float64 => "double",
    }
}
