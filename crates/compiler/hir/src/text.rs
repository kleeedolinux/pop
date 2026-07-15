//! Deterministic HIR text rendering used by diagnostics and regression tests.
//!
//! Rendering is deliberately separate from HIR construction and verification:
//! textual dumps are disposable tooling output, never semantic compiler input.

use std::fmt::Write;

use pop_foundation::{ClassId, SymbolId, TypeId, UnionCaseId};
use pop_resolve::Visibility;
use pop_types::{
    AttributeConstant, FloatValue, NumericConversionKind, TypeArena, TypedBinaryOperator,
    TypedUnaryOperator,
};

use crate::ir::*;

pub(crate) fn dump_declaration(
    output: &mut String,
    declaration: &HirDeclaration,
    arena: &TypeArena,
) {
    let _ = write!(
        output,
        "declaration s{} {} m{} b{} ",
        declaration.symbol.raw(),
        visibility_text(declaration.visibility),
        declaration.module.raw(),
        declaration.bubble.raw()
    );
    match &declaration.kind {
        HirDeclarationKind::Record(record) => {
            let _ = write!(
                output,
                "record {}:{}",
                declaration.name,
                type_text(record.type_id, arena)
            );
        }
        HirDeclarationKind::Union(union) => {
            let _ = write!(
                output,
                "union {}:{}",
                declaration.name,
                type_text(union.type_id, arena)
            );
        }
        HirDeclarationKind::Error(error) => {
            let _ = write!(
                output,
                "error {} e{}:{}",
                declaration.name,
                error.error.raw(),
                type_text(error.type_id, arena)
            );
            for case in &error.cases {
                let _ = write!(output, " [errorCase#{} {}(", case.case.raw(), case.name);
                for (index, parameter) in case.parameters.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    let _ = write!(
                        output,
                        "{}:{}",
                        parameter.name,
                        type_text(parameter.type_id, arena)
                    );
                }
                output.push_str(")] ");
            }
        }
        HirDeclarationKind::Enum(enumeration) => {
            let _ = write!(
                output,
                "enum {}:{}",
                declaration.name,
                type_text(enumeration.type_id, arena)
            );
            for case in &enumeration.cases {
                let _ = write!(
                    output,
                    " [ec{} {}={}]",
                    case.case.raw(),
                    case.name,
                    case.discriminant
                );
            }
        }
        HirDeclarationKind::Class(class) => {
            let _ = write!(
                output,
                "class {} c{}:{} {}",
                declaration.name,
                class.class.raw(),
                type_text(class.type_id, arena),
                if class.is_open { "open" } else { "sealed" }
            );
            for implementation in &class.interfaces {
                let _ = write!(
                    output,
                    " implements i{}:{}",
                    implementation.interface.raw(),
                    type_text(implementation.interface_type, arena)
                );
                for mapping in &implementation.methods {
                    let _ = write!(
                        output,
                        " [im{} slot{} => m{}]",
                        mapping.interface_method.raw(),
                        mapping.slot,
                        mapping.class_method.raw()
                    );
                }
            }
            for implementation in &class.builtin_interfaces {
                let _ = write!(
                    output,
                    " implements b{}:{}",
                    implementation.interface.raw(),
                    type_text(implementation.interface_type, arena)
                );
                for mapping in &implementation.methods {
                    let _ = write!(
                        output,
                        " [iterationMethod#{} => m{}]",
                        mapping.protocol_method.raw(),
                        mapping.class_method.raw()
                    );
                }
            }
        }
        HirDeclarationKind::Interface(interface) => {
            let _ = write!(
                output,
                "interface {} i{}:{}",
                declaration.name,
                interface.interface.raw(),
                type_text(interface.type_id, arena)
            );
            for method in &interface.methods {
                let _ = write!(
                    output,
                    " [im{} slot{} {}(",
                    method.method.raw(),
                    method.slot,
                    method.name
                );
                for (index, parameter) in method.parameters.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&type_text(parameter.type_id, arena));
                }
                output.push_str(") -> (");
                for (index, result) in method.results.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&type_text(*result, arena));
                }
                output.push_str(")]");
            }
        }
        HirDeclarationKind::Attribute(attribute) => {
            let _ = write!(
                output,
                "attribute {} a{}",
                declaration.name,
                attribute.attribute.raw()
            );
        }
    }
    output.push('\n');
}

