//! Deterministic rendering for canonical MIR dumps.
//!
//! Rendering is tooling output only. Backends consume verified MIR values
//! directly and must never reconstruct semantics by parsing this text.

use std::fmt::Write;

use pop_foundation::{FieldId, MethodId, SymbolId, TypeId, ValueId};
use pop_runtime_interface::{ArrayElementMap, ObjectMap, PanicPayload, UnwindReason};
use pop_types::{FloatKind, FloatValue, IntegerKind};

use crate::ir::*;

pub(crate) fn dump_declaration(output: &mut String, declaration: &MirDeclaration) {
    match &declaration.kind {
        MirDeclarationKind::Record(record) => {
            let _ = write!(
                output,
                "type.record s{} t{} fields ",
                declaration.symbol.raw(),
                record.type_id.raw()
            );
            dump_declared_fields(output, &record.fields);
        }
        MirDeclarationKind::Union(union) => {
            let _ = write!(
                output,
                "type.union s{} t{} cases ",
                declaration.symbol.raw(),
                union.type_id.raw()
            );
            dump_union_cases(output, &union.cases);
        }
        MirDeclarationKind::Error(error) => {
            let _ = write!(
                output,
                "type.error s{} e{} t{} cases ",
                declaration.symbol.raw(),
                error.error.raw(),
                error.type_id.raw()
            );
            if error.cases.is_empty() {
                output.push('-');
            } else {
                for (index, case) in error.cases.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    let _ = write!(output, "errorCase#{}(", case.case.raw());
                    dump_type_ids(output, &case.parameters);
                    output.push(')');
                }
            }
        }
        MirDeclarationKind::Enum(enumeration) => {
            let _ = write!(
                output,
                "type.enum s{} t{} cases ",
                declaration.symbol.raw(),
                enumeration.type_id.raw()
            );
            if enumeration.cases.is_empty() {
                output.push('-');
            } else {
                for (index, case) in enumeration.cases.iter().enumerate() {
                    if index > 0 {
                        output.push(',');
                    }
                    let _ = write!(output, "case#{}={}", case.case.raw(), case.discriminant);
                }
            }
        }
        MirDeclarationKind::Class(class) => {
            let _ = write!(
                output,
                "type.class s{} c{} t{} fields ",
                declaration.symbol.raw(),
                class.class.raw(),
                class.type_id.raw()
            );
            dump_declared_fields(output, &class.fields);
            output.push_str(" methods ");
            dump_method_ids(output, &class.methods);
            output.push_str(" implements ");
            dump_interface_implementations(output, &class.interfaces);
            output.push_str(" implementsBuiltin ");
            dump_builtin_interface_implementations(output, &class.builtin_interfaces);
        }
        MirDeclarationKind::Interface(interface) => {
            let _ = write!(
                output,
                "type.interface s{} i{} t{} methods ",
                declaration.symbol.raw(),
                interface.interface.raw(),
                interface.type_id.raw()
            );
            dump_interface_methods(output, &interface.methods);
        }
    }
    output.push('\n');
}

fn dump_interface_methods(output: &mut String, methods: &[MirInterfaceMethod]) {
    if methods.is_empty() {
        output.push('-');
        return;
    }
    for (index, method) in methods.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let _ = write!(output, "im{}@{}(", method.method.raw(), method.slot);
        dump_type_ids(output, &method.parameters);
        output.push_str(")->(");
        dump_type_ids(output, &method.results);
        output.push(')');
    }
}

fn dump_interface_implementations(
    output: &mut String,
    implementations: &[MirInterfaceImplementation],
) {
    if implementations.is_empty() {
        output.push('-');
        return;
    }
    for (index, implementation) in implementations.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let _ = write!(
            output,
            "i{}:t{}[",
            implementation.interface.raw(),
            implementation.interface_type.raw()
        );
        for (method_index, method) in implementation.methods.iter().enumerate() {
            if method_index > 0 {
                output.push(';');
            }
            let _ = write!(
                output,
                "im{}@{}=m{}",
                method.interface_method.raw(),
                method.slot,
                method.class_method.raw()
            );
        }
        output.push(']');
    }
}

fn dump_builtin_interface_implementations(
    output: &mut String,
    implementations: &[MirBuiltinInterfaceImplementation],
) {
    if implementations.is_empty() {
        output.push('-');
        return;
    }
    for (index, implementation) in implementations.iter().enumerate() {
        if index > 0 {
            output.push(',');
        }
        let _ = write!(
            output,
            "b{}:t{}[",
            implementation.interface.raw(),
            implementation.interface_type.raw()
        );
        for (method_index, method) in implementation.methods.iter().enumerate() {
            if method_index > 0 {
                output.push(';');
            }
            let _ = write!(
                output,
                "iterationMethod#{}=m{}",
                method.protocol_method.raw(),
                method.class_method.raw()
            );
        }
        output.push(']');
    }
}