pub(crate) fn dump_function(output: &mut String, function: &HirFunction, arena: &TypeArena) {
    for attribute in &function.attributes {
        let _ = write!(
            output,
            "attribute a{} s{}(",
            attribute.attribute.raw(),
            attribute.definition.raw()
        );
        for (index, argument) in attribute.arguments.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            let _ = write!(
                output,
                "{}:{}=",
                argument.name,
                type_text(argument.value_type, arena)
            );
            dump_attribute_value(output, &argument.value);
        }
        output.push_str(")\n");
    }
    let _ = write!(
        output,
        "function s{} f{} {} m{} b{} {}(",
        function.symbol.raw(),
        function.function.raw(),
        visibility_text(function.visibility),
        function.module.raw(),
        function.bubble.raw(),
        function.name
    );
    for (index, parameter) in function.parameters.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(
            output,
            "p{}:{}:{}",
            parameter.parameter.raw(),
            parameter.name,
            type_text(parameter.type_id, arena)
        );
    }
    output.push_str(") -> (");
    for (index, result) in function.results.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push_str(&type_text(*result, arena));
    }
    output.push_str(")\n");
    dump_statements(output, &function.body, arena, 1);
}

pub(crate) fn dump_method(output: &mut String, method: &HirMethod, arena: &TypeArena) {
    let _ = writeln!(
        output,
        "method m{} class c{} definition s{}",
        method.method.raw(),
        method.class.raw(),
        method.definition.raw()
    );
    dump_function(output, &method.function, arena);
}

fn dump_attribute_value(output: &mut String, value: &AttributeConstant) {
    match value {
        AttributeConstant::Nil => output.push_str("nil"),
        AttributeConstant::Boolean(value) => {
            output.push_str(if *value { "true" } else { "false" });
        }
        AttributeConstant::Integer(value) => {
            let _ = write!(output, "{value}");
        }
        AttributeConstant::Float(value) => dump_float_value(output, *value),
        AttributeConstant::String(value) => {
            output.push('"');
            output.push_str(value);
            output.push('"');
        }
        AttributeConstant::Tuple(values) => {
            output.push('(');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                dump_attribute_value(output, value);
            }
            output.push(')');
        }
    }
}

#[allow(clippy::too_many_lines)]
fn dump_statements(
    output: &mut String,
    statements: &[HirStatement],
    arena: &TypeArena,
    depth: usize,
) {
    for statement in statements {
        let indentation = "  ".repeat(depth);
        output.push_str(&indentation);
        match statement.kind() {
            HirStatementKind::Local {
                binding,
                local,
                name,
                local_type,
                initializer,
            } => {
                let _ = write!(
                    output,
                    "local bind#{} l{} {}:{} = ",
                    binding.raw(),
                    local.raw(),
                    name,
                    type_text(*local_type, arena)
                );
                dump_expression(output, initializer, arena);
                output.push('\n');
            }
            HirStatementKind::MultipleLocal { bindings, value } => {
                output.push_str("multipleLocal ");
                for (index, binding) in bindings.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    let _ = write!(
                        output,
                        "bind#{} l{} {}:{}",
                        binding.binding.raw(),
                        binding.local.raw(),
                        binding.name,
                        type_text(binding.local_type, arena)
                    );
                }
                output.push_str(" = ");
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::LocalSet { local, value } => {
                let _ = write!(output, "local.set l{} = ", local.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::ParameterSet { parameter, value } => {
                let _ = write!(output, "parameter.set p{} = ", parameter.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::CaptureSet { capture, value } => {
                let _ = write!(output, "capture.set cap{} = ", capture.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::MultipleAssignment { targets, value } => {
                output.push_str("multipleAssignment ");
                for (index, target) in targets.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    dump_assignment_target(output, target, arena);
                }
                output.push_str(" = ");
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::Return { values } => {
                output.push_str("return");
                for value in values {
                    output.push(' ');
                    dump_expression(output, value, arena);
                }
                output.push('\n');
            }
            HirStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                output.push_str("if ");
                dump_expression(output, condition, arena);
                output.push('\n');
                dump_statements(output, then_body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("else\n");
                dump_statements(output, else_body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::OptionalIf {
                binding,
                local,
                name,
                inner_type,
                initializer,
                then_body,
                else_body,
            } => {
                let _ = write!(
                    output,
                    "optionalIf bind#{} l{} {}:{} = ",
                    binding.raw(),
                    local.raw(),
                    name,
                    type_text(*inner_type, arena)
                );
                dump_expression(output, initializer, arena);
                output.push('\n');
                dump_statements(output, then_body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("else\n");
                dump_statements(output, else_body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::While { condition, body } => {
                output.push_str("while ");
                dump_expression(output, condition, arena);
                output.push('\n');
                dump_statements(output, body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::OptionalWhile {
                binding,
                local,
                name,
                inner_type,
                initializer,
                body,
            } => {
                let _ = write!(
                    output,
                    "optionalWhile bind#{} l{} {}:{} = ",
                    binding.raw(),
                    local.raw(),
                    name,
                    type_text(*inner_type, arena)
                );
                dump_expression(output, initializer, arena);
                output.push('\n');
                dump_statements(output, body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::RepeatUntil { body, condition } => {
                output.push_str("repeat\n");
                dump_statements(output, body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("until ");
                dump_expression(output, condition, arena);
                output.push('\n');
            }
            HirStatementKind::NumericFor {
                binding,
                local,
                name,
                integer_type,
                first,
                last,
                step,
                body,
            } => {
                let _ = write!(
                    output,
                    "numericFor bind#{} l{} {}:{} = ",
                    binding.raw(),
                    local.raw(),
                    name,
                    type_text(*integer_type, arena)
                );
                dump_expression(output, first, arena);
                output.push_str(", ");
                dump_expression(output, last, arena);
                output.push_str(", ");
                dump_expression(output, step, arena);
                output.push('\n');
                dump_statements(output, body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::GeneralizedFor {
                protocol,
                source,
                item_type,
                iterator_type,
                iteration_type,
                bindings,
                iterable,
                body,
            } => {
                let _ = write!(
                    output,
                    "generalizedFor {:?} item:{} iterator:{} step:{} protocol(iteration#{}, iterable#{}, iterator#{}, list#{}, range#{}, cases {}:{}, methods {}:{}) ",
                    source,
                    type_text(*item_type, arena),
                    type_text(*iterator_type, arena),
                    type_text(*iteration_type, arena),
                    protocol.iteration().raw(),
                    protocol.iterable().raw(),
                    protocol.iterator().raw(),
                    protocol.list().raw(),
                    protocol.range().raw(),
                    protocol.item_case().raw(),
                    protocol.end_case().raw(),
                    protocol.iterator_method().raw(),
                    protocol.next_method().raw(),
                );
                for (index, binding) in bindings.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    let _ = write!(
                        output,
                        "bind#{} l{} {}:{}",
                        binding.binding.raw(),
                        binding.local.raw(),
                        binding.name,
                        type_text(binding.local_type, arena)
                    );
                }
                output.push_str(" in ");
                dump_expression(output, iterable, arena);
                output.push('\n');
                dump_statements(output, body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::Break => output.push_str("break\n"),
            HirStatementKind::Continue => output.push_str("continue\n"),
            HirStatementKind::Match {
                scrutinee,
                union,
                arms,
            } => {
                let _ = write!(output, "match s{} ", union.raw());
                dump_expression(output, scrutinee, arena);
                output.push('\n');
                for arm in arms {
                    output.push_str(&indentation);
                    let _ = write!(output, "when case#{}(", arm.case.raw());
                    for (index, binding) in arm.bindings.iter().enumerate() {
                        if index != 0 {
                            output.push_str(", ");
                        }
                        if binding.is_ignored() {
                            let _ = write!(output, "_:{}", type_text(binding.type_id, arena));
                        } else if let (Some(binding_id), Some(local)) =
                            (binding.binding, binding.local)
                        {
                            let _ = write!(
                                output,
                                "bind#{} l{} {}:{}",
                                binding_id.raw(),
                                local.raw(),
                                binding.name,
                                type_text(binding.type_id, arena)
                            );
                        }
                    }
                    output.push_str(")\n");
                    dump_statements(output, &arm.body, arena, depth + 1);
                }
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::ErrorMatch {
                scrutinee,
                error,
                arms,
            } => {
                let _ = write!(output, "errorMatch e{} ", error.raw());
                dump_expression(output, scrutinee, arena);
                output.push('\n');
                for arm in arms {
                    output.push_str(&indentation);
                    let _ = write!(output, "when errorCase#{}\n", arm.case.raw());
                    dump_statements(output, &arm.body, arena, depth + 1);
                }
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::ResultMatch {
                scrutinee,
                result,
                result_type,
                arms,
            } => {
                let _ = write!(
                    output,
                    "resultMatch builtin#{}:{} ",
                    result.raw(),
                    type_text(*result_type, arena)
                );
                dump_expression(output, scrutinee, arena);
                output.push('\n');
                for arm in arms {
                    output.push_str(&indentation);
                    let _ = write!(output, "when resultCase#{}\n", arm.case.raw());
                    dump_statements(output, &arm.body, arena, depth + 1);
                }
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::Defer { body } => {
                output.push_str("defer\n");
                dump_statements(output, body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::FieldSet { base, field, value } => {
                output.push_str("field.set ");
                dump_expression(output, base, arena);
                let _ = write!(output, ".field#{} = ", field.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::CompoundFieldSet {
                base,
                field,
                operator,
                value,
                ..
            } => {
                let _ = write!(output, "compound.fieldSet {operator:?} ");
                dump_expression(output, base, arena);
                let _ = write!(output, ".field#{} = ", field.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::ArraySet {
                array,
                index,
                value,
            } => {
                output.push_str("array.set ");
                dump_expression(output, array, arena);
                output.push('[');
                dump_expression(output, index, arena);
                output.push_str("] = ");
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::ListSet { list, index, value } => {
                output.push_str("list.set ");
                dump_expression(output, list, arena);
                output.push('[');
                dump_expression(output, index, arena);
                output.push_str("] = ");
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::TableSet { table, key, value } => {
                output.push_str("table.set ");
                dump_expression(output, table, arena);
                output.push('[');
                dump_expression(output, key, arena);
                output.push_str("] = ");
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::CompoundArraySet {
                array,
                index,
                operator,
                value,
                ..
            } => {
                let _ = write!(output, "compound.arraySet {operator:?} ");
                dump_expression(output, array, arena);
                output.push('[');
                dump_expression(output, index, arena);
                output.push_str("] = ");
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::Call(call) => {
                output.push_str("do ");
                dump_call(
                    output,
                    call.dispatch(),
                    call.type_arguments(),
                    call.arguments(),
                    arena,
                );
                output.push('\n');
            }
            HirStatementKind::Expression(expression) => {
                dump_expression(output, expression, arena);
                output.push('\n');
            }
        }
    }
}

fn dump_assignment_target(output: &mut String, target: &HirAssignmentTarget, arena: &TypeArena) {
    match target {
        HirAssignmentTarget::Local { local, .. } => {
            let _ = write!(output, "l{}", local.raw());
        }
        HirAssignmentTarget::Capture { capture, .. } => {
            let _ = write!(output, "cap{}", capture.raw());
        }
        HirAssignmentTarget::Field { base, field, .. } => {
            dump_expression(output, base, arena);
            let _ = write!(output, ".field#{}", field.raw());
        }
        HirAssignmentTarget::Array { array, index, .. } => {
            dump_array_get(output, array, index, arena);
        }
        HirAssignmentTarget::List { list, index, .. } => {
            dump_expression(output, list, arena);
            output.push('[');
            dump_expression(output, index, arena);
            output.push(']');
        }
        HirAssignmentTarget::Table { table, key, .. } => {
            dump_expression(output, table, arena);
            output.push('[');
            dump_expression(output, key, arena);
            output.push(']');
        }
    }
}

#[allow(clippy::too_many_lines)]
fn dump_expression(output: &mut String, expression: &HirExpression, arena: &TypeArena) {
    match expression.kind() {
        HirExpressionKind::Integer(value) => {
            let _ = write!(output, "{value}");
        }
        HirExpressionKind::Float(value) => dump_float_value(output, *value),
        HirExpressionKind::String(value) => {
            let _ = write!(output, "{value:?}");
        }
        HirExpressionKind::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
        HirExpressionKind::Nil => output.push_str("nil"),
        HirExpressionKind::Closure(closure) => {
            let _ = write!(output, "closure nested#{} [", closure.function.raw());
            for (index, capture) in closure.captures.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                let _ = write!(
                    output,
                    "capture.{} cap{} bind#{}=",
                    match capture.mode {
                        HirCaptureMode::Value => "value",
                        HirCaptureMode::Cell => "cell",
                    },
                    capture.capture.raw(),
                    capture.binding.raw()
                );
                match capture.source {
                    HirCaptureSource::Local(local) => {
                        let _ = write!(output, "l{}", local.raw());
                    }
                    HirCaptureSource::Parameter(parameter) => {
                        let _ = write!(output, "p{}", parameter.raw());
                    }
                    HirCaptureSource::Capture(source) => {
                        let _ = write!(output, "cap{}", source.raw());
                    }
                }
                let _ = write!(output, ":{}", type_text(capture.type_id, arena));
            }
            output.push_str("] (");
            for (index, parameter) in closure.parameters.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                let _ = write!(
                    output,
                    "bind#{} p{} {}:{}",
                    parameter.binding.raw(),
                    parameter.parameter.raw(),
                    parameter.name,
                    type_text(parameter.type_id, arena)
                );
            }
            output.push_str(") {\n");
            dump_statements(output, &closure.body, arena, 1);
            output.push('}');
        }
        HirExpressionKind::Local(local) => {
            let _ = write!(output, "l{}", local.raw());
        }
        HirExpressionKind::Parameter(parameter) => {
            let _ = write!(output, "p{}", parameter.raw());
        }
        HirExpressionKind::Capture(capture) => {
            let _ = write!(output, "cap{}", capture.raw());
        }
        HirExpressionKind::Function(function) => {
            let _ = write!(output, "function s{}", function.raw());
        }
        HirExpressionKind::Field { base, field } => {
            dump_expression(output, base, arena);
            let _ = write!(output, ".field#{}", field.raw());
        }
        HirExpressionKind::ArrayGet { array, index } => {
            dump_array_get(output, array, index, arena);
        }
        HirExpressionKind::TableGet { table, key } => {
            output.push_str("table.get ");
            dump_expression(output, table, arena);
            output.push(' ');
            dump_expression(output, key, arena);
        }
        HirExpressionKind::TupleGet { tuple, index } => {
            let _ = write!(output, "tuple.get {index} ");
            dump_expression(output, tuple, arena);
        }
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            output.push_str("array.create ");
            dump_expression(output, length, arena);
            output.push(' ');
            dump_expression(output, initial_value, arena);
        }
        HirExpressionKind::ArrayLength { array } => {
            output.push_str("array.length ");
            dump_expression(output, array, arena);
        }
        HirExpressionKind::ArrayGetChecked { array, index } => {
            output.push_str("array.get.checked ");
            dump_expression(output, array, arena);
            output.push(' ');
            dump_expression(output, index, arena);
        }
        HirExpressionKind::ArrayFill { array, value } => {
            output.push_str("array.fill ");
            dump_expression(output, array, arena);
            output.push(' ');
            dump_expression(output, value, arena);
        }
        HirExpressionKind::ListCreate { capacity } => {
            output.push_str("list.create");
            if let Some(capacity) = capacity {
                output.push(' ');
                dump_expression(output, capacity, arena);
            }
        }
        HirExpressionKind::ListLength { list } => {
            output.push_str("list.length ");
            dump_expression(output, list, arena);
        }
        HirExpressionKind::ListGet { list, index } => {
            output.push_str("list.get ");
            dump_expression(output, list, arena);
            output.push(' ');
            dump_expression(output, index, arena);
        }
        HirExpressionKind::ListGetChecked { list, index } => {
            output.push_str("list.get.checked ");
            dump_expression(output, list, arena);
            output.push(' ');
            dump_expression(output, index, arena);
        }
        HirExpressionKind::ListAdd { list, value } => {
            output.push_str("list.add ");
            dump_expression(output, list, arena);
            output.push(' ');
            dump_expression(output, value, arena);
        }
        HirExpressionKind::RangeCreate { first, last, step } => {
            output.push_str("range.create ");
            dump_expression(output, first, arena);
            output.push(' ');
            dump_expression(output, last, arena);
            output.push(' ');
            dump_expression(output, step, arena);
        }
        HirExpressionKind::Record { record, fields } => {
            let _ = write!(output, "record s{} ", record.raw());
            dump_fields(output, fields, arena);
        }
        HirExpressionKind::ClassConstruct {
            class,
            definition,
            fields,
        } => {
            dump_class(output, *class, *definition, fields, arena);
        }
        HirExpressionKind::RecordUpdate {
            record,
            base,
            fields,
        } => {
            let _ = write!(output, "record.update s{} ", record.raw());
            dump_expression(output, base, arena);
            output.push(' ');
            dump_fields(output, fields, arena);
        }
        HirExpressionKind::Array(elements) => {
            dump_array(output, elements, arena);
        }
        HirExpressionKind::Table(entries) => {
            dump_table(output, entries, arena);
        }
        HirExpressionKind::UnionCase {
            union,
            case,
            arguments,
        } => {
            dump_union_case(output, *union, *case, arguments, arena);
        }
        HirExpressionKind::ResultCase {
            result,
            case,
            arguments,
        } => {
            let _ = write!(
                output,
                "result.case builtin#{} case#{}(",
                result.raw(),
                case.raw()
            );
            dump_expression_list(output, arguments, arena);
            output.push(')');
        }
        HirExpressionKind::IterationCase {
            iteration,
            case,
            arguments,
        } => {
            let _ = write!(
                output,
                "iteration.case builtin#{} case#{}(",
                iteration.raw(),
                case.raw()
            );
            dump_expression_list(output, arguments, arena);
            output.push(')');
        }
        HirExpressionKind::ErrorCase {
            error,
            case,
            arguments,
        } => {
            let _ = write!(output, "error.case e{} case#{}(", error.raw(), case.raw());
            dump_expression_list(output, arguments, arena);
            output.push(')');
        }
        HirExpressionKind::EnumCase {
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
        HirExpressionKind::Tuple(elements) => {
            output.push('(');
            for (index, element) in elements.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                dump_expression(output, element, arena);
            }
            output.push(')');
        }
        HirExpressionKind::StringConcat { left, right } => {
            output.push_str("string.concat(");
            dump_expression(output, left, arena);
            output.push_str(", ");
            dump_expression(output, right, arena);
            output.push(')');
        }
        HirExpressionKind::StringFormat { kind, value } => {
            let _ = write!(output, "string.format {kind:?}(");
            dump_expression(output, value, arena);
            output.push(')');
        }
        HirExpressionKind::Unary { operator, operand } => {
            output.push_str(unary_text(*operator));
            output.push(' ');
            dump_expression(output, operand, arena);
        }
        HirExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            output.push('(');
            dump_expression(output, left, arena);
            output.push(' ');
            output.push_str(binary_text(*operator));
            output.push(' ');
            dump_expression(output, right, arena);
            output.push(')');
        }
        HirExpressionKind::OptionalDefault { optional, fallback } => {
            output.push_str("optional.default(");
            dump_expression(output, optional, arena);
            output.push_str(", ");
            dump_expression(output, fallback, arena);
            output.push(')');
        }
        HirExpressionKind::OptionalPropagate {
            optional,
            enclosing_result,
        } => {
            let _ = write!(
                output,
                "optional.propagate result:{}(",
                type_text(*enclosing_result, arena)
            );
            dump_expression(output, optional, arena);
            output.push(')');
        }
        HirExpressionKind::ResultPropagate {
            result,
            result_definition,
            enclosing_result,
            ..
        } => {
            let _ = write!(
                output,
                "result.propagate builtin#{} enclosing:{}(",
                result_definition.raw(),
                type_text(*enclosing_result, arena)
            );
            dump_expression(output, result, arena);
            output.push(')');
        }
        HirExpressionKind::OptionalNarrow { optional } => {
            output.push_str("optional.narrow(");
            dump_expression(output, optional, arena);
            output.push(')');
        }
        HirExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            output.push_str("conditional(");
            dump_expression(output, condition, arena);
            output.push_str(", ");
            dump_expression(output, when_true, arena);
            output.push_str(", ");
            dump_expression(output, when_false, arena);
            output.push(')');
        }
        HirExpressionKind::Await { task } => {
            output.push_str("await(");
            dump_expression(output, task, arena);
            output.push(')');
        }
        HirExpressionKind::Call {
            dispatch,
            type_arguments,
            arguments,
            ..
        } => {
            dump_call(output, dispatch, type_arguments, arguments, arena);
        }
        HirExpressionKind::InterfaceUpcast { value, interface } => {
            match interface {
                pop_foundation::NominalInterfaceId::User(interface) => {
                    let _ = write!(output, "convert.interface i{} ", interface.raw());
                }
                pop_foundation::NominalInterfaceId::Builtin(interface) => {
                    let _ = write!(output, "convert.interface b{} ", interface.raw());
                }
            }
            dump_expression(output, value, arena);
        }
        HirExpressionKind::NumericConvert { value, conversion } => {
            let _ = write!(output, "convert.{}(", conversion_text(*conversion));
            dump_expression(output, value, arena);
            output.push(')');
        }
    }
    let _ = write!(output, ":{}", type_text(expression.type_id(), arena));
}

fn dump_float_value(output: &mut String, value: FloatValue) {
    let _ = write!(
        output,
        "{}(0x{:x})",
        match value.kind() {
            pop_types::FloatKind::Float32 => "float32",
            pop_types::FloatKind::Float64 => "float64",
        },
        value.bits()
    );
}

fn dump_call(
    output: &mut String,
    dispatch: &HirCallDispatch,
    type_arguments: &[TypeId],
    arguments: &[HirExpression],
    arena: &TypeArena,
) {
    match dispatch {
        HirCallDispatch::Standard { function } => {
            let _ = write!(output, "call.standard sf{}", function.raw());
        }
        HirCallDispatch::Direct { function } => {
            let _ = write!(output, "call.direct s{}", function.raw());
        }
        HirCallDispatch::Referenced { function } => {
            let _ = write!(
                output,
                "call.reference b{}:s{}",
                function.bubble().raw(),
                function.symbol().raw()
            );
        }
        HirCallDispatch::DirectMethod { method } => {
            let _ = write!(output, "call.method m{}", method.raw());
        }
        HirCallDispatch::InterfaceMethod {
            interface,
            method,
            slot,
        } => {
            let _ = write!(
                output,
                "call.interface i{} im{} slot{}",
                interface.raw(),
                method.raw(),
                slot
            );
        }
        HirCallDispatch::BuiltinInterfaceMethod { interface, method } => {
            let _ = write!(
                output,
                "call.builtin-interface b{} iterationMethod#{}",
                interface.raw(),
                method.raw()
            );
        }
        HirCallDispatch::Indirect { callee } => {
            output.push_str("call.indirect ");
            dump_expression(output, callee, arena);
        }
    }
    if !type_arguments.is_empty() {
        output.push_str("<<");
        for (index, argument) in type_arguments.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            output.push_str(&type_text(*argument, arena));
        }
        output.push_str(">>");
    }
    output.push('(');
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, argument, arena);
    }
    output.push(')');
}

fn dump_class(
    output: &mut String,
    class: ClassId,
    definition: SymbolId,
    fields: &[HirFieldValue],
    arena: &TypeArena,
) {
    let _ = write!(output, "class c{} s{} ", class.raw(), definition.raw());
    dump_fields(output, fields, arena);
}

fn dump_union_case(
    output: &mut String,
    union: SymbolId,
    case: UnionCaseId,
    arguments: &[HirExpression],
    arena: &TypeArena,
) {
    let _ = write!(output, "union.case s{} case#{}(", union.raw(), case.raw());
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, argument, arena);
    }
    output.push(')');
}

fn dump_expression_list(output: &mut String, expressions: &[HirExpression], arena: &TypeArena) {
    for (index, expression) in expressions.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, expression, arena);
    }
}

fn dump_array_get(
    output: &mut String,
    array: &HirExpression,
    index: &HirExpression,
    arena: &TypeArena,
) {
    output.push_str("array.get ");
    dump_expression(output, array, arena);
    output.push('[');
    dump_expression(output, index, arena);
    output.push(']');
}

fn dump_array(output: &mut String, elements: &[HirExpression], arena: &TypeArena) {
    output.push_str("array[");
    for (index, element) in elements.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, element, arena);
    }
    output.push(']');
}

fn dump_table(output: &mut String, entries: &[HirTableEntry], arena: &TypeArena) {
    output.push_str("table{");
    for (index, entry) in entries.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, entry.key(), arena);
        output.push_str(" => ");
        dump_expression(output, entry.value(), arena);
    }
    output.push('}');
}

fn dump_fields(output: &mut String, fields: &[HirFieldValue], arena: &TypeArena) {
    output.push('{');
    for (index, field) in fields.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "field#{} = ", field.field().raw());
        dump_expression(output, field.value(), arena);
    }
    output.push('}');
}

fn type_text(type_id: TypeId, arena: &TypeArena) -> String {
    if arena.get(type_id).is_some() {
        format!("t{}", type_id.raw())
    } else {
        format!("invalid-t{}", type_id.raw())
    }
}

const fn visibility_text(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Internal => "internal",
        Visibility::Private => "private",
    }
}

const fn unary_text(operator: TypedUnaryOperator) -> &'static str {
    match operator {
        TypedUnaryOperator::Not => "not",
        TypedUnaryOperator::Negate => "-",
    }
}

const fn binary_text(operator: TypedBinaryOperator) -> &'static str {
    match operator {
        TypedBinaryOperator::Or => "or",
        TypedBinaryOperator::And => "and",
        TypedBinaryOperator::Equal => "==",
        TypedBinaryOperator::NotEqual => "~=",
        TypedBinaryOperator::LessThan => "<",
        TypedBinaryOperator::LessThanOrEqual => "<=",
        TypedBinaryOperator::GreaterThan => ">",
        TypedBinaryOperator::GreaterThanOrEqual => ">=",
        TypedBinaryOperator::Add => "+",
        TypedBinaryOperator::Subtract => "-",
        TypedBinaryOperator::Multiply => "*",
        TypedBinaryOperator::Divide => "/",
        TypedBinaryOperator::Remainder => "%",
    }
}

fn conversion_text(conversion: NumericConversionKind) -> String {
    match conversion {
        NumericConversionKind::IntegerToInteger { source, target } => {
            format!("integerToInteger.{source:?}.{target:?}")
        }
        NumericConversionKind::IntegerToFloat { source, target } => {
            format!("integerToFloat.{source:?}.{target:?}")
        }
        NumericConversionKind::FloatToInteger { source, target } => {
            format!("floatToInteger.{source:?}.{target:?}")
        }
        NumericConversionKind::FloatToFloat { source, target } => {
            format!("floatToFloat.{source:?}.{target:?}")
        }
    }
}