fn dump_type_ids(output: &mut String, types: &[TypeId]) {
    for (index, type_id) in types.iter().enumerate() {
        if index > 0 {
            output.push(';');
        }
        let _ = write!(output, "t{}", type_id.raw());
    }
}

fn dump_declared_fields(output: &mut String, fields: &[MirField]) {
    if fields.is_empty() {
        output.push('-');
        return;
    }
    for (index, field) in fields.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(
            output,
            "field#{}:t{}",
            field.field.raw(),
            field.field_type.raw()
        );
    }
}

fn dump_union_cases(output: &mut String, cases: &[MirUnionCase]) {
    if cases.is_empty() {
        output.push('-');
        return;
    }
    for (index, case) in cases.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "case#{}(", case.case.raw());
        for (parameter_index, parameter) in case.parameters.iter().enumerate() {
            if parameter_index != 0 {
                output.push(';');
            }
            let _ = write!(output, "t{}", parameter.raw());
        }
        output.push(')');
    }
}

fn dump_method_ids(output: &mut String, methods: &[MethodId]) {
    if methods.is_empty() {
        output.push('-');
        return;
    }
    for (index, method) in methods.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "m{}", method.raw());
    }
}

pub(crate) fn dump_function(output: &mut String, function: &MirFunction) {
    if function.is_async {
        output.push_str("async ");
    }
    let _ = write!(
        output,
        "function s{} f{}(",
        function.symbol.raw(),
        function.function.raw()
    );
    for (index, parameter) in function.parameters.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "t{}", parameter.raw());
    }
    output.push_str(") -> (");
    for (index, result) in function.results.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "t{}", result.raw());
    }
    output.push_str(") effects[");
    dump_effects(output, function.effects);
    output.push_str("]\n");
    dump_blocks(output, &function.blocks);
}

pub(crate) fn dump_function_reference(output: &mut String, reference: &MirFunctionReference) {
    if reference.is_async {
        output.push_str("async ");
    }
    let _ = write!(
        output,
        "reference b{}:s{} params(",
        reference.identity.bubble().raw(),
        reference.identity.symbol().raw()
    );
    dump_type_ids(output, &reference.parameters);
    output.push_str(") results(");
    dump_type_ids(output, &reference.results);
    output.push_str(") effects[");
    dump_effects(output, reference.effects);
    output.push_str("]\n");
}

pub(crate) fn dump_nested_function(output: &mut String, function: &MirNestedFunction) {
    if function.is_async {
        output.push_str("async ");
    }
    let _ = write!(
        output,
        "nested s{} nf{} captures ",
        function.owner.raw(),
        function.function.raw()
    );
    if function.captures.is_empty() {
        output.push('-');
    } else {
        for (index, capture) in function.captures.iter().enumerate() {
            if index > 0 {
                output.push(',');
            }
            let mode = match capture.mode {
                MirCaptureMode::Value => "value",
                MirCaptureMode::Cell => "cell",
            };
            let _ = write!(
                output,
                "cap{}:bind{}@{}:t{}:{mode}",
                capture.capture.raw(),
                capture.binding.raw(),
                capture.slot,
                capture.type_id.raw()
            );
        }
    }
    output.push_str(" params(");
    dump_type_ids(output, &function.parameters);
    output.push_str(") results(");
    dump_type_ids(output, &function.results);
    output.push_str(") effects[");
    dump_effects(output, function.effects);
    output.push_str("]\n");
    dump_blocks(output, &function.blocks);
}

fn dump_blocks(output: &mut String, blocks: &[MirBlock]) {
    for block in blocks {
        let _ = write!(output, "  b{}(", block.block.raw());
        for (index, argument) in block.arguments.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            let _ = write!(
                output,
                "v{}:t{}",
                argument.value.raw(),
                argument.type_id.raw()
            );
        }
        output.push(')');
        if let Some(cleanup) = block.cleanup {
            let _ = write!(
                output,
                " cleanup scope#{} reason {}",
                cleanup.scope.raw(),
                dump_cleanup_reason(cleanup.reason)
            );
        }
        output.push_str(":\n");
        for instruction in &block.instructions {
            if let Some(result_type) = instruction.result_type {
                let _ = write!(
                    output,
                    "    v{}:t{} = ",
                    instruction.result.raw(),
                    result_type.raw()
                );
            } else {
                let _ = write!(output, "    do v{} ", instruction.result.raw());
            }
            dump_instruction(output, &instruction.kind);
            if let MirUnwindAction::Cleanup(target) = instruction.unwind {
                let _ = write!(output, " unwind cleanup:b{}", target.raw());
            }
            output.push('\n');
        }
        output.push_str("    ");
        dump_terminator(output, &block.terminator);
        output.push('\n');
    }
}

const fn dump_cleanup_reason(reason: MirCleanupExitReason) -> &'static str {
    match reason {
        MirCleanupExitReason::Normal => "normal",
        MirCleanupExitReason::Return => "return",
        MirCleanupExitReason::ResultFailure => "resultFailure",
        MirCleanupExitReason::Break => "break",
        MirCleanupExitReason::Continue => "continue",
        MirCleanupExitReason::Unwind => "unwind",
        MirCleanupExitReason::Cancellation => "cancellation",
    }
}

fn dump_instruction(output: &mut String, instruction: &MirInstructionKind) {
    if dump_numeric_instruction(output, instruction)
        || dump_callable_or_schema_instruction(output, instruction)
    {
        return;
    }
    match instruction {
        MirInstructionKind::StringConstant(value) => {
            let _ = write!(output, "const.string {value:?}");
        }
        MirInstructionKind::StringConcat { left, right } => {
            dump_binary(output, "string.concat", *left, *right);
        }
        MirInstructionKind::StringFormat { kind, value } => {
            let _ = write!(output, "string.format {kind:?} v{}", value.raw());
        }
        MirInstructionKind::BooleanConstant(value) => {
            let _ = write!(output, "const.boolean {value}");
        }
        MirInstructionKind::NilConstant => output.push_str("const.nil"),
        MirInstructionKind::OptionalIsPresent { optional } => {
            dump_unary(output, "optionalIsPresent", *optional);
        }
        MirInstructionKind::OptionalGet { optional } => {
            dump_unary(output, "optionalGet", *optional);
        }
        MirInstructionKind::ResultMake {
            result,
            case,
            arguments,
        } => {
            let _ = write!(
                output,
                "resultMake bt{} resultCase#{} ",
                result.raw(),
                case.raw()
            );
            dump_value_list(output, arguments);
        }
        MirInstructionKind::IterationMake {
            iteration,
            case,
            arguments,
        } => {
            let _ = write!(
                output,
                "iterationMake bt{} iterationCase#{} ",
                iteration.raw(),
                case.raw()
            );
            dump_value_list(output, arguments);
        }
        MirInstructionKind::ErrorMake {
            error,
            case,
            arguments,
        } => {
            let _ = write!(
                output,
                "errorMake e{} errorCase#{} ",
                error.raw(),
                case.raw()
            );
            dump_value_list(output, arguments);
        }
        MirInstructionKind::ResultIsOk { result, definition } => {
            let _ = write!(
                output,
                "resultIsOk bt{} v{}",
                definition.raw(),
                result.raw()
            );
        }
        MirInstructionKind::ResultGetOk { result, definition } => {
            let _ = write!(
                output,
                "resultGetOk bt{} v{}",
                definition.raw(),
                result.raw()
            );
        }
        MirInstructionKind::ResultGetError { result, definition } => {
            let _ = write!(
                output,
                "resultGetError bt{} v{}",
                definition.raw(),
                result.raw()
            );
        }
        MirInstructionKind::EnumConstant {
            definition,
            case,
            discriminant,
        } => {
            let _ = write!(
                output,
                "enum.case s{} ec{} {}",
                definition.raw(),
                case.raw(),
                discriminant
            );
        }
        MirInstructionKind::FunctionReference(function) => {
            let _ = write!(output, "functionReference s{}", function.raw());
        }
        MirInstructionKind::TaskCreate {
            dispatch,
            arguments,
            completion_type,
            object_map,
        } => {
            output.push_str("task.create ");
            match dispatch {
                MirTaskDispatch::Direct(function) => {
                    let _ = write!(output, "direct:s{}", function.raw());
                }
                MirTaskDispatch::Referenced(function) => {
                    let _ = write!(
                        output,
                        "reference:b{}:s{}",
                        function.bubble().raw(),
                        function.symbol().raw()
                    );
                }
                MirTaskDispatch::Indirect(callee) => {
                    let _ = write!(output, "indirect:v{}", callee.raw());
                }
            }
            let _ = write!(output, " completion:t{} ", completion_type.raw());
            dump_object_map(output, object_map);
            output.push_str(" args ");
            dump_value_list(output, arguments);
        }
        MirInstructionKind::TupleMake(values) => dump_values(output, "tupleMake", values),
        MirInstructionKind::TupleGet { tuple, index } => {
            let _ = write!(output, "tupleGet {index} v{}", tuple.raw());
        }
        MirInstructionKind::ArrayMake {
            elements,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(output, "arrayMake {map} ");
            dump_value_list(output, elements);
        }
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(
                output,
                "arrayCreate {map} v{} v{}",
                length.raw(),
                initial_value.raw()
            );
        }
        MirInstructionKind::TableMake {
            entries,
            key_map,
            value_map,
        } => {
            let key_map = array_element_map_name(*key_map);
            let value_map = array_element_map_name(*value_map);
            let _ = write!(output, "tableMake {key_map} {value_map} ");
            dump_table_entries(output, entries);
        }
        MirInstructionKind::TableGet { table, key } => {
            dump_binary(output, "tableGet", *table, *key);
        }
        MirInstructionKind::TableSet {
            table,
            key,
            value,
            key_map,
            value_map,
        } => {
            let key_map = array_element_map_name(*key_map);
            let value_map = array_element_map_name(*value_map);
            let _ = write!(
                output,
                "tableSet {key_map} {value_map} v{} v{} v{}",
                table.raw(),
                key.raw(),
                value.raw()
            );
        }
        MirInstructionKind::ArrayGet { array, index } => {
            dump_binary(output, "arrayGet", *array, *index);
        }
        MirInstructionKind::ArrayLength { array } => {
            let _ = write!(output, "arrayLength v{}", array.raw());
        }
        MirInstructionKind::ArrayGetChecked { array, index } => {
            dump_binary(output, "arrayGetChecked", *array, *index);
        }
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(
                output,
                "arraySet {map} v{} v{} v{}",
                array.raw(),
                index.raw(),
                value.raw()
            );
        }
        MirInstructionKind::ArrayFill {
            array,
            value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(output, "arrayFill {map} v{} v{}", array.raw(), value.raw());
        }
        MirInstructionKind::ListCreate {
            capacity,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let capacity =
                capacity.map_or_else(|| "none".to_owned(), |value| format!("v{}", value.raw()));
            let _ = write!(output, "listCreate {map} {capacity}");
        }
        MirInstructionKind::ListLength { list } => {
            let _ = write!(output, "listLength v{}", list.raw());
        }
        MirInstructionKind::ListGet { list, index } => {
            dump_binary(output, "listGet", *list, *index);
        }
        MirInstructionKind::ListGetChecked { list, index } => {
            dump_binary(output, "listGetChecked", *list, *index);
        }
        MirInstructionKind::ListSet {
            list,
            index,
            value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(
                output,
                "listSet {map} v{} v{} v{}",
                list.raw(),
                index.raw(),
                value.raw()
            );
        }
        MirInstructionKind::ListAdd {
            list,
            value,
            element_map,
        } => {
            let map = array_element_map_name(*element_map);
            let _ = write!(output, "listAdd {map} v{} v{}", list.raw(), value.raw());
        }
        MirInstructionKind::RangeCreate { first, last, step } => {
            let _ = write!(
                output,
                "rangeCreate v{} v{} v{}",
                first.raw(),
                last.raw(),
                step.raw()
            );
        }
        binary @ (MirInstructionKind::BooleanAnd { .. }
        | MirInstructionKind::BooleanOr { .. }
        | MirInstructionKind::CompareEqual { .. }
        | MirInstructionKind::CompareNotEqual { .. }) => dump_binary_instruction(output, binary),
        MirInstructionKind::BooleanNot { operand } => dump_unary(output, "booleanNot", *operand),
        MirInstructionKind::GcSafePoint {
            safe_point, roots, ..
        } => {
            let _ = write!(output, "gcSafePoint sp{} roots ", safe_point.raw());
            dump_value_list(output, roots);
        }
        MirInstructionKind::RetainRoot { value } => {
            let _ = write!(output, "retainRoot v{}", value.raw());
        }
        MirInstructionKind::ReleaseRoot { handle } => {
            let _ = write!(output, "releaseRoot v{}", handle.raw());
        }
        MirInstructionKind::Pin { value } => {
            let _ = write!(output, "pin v{}", value.raw());
        }
        MirInstructionKind::Unpin { handle } => {
            let _ = write!(output, "unpin v{}", handle.raw());
        }
        MirInstructionKind::WriteBarrier {
            owner,
            slot,
            previous,
            value,
        } => {
            let _ = write!(
                output,
                "writeBarrier v{} slot {} previous ",
                owner.raw(),
                slot.raw()
            );
            dump_optional_value(output, *previous);
            output.push_str(" value ");
            dump_optional_value(output, *value);
        }
        _ => unreachable!("specialized MIR dumper accepts every remaining instruction"),
    }
}

const fn array_element_map_name(map: ArrayElementMap) -> &'static str {
    match map {
        ArrayElementMap::Scalar => "scalar",
        ArrayElementMap::ManagedReference => "managed",
    }
}

fn dump_numeric_instruction(output: &mut String, instruction: &MirInstructionKind) -> bool {
    if dump_numeric_binary_instruction(output, instruction) {
        return true;
    }
    match instruction {
        MirInstructionKind::IntegerConstant(value) => {
            let _ = write!(
                output,
                "const.integer {} {value}",
                integer_kind_text(value.kind())
            );
        }
        MirInstructionKind::FloatConstant(value) => {
            let _ = match value {
                FloatValue::Float32(bits) => {
                    write!(output, "const.float Float32 0x{bits:08x}")
                }
                FloatValue::Float64(bits) => {
                    write!(output, "const.float Float64 0x{bits:016x}")
                }
            };
        }
        MirInstructionKind::IntegerNegate { kind, operand } => {
            dump_numeric_unary(output, "integer.negate", integer_kind_text(*kind), *operand);
        }
        MirInstructionKind::FloatNegate { kind, operand } => {
            dump_numeric_unary(output, "float.negate", float_kind_text(*kind), *operand);
        }
        MirInstructionKind::ConvertInteger {
            source,
            target,
            operand,
        } => {
            let _ = write!(
                output,
                "numeric.integerToInteger {} {} v{}",
                integer_kind_text(*source),
                integer_kind_text(*target),
                operand.raw()
            );
        }
        MirInstructionKind::ConvertIntegerToFloat {
            source,
            target,
            operand,
        } => {
            let _ = write!(
                output,
                "numeric.integerToFloat {} {} v{}",
                integer_kind_text(*source),
                float_kind_text(*target),
                operand.raw()
            );
        }
        MirInstructionKind::ConvertFloatToInteger {
            source,
            target,
            operand,
        } => {
            let _ = write!(
                output,
                "numeric.floatToInteger {} {} v{}",
                float_kind_text(*source),
                integer_kind_text(*target),
                operand.raw()
            );
        }
        MirInstructionKind::ConvertFloat {
            source,
            target,
            operand,
        } => {
            let _ = write!(
                output,
                "numeric.floatToFloat {} {} v{}",
                float_kind_text(*source),
                float_kind_text(*target),
                operand.raw()
            );
        }
        _ => return false,
    }
    true
}

fn dump_numeric_binary_instruction(output: &mut String, instruction: &MirInstructionKind) -> bool {
    let (name, kind, left, right) = match instruction {
        MirInstructionKind::CheckedIntegerAdd { kind, left, right } => {
            ("integer.checkedAdd", integer_kind_text(*kind), left, right)
        }
        MirInstructionKind::CheckedIntegerSubtract { kind, left, right } => (
            "integer.checkedSubtract",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CheckedIntegerMultiply { kind, left, right } => (
            "integer.checkedMultiply",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CheckedIntegerDivide { kind, left, right } => (
            "integer.checkedDivide",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => (
            "integer.checkedRemainder",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::FloatAdd { kind, left, right } => {
            ("float.add", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::FloatSubtract { kind, left, right } => {
            ("float.subtract", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::FloatMultiply { kind, left, right } => {
            ("float.multiply", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::FloatDivide { kind, left, right } => {
            ("float.divide", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::CompareIntegerLess { kind, left, right } => {
            ("integer.compareLess", integer_kind_text(*kind), left, right)
        }
        MirInstructionKind::CompareIntegerLessOrEqual { kind, left, right } => (
            "integer.compareLessOrEqual",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CompareIntegerGreater { kind, left, right } => (
            "integer.compareGreater",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CompareIntegerGreaterOrEqual { kind, left, right } => (
            "integer.compareGreaterOrEqual",
            integer_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CompareFloatLess { kind, left, right } => {
            ("float.compareLess", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::CompareFloatLessOrEqual { kind, left, right } => (
            "float.compareLessOrEqual",
            float_kind_text(*kind),
            left,
            right,
        ),
        MirInstructionKind::CompareFloatGreater { kind, left, right } => {
            ("float.compareGreater", float_kind_text(*kind), left, right)
        }
        MirInstructionKind::CompareFloatGreaterOrEqual { kind, left, right } => (
            "float.compareGreaterOrEqual",
            float_kind_text(*kind),
            left,
            right,
        ),
        _ => return false,
    };
    dump_numeric_binary(output, name, kind, *left, *right);
    true
}

fn dump_callable_or_schema_instruction(
    output: &mut String,
    instruction: &MirInstructionKind,
) -> bool {
    match instruction {
        MirInstructionKind::CallStandard {
            function,
            arguments,
            declared_effects,
        } => {
            let _ = write!(output, "callStandard sf{} ", function.raw());
            dump_value_list(output, arguments);
            output.push_str(" effects[");
            dump_effects(output, *declared_effects);
            output.push(']');
        }
        MirInstructionKind::CallDirect {
            function,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(output, "callDirect s{} ", function.raw());
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallReferenced {
            function,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(
                output,
                "callReference b{}:s{} ",
                function.bubble().raw(),
                function.symbol().raw()
            );
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallDirectMethod {
            method,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(output, "callDirectMethod m{} ", method.raw());
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallInterface {
            interface,
            method,
            slot,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(
                output,
                "call.interface i{} im{} slot#{} ",
                interface.raw(),
                method.raw(),
                slot
            );
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallBuiltinInterface {
            interface,
            method,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(
                output,
                "call.builtinInterface interface#{} method#{} ",
                interface.raw(),
                method.raw()
            );
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::CallIndirect {
            callee,
            arguments,
            declared_effects,
            unwind,
        } => {
            let _ = write!(output, "callIndirect v{} ", callee.raw());
            dump_value_list(output, arguments);
            dump_call_contract(output, *declared_effects, *unwind);
        }
        MirInstructionKind::RecordMake { record, fields } => {
            dump_fields(output, "recordMake", *record, None, fields);
        }
        MirInstructionKind::ClassMake {
            class,
            fields,
            object_map,
        } => {
            let _ = write!(output, "classMake c{} ", class.raw());
            dump_object_map(output, object_map);
            output.push(' ');
            dump_field_values(output, fields);
        }
        MirInstructionKind::RecordUpdate {
            record,
            base,
            fields,
        } => dump_fields(output, "recordUpdate", *record, Some(*base), fields),
        MirInstructionKind::FieldGet { base, field } => {
            let _ = write!(output, "fieldGet v{} field#{}", base.raw(), field.raw());
        }
        MirInstructionKind::FieldSet { base, field, value } => {
            let _ = write!(
                output,
                "fieldSet v{} field#{} v{}",
                base.raw(),
                field.raw(),
                value.raw()
            );
        }
        MirInstructionKind::UnionMake {
            union,
            case,
            arguments,
        } => {
            let _ = write!(output, "unionMake s{} case#{} ", union.raw(), case.raw());
            dump_value_list(output, arguments);
        }
        MirInstructionKind::IterationIsItem {
            iteration,
            definition,
            item_case,
            end_case,
        } => {
            let _ = write!(
                output,
                "iteration.isItem definition#{} case#{} endCase#{} v{}",
                definition.raw(),
                item_case.raw(),
                end_case.raw(),
                iteration.raw()
            );
        }
        MirInstructionKind::IterationGetItem {
            iteration,
            definition,
            item_case,
        } => {
            let _ = write!(
                output,
                "iteration.getItem definition#{} case#{} v{}",
                definition.raw(),
                item_case.raw(),
                iteration.raw()
            );
        }
        MirInstructionKind::InterfaceUpcast { value, interface } => {
            let (prefix, raw) = match interface {
                pop_foundation::NominalInterfaceId::User(interface) => ('i', interface.raw()),
                pop_foundation::NominalInterfaceId::Builtin(interface) => ('b', interface.raw()),
            };
            let _ = write!(output, "interface.upcast v{} {prefix}{raw}", value.raw());
        }
        MirInstructionKind::CaptureCellAllocate {
            binding,
            initial,
            value_type,
            object_map,
        } => {
            let _ = write!(
                output,
                "captureCell.allocate bind{} v{} t{} ",
                binding.raw(),
                initial.raw(),
                value_type.raw()
            );
            dump_object_map(output, object_map);
        }
        MirInstructionKind::CaptureCellLoad { cell } => {
            let _ = write!(output, "captureCell.load v{}", cell.raw());
        }
        MirInstructionKind::CaptureCellStore { cell, value } => {
            let _ = write!(output, "captureCell.store v{} v{}", cell.raw(), value.raw());
        }
        MirInstructionKind::ClosureEnvironmentAllocate {
            owner,
            function,
            captures,
            object_map,
        } => {
            let _ = write!(
                output,
                "closureEnvironment.allocate s{} nf{} ",
                owner.raw(),
                function.raw()
            );
            dump_object_map(output, object_map);
            output.push_str(" captures[");
            for (index, capture) in captures.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let mode = match capture.mode {
                    MirCaptureMode::Value => "value",
                    MirCaptureMode::Cell => "cell",
                };
                if capture.self_reference {
                    let _ = write!(
                        output,
                        "cap{}:bind{}@{}=self:t{}:{mode}",
                        capture.capture.raw(),
                        capture.binding.raw(),
                        capture.slot,
                        capture.type_id.raw()
                    );
                } else {
                    let _ = write!(
                        output,
                        "cap{}:bind{}@{}=v{}:t{}:{mode}",
                        capture.capture.raw(),
                        capture.binding.raw(),
                        capture.slot,
                        capture.value.raw(),
                        capture.type_id.raw()
                    );
                }
            }
            output.push(']');
        }
        MirInstructionKind::CaptureLoad {
            capture,
            slot,
            mode,
        } => {
            let mode = match mode {
                MirCaptureMode::Value => "value",
                MirCaptureMode::Cell => "cell",
            };
            let _ = write!(
                output,
                "capture.load cap{} slot#{} {mode}",
                capture.raw(),
                slot
            );
        }
        MirInstructionKind::CaptureCellReference { capture, slot } => {
            let _ = write!(output, "capture.cell cap{} slot#{}", capture.raw(), slot);
        }
        MirInstructionKind::CaptureStore {
            capture,
            slot,
            value,
        } => {
            let _ = write!(
                output,
                "capture.store cap{} slot#{} v{}",
                capture.raw(),
                slot,
                value.raw()
            );
        }
        _ => return false,
    }
    true
}

fn dump_binary_instruction(output: &mut String, instruction: &MirInstructionKind) {
    let (name, left, right) = match instruction {
        MirInstructionKind::BooleanAnd { left, right } => ("booleanAnd", left, right),
        MirInstructionKind::BooleanOr { left, right } => ("booleanOr", left, right),
        MirInstructionKind::CompareEqual { left, right } => ("compareEqual", left, right),
        MirInstructionKind::CompareNotEqual { left, right } => ("compareNotEqual", left, right),
        _ => unreachable!("binary MIR dumper accepts only binary instructions"),
    };
    dump_binary(output, name, *left, *right);
}

fn dump_terminator(output: &mut String, terminator: &MirTerminator) {
    match terminator {
        MirTerminator::Missing => output.push_str("missing"),
        MirTerminator::Branch { target, arguments } => {
            let _ = write!(output, "branch b{} ", target.raw());
            dump_value_list(output, arguments);
        }
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => {
            let _ = write!(
                output,
                "condBranch v{} b{} b{}",
                condition.raw(),
                when_true.raw(),
                when_false.raw()
            );
        }
        MirTerminator::UnionSwitch {
            scrutinee,
            union,
            arms,
        } => {
            let _ = write!(
                output,
                "union.switch v{} s{} [",
                scrutinee.raw(),
                union.raw()
            );
            for (index, arm) in arms.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let _ = write!(output, "case#{}:b{}", arm.case.raw(), arm.target.raw());
            }
            output.push(']');
        }
        MirTerminator::ErrorSwitch {
            scrutinee,
            error,
            arms,
        } => {
            let _ = write!(
                output,
                "errorSwitch v{} e{} [",
                scrutinee.raw(),
                error.raw()
            );
            for (index, arm) in arms.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let _ = write!(output, "errorCase#{}:b{}", arm.case.raw(), arm.target.raw());
            }
            output.push(']');
        }
        MirTerminator::Suspend {
            operation: MirSuspendOperation::Task { task, result_type },
            resume,
            cancellation,
            unwind,
            safe_point,
            live_frame,
        } => {
            let _ = write!(
                output,
                "suspend.task v{} result:t{} resume:b{} cancellation:b{} unwind:",
                task.raw(),
                result_type.raw(),
                resume.raw(),
                cancellation.raw()
            );
            match unwind {
                MirUnwindAction::Propagate => output.push_str("propagate"),
                MirUnwindAction::Cleanup(target) => {
                    let _ = write!(output, "cleanup:b{}", target.raw());
                }
            }
            let _ = write!(
                output,
                " safePoint:sp{} state:cs{} frame[",
                safe_point.raw(),
                live_frame.state.raw()
            );
            for (index, slot) in live_frame.slots.iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let _ = write!(output, "v{}:t{}", slot.value.raw(), slot.type_id.raw());
            }
            output.push_str("] roots[");
            for (index, root) in live_frame.stack_map.root_slots().iter().enumerate() {
                if index > 0 {
                    output.push(',');
                }
                let _ = write!(output, "{}", root.raw());
            }
            output.push(']');
        }
        MirTerminator::Return { values } => dump_values(output, "return", values),
        MirTerminator::Trap(trap) => {
            let _ = write!(output, "trap {}", trap_kind_text(trap.kind()));
        }
        MirTerminator::Panic(payload) => {
            output.push_str("panic ");
            dump_panic_payload(output, payload);
        }
        MirTerminator::ContinueUnwind(reason) => {
            output.push_str("resumeUnwind ");
            dump_unwind_reason(output, reason);
        }
        MirTerminator::ResumeUnwind => output.push_str("resumeCurrentUnwind"),
        MirTerminator::Unreachable => output.push_str("unreachable"),
    }
}

fn dump_binary(output: &mut String, name: &str, left: ValueId, right: ValueId) {
    let _ = write!(output, "{name} v{} v{}", left.raw(), right.raw());
}

fn dump_unary(output: &mut String, name: &str, operand: ValueId) {
    let _ = write!(output, "{name} v{}", operand.raw());
}

fn dump_numeric_binary(output: &mut String, name: &str, kind: &str, left: ValueId, right: ValueId) {
    let _ = write!(output, "{name} {kind} v{} v{}", left.raw(), right.raw());
}

fn dump_numeric_unary(output: &mut String, name: &str, kind: &str, operand: ValueId) {
    let _ = write!(output, "{name} {kind} v{}", operand.raw());
}

pub(crate) const fn integer_kind_text(kind: IntegerKind) -> &'static str {
    match kind {
        IntegerKind::Int8 => "Int8",
        IntegerKind::Int16 => "Int16",
        IntegerKind::Int32 => "Int32",
        IntegerKind::Int64 => "Int64",
        IntegerKind::UInt8 => "UInt8",
        IntegerKind::UInt16 => "UInt16",
        IntegerKind::UInt32 => "UInt32",
        IntegerKind::UInt64 => "UInt64",
    }
}

pub(crate) const fn float_kind_text(kind: FloatKind) -> &'static str {
    match kind {
        FloatKind::Float32 => "Float32",
        FloatKind::Float64 => "Float64",
    }
}

fn dump_values(output: &mut String, name: &str, values: &[ValueId]) {
    output.push_str(name);
    output.push(' ');
    dump_value_list(output, values);
}

fn dump_value_list(output: &mut String, values: &[ValueId]) {
    output.push('(');
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "v{}", value.raw());
    }
    output.push(')');
}

fn dump_fields(
    output: &mut String,
    name: &str,
    record: SymbolId,
    base: Option<ValueId>,
    fields: &[(FieldId, ValueId)],
) {
    let _ = write!(output, "{name} s{}", record.raw());
    if let Some(base) = base {
        let _ = write!(output, " v{}", base.raw());
    }
    output.push_str(" {");
    for (index, (field, value)) in fields.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "field#{}=v{}", field.raw(), value.raw());
    }
    output.push('}');
}

fn dump_field_values(output: &mut String, fields: &[(FieldId, ValueId)]) {
    output.push('{');
    for (index, (field, value)) in fields.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "field#{}=v{}", field.raw(), value.raw());
    }
    output.push('}');
}

fn dump_table_entries(output: &mut String, entries: &[(ValueId, ValueId)]) {
    output.push('(');
    for (index, (key, value)) in entries.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "v{} => v{}", key.raw(), value.raw());
    }
    output.push(')');
}

fn dump_effects(output: &mut String, effects: MirEffectSummary) {
    for (index, effect) in effects.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        output.push_str(match effect {
            MirEffect::Allocates => "Allocates",
            MirEffect::WritesManagedReference => "WritesManagedReference",
            MirEffect::MayTrap => "MayTrap",
            MirEffect::MayUnwind => "MayUnwind",
            MirEffect::Suspends => "Suspends",
            MirEffect::Blocks => "Blocks",
            MirEffect::UnsafeMemory => "UnsafeMemory",
            MirEffect::ForeignFunction => "ForeignFunction",
            MirEffect::AmbientIo => "AmbientIo",
            MirEffect::CompilerQuery => "CompilerQuery",
            MirEffect::GcSafePoint => "GcSafePoint",
            MirEffect::Roots => "Roots",
        });
    }
}

fn dump_call_contract(output: &mut String, effects: MirEffectSummary, unwind: MirUnwindAction) {
    output.push_str(" effects[");
    dump_effects(output, effects);
    output.push_str("] unwind ");
    match unwind {
        MirUnwindAction::Propagate => output.push_str("propagate"),
        MirUnwindAction::Cleanup(block) => {
            let _ = write!(output, "cleanup:b{}", block.raw());
        }
    }
}

fn dump_object_map(output: &mut String, map: &ObjectMap) {
    let _ = write!(output, "map[{}:", map.slot_count());
    for (index, slot) in map.reference_slots().iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        let _ = write!(output, "{}", slot.raw());
    }
    output.push(']');
}

fn dump_optional_value(output: &mut String, value: Option<ValueId>) {
    if let Some(value) = value {
        let _ = write!(output, "v{}", value.raw());
    } else {
        output.push_str("nil");
    }
}

const fn trap_kind_text(kind: pop_runtime_interface::TrapKind) -> &'static str {
    match kind {
        pop_runtime_interface::TrapKind::IntegerOverflow => "IntegerOverflow",
        pop_runtime_interface::TrapKind::DivisionByZero => "DivisionByZero",
        pop_runtime_interface::TrapKind::NumericConversion => "NumericConversion",
        pop_runtime_interface::TrapKind::InvalidRangeStep => "InvalidRangeStep",
        pop_runtime_interface::TrapKind::BoundsViolation => "BoundsViolation",
        pop_runtime_interface::TrapKind::ConcurrentModification => "ConcurrentModification",
        pop_runtime_interface::TrapKind::ImpossibleState => "ImpossibleState",
    }
}

fn dump_panic_payload(output: &mut String, payload: &PanicPayload) {
    match payload.kind() {
        pop_runtime_interface::PanicKind::RuntimeInvariant => output.push_str("RuntimeInvariant"),
        pop_runtime_interface::PanicKind::DoublePanic => output.push_str("DoublePanic"),
        pop_runtime_interface::PanicKind::OutOfMemory {
            requested_objects,
            requested_slots,
        } => {
            let _ = write!(output, "OutOfMemory({requested_objects},{requested_slots})");
        }
    }
}

fn dump_unwind_reason(output: &mut String, reason: &UnwindReason) {
    match reason {
        UnwindReason::Panic(payload) => dump_panic_payload(output, payload),
        UnwindReason::Cancellation => output.push_str("Cancellation"),
    }
}
