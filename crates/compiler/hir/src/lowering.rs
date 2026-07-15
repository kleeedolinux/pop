//! Typed-body to HIR construction.
//!
//! This module owns the architecture boundary where resolved, fully typed
//! source bodies become backend-neutral HIR. It must not infer types, query
//! a backend, or admit compile-time-only handles into runtime HIR.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    BindingId, BubbleId, FunctionId, InterfaceId, InterfaceMethodId, MethodId, ModuleId,
    SourceSpan, SymbolId, ValueParameterId,
};
use pop_resolve::Visibility;
use pop_types::{
    CaptureMode, CaptureSource, ClassDefinition, ClassInterfaceImplementation,
    ClassMethodDefinition, ForeignFunctionDeclaration, InterfaceDefinition, ResolvedAttribute,
    ResolvedFunctionSignature, TypeArena, TypedAssignmentTarget, TypedBody, TypedCall,
    TypedCallDispatch, TypedCapture, TypedClosure, TypedExpression, TypedExpressionKind,
    TypedFieldValue, TypedIterationSource, TypedMatchArm, TypedMatchBinding, TypedStatement,
    TypedStatementKind, TypedTableEntry,
};

use crate::ir::*;
use crate::verification::{HirBuildError, HirVerificationError, verify_hir_callable};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HirFunctionContext {
    pub(crate) module: ModuleId,
    pub(crate) bubble: BubbleId,
    pub(crate) visibility: Visibility,
}

#[derive(Clone, Copy)]
pub struct HirKnownCallables<'a> {
    pub(crate) functions: &'a BTreeSet<SymbolId>,
    pub(crate) methods: &'a BTreeSet<MethodId>,
    pub(crate) interfaces: &'a [InterfaceDefinition],
}

impl<'a> HirKnownCallables<'a> {
    #[must_use]
    pub const fn new(functions: &'a BTreeSet<SymbolId>, methods: &'a BTreeSet<MethodId>) -> Self {
        Self {
            functions,
            methods,
            interfaces: &[],
        }
    }

    /// Adds nominal interface member schemas used to resolve per-interface
    /// dispatch slots while lowering typed calls.
    #[must_use]
    pub const fn with_interfaces(mut self, interfaces: &'a [InterfaceDefinition]) -> Self {
        self.interfaces = interfaces;
        self
    }
}

impl HirFunctionContext {
    #[must_use]
    pub const fn new(module: ModuleId, bubble: BubbleId, visibility: Visibility) -> Self {
        Self {
            module,
            bubble,
            visibility,
        }
    }
}

/// Constructs one bodyless backend-neutral HIR foreign declaration.
///
/// # Errors
///
/// Returns a deterministic error when the signature lacks canonical types or
/// disagrees with the typed foreign declaration identity.
pub fn build_hir_foreign_function(
    context: HirFunctionContext,
    signature: &ResolvedFunctionSignature,
    declaration: &ForeignFunctionDeclaration,
    attributes: &[ResolvedAttribute],
) -> Result<HirForeignFunction, Vec<HirBuildError>> {
    if signature.symbol() != declaration.symbol()
        || signature.is_async()
        || !signature.type_parameters().is_empty()
    {
        return Err(vec![HirVerificationError::InvalidGenericBounds {
            function: signature.symbol(),
            span: declaration.span(),
        }]);
    }
    let parameters: Option<Vec<_>> = signature
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            Some(HirParameter {
                binding: BindingId::from_raw(u32::try_from(index).ok()?),
                parameter: ValueParameterId::from_raw(u32::try_from(index).ok()?),
                name: parameter.name().to_owned(),
                type_id: parameter.parameter_type().type_id()?,
                span: parameter.span(),
            })
        })
        .collect();
    let results: Option<Vec<_>> = signature
        .results()
        .iter()
        .map(pop_types::ResolvedType::type_id)
        .collect();
    let Some((parameters, results)) = parameters.zip(results) else {
        return Err(vec![HirVerificationError::MissingCanonicalType]);
    };
    Ok(HirForeignFunction {
        function: FunctionId::from_raw(signature.symbol().raw()),
        symbol: signature.symbol(),
        module: context.module,
        bubble: context.bubble,
        visibility: context.visibility,
        name: signature.name().to_owned(),
        parameters,
        results,
        attributes: attributes.iter().map(lower_attribute).collect(),
        declaration: declaration.clone(),
    })
}

/// Constructs HIR from an accepted typed body, then verifies the result.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_function(
    module: ModuleId,
    bubble: BubbleId,
    visibility: Visibility,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
) -> Result<HirFunction, Vec<HirBuildError>> {
    build_hir_function_with_attributes(
        HirFunctionContext::new(module, bubble, visibility),
        signature,
        body,
        arena,
        known_functions,
        &[],
    )
}

/// Constructs and verifies HIR while retaining accepted compile-time attributes.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_function_with_attributes(
    context: HirFunctionContext,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    attributes: &[ResolvedAttribute],
) -> Result<HirFunction, Vec<HirBuildError>> {
    build_hir_function_with_methods_and_attributes(
        context,
        signature,
        body,
        arena,
        known_functions,
        &BTreeSet::new(),
        attributes,
    )
}

/// Constructs and verifies a function that may directly call known class methods.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_function_with_methods_and_attributes(
    context: HirFunctionContext,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    known_methods: &BTreeSet<MethodId>,
    attributes: &[ResolvedAttribute],
) -> Result<HirFunction, Vec<HirBuildError>> {
    build_hir_function_with_known_callables_and_attributes(
        context,
        signature,
        body,
        arena,
        HirKnownCallables::new(known_functions, known_methods),
        attributes,
    )
}

/// Constructs a function with complete direct, class, and nominal-interface
/// callable schemas. Interface schemas are required for interface calls because
/// `InterfaceMethodId` is global while dispatch slots are per interface.
///
/// # Errors
///
/// Returns deterministic build or verification failures. In particular,
/// compile-time-only attribute queries and interface calls without a known
/// canonical slot never enter runtime HIR.
pub fn build_hir_function_with_known_callables_and_attributes(
    context: HirFunctionContext,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known: HirKnownCallables<'_>,
    attributes: &[ResolvedAttribute],
) -> Result<HirFunction, Vec<HirBuildError>> {
    if let Some(span) = first_compile_time_only_statement(body.statements()) {
        return Err(vec![HirVerificationError::CompileTimeOnlyExpression {
            span,
        }]);
    }
    let interface_slots = collect_interface_slots(known.interfaces);
    if let Some((interface, method, span)) =
        first_unknown_interface_call(body.statements(), &interface_slots)
    {
        return Err(vec![HirVerificationError::UnknownInterfaceMethod {
            interface,
            method,
            span,
        }]);
    }
    let parameters: Option<Vec<_>> = signature
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            Some(HirParameter {
                binding: BindingId::from_raw(u32::try_from(index).ok()?),
                parameter: ValueParameterId::from_raw(u32::try_from(index).ok()?),
                name: parameter.name().to_owned(),
                type_id: parameter.parameter_type().type_id()?,
                span: parameter.span(),
            })
        })
        .collect();
    let results: Option<Vec<_>> = signature
        .results()
        .iter()
        .map(pop_types::ResolvedType::type_id)
        .collect();
    let Some((parameters, results)) = parameters.zip(results) else {
        return Err(vec![HirVerificationError::MissingCanonicalType]);
    };
    let function = HirFunction {
        function: FunctionId::from_raw(signature.symbol().raw()),
        symbol: signature.symbol(),
        module: context.module,
        bubble: context.bubble,
        visibility: context.visibility,
        name: signature.name().to_owned(),
        is_async: signature.is_async(),
        type_parameters: signature
            .type_parameters()
            .iter()
            .map(pop_types::ResolvedTypeParameter::type_id)
            .collect(),
        type_parameter_names: signature
            .type_parameters()
            .iter()
            .map(|parameter| parameter.name().to_owned())
            .collect(),
        type_parameter_bounds: signature
            .type_parameters()
            .iter()
            .map(pop_types::ResolvedTypeParameter::bound)
            .collect(),
        parameters,
        results,
        body: body
            .statements()
            .iter()
            .map(|statement| lower_statement(statement, &interface_slots))
            .collect(),
        attributes: attributes.iter().map(lower_attribute).collect(),
        effects: pop_types::EffectSummary::empty(),
    };
    verify_hir_callable(&function, arena, known.functions, known.methods)?;
    Ok(function)
}

/// Constructs one verified native method body while retaining its `MethodId`.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_method(
    context: HirFunctionContext,
    definition: &ClassDefinition,
    method: &ClassMethodDefinition,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known: HirKnownCallables<'_>,
) -> Result<HirMethod, Vec<HirBuildError>> {
    let function = build_hir_function_with_known_callables_and_attributes(
        context,
        signature,
        body,
        arena,
        known,
        &[],
    )?;
    Ok(HirMethod {
        method: method.method(),
        class: definition.class(),
        definition: definition.symbol(),
        function,
    })
}

fn lower_attribute(attribute: &ResolvedAttribute) -> HirAttribute {
    HirAttribute {
        attribute: attribute.attribute(),
        definition: attribute.definition(),
        arguments: attribute
            .arguments()
            .iter()
            .map(|argument| HirAttributeArgument {
                name: argument.name().to_owned(),
                value: argument.value().clone(),
                value_type: argument.value_type(),
                origin: argument.origin(),
            })
            .collect(),
        span: attribute.span(),
    }
}

type HirInterfaceSlotMap = BTreeMap<(InterfaceId, InterfaceMethodId), u32>;

fn lower_statement(
    statement: &TypedStatement,
    interface_slots: &HirInterfaceSlotMap,
) -> HirStatement {
    let kind = match statement.kind() {
        TypedStatementKind::Local {
            binding,
            local,
            name,
            local_type,
            initializer,
        } => HirStatementKind::Local {
            binding: *binding,
            local: *local,
            name: name.clone(),
            local_type: *local_type,
            initializer: lower_expression(initializer, interface_slots),
        },
        TypedStatementKind::MultipleLocal { bindings, value } => HirStatementKind::MultipleLocal {
            bindings: bindings
                .iter()
                .map(|binding| HirLocalBinding {
                    binding: binding.binding(),
                    local: binding.local(),
                    name: binding.name().to_owned(),
                    local_type: binding.local_type(),
                    span: binding.span(),
                })
                .collect(),
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::LocalSet { local, value } => HirStatementKind::LocalSet {
            local: *local,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::ParameterSet { parameter, value } => HirStatementKind::ParameterSet {
            parameter: *parameter,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::CaptureSet { capture, value } => HirStatementKind::CaptureSet {
            capture: *capture,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::Return { values } => HirStatementKind::Return {
            values: values
                .iter()
                .map(|value| lower_expression(value, interface_slots))
                .collect(),
        },
        TypedStatementKind::If {
            condition,
            then_body,
            else_body,
        } => HirStatementKind::If {
            condition: lower_expression(condition, interface_slots),
            then_body: then_body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
            else_body: else_body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::OptionalIf {
            binding,
            local,
            name,
            inner_type,
            initializer,
            then_body,
            else_body,
        } => HirStatementKind::OptionalIf {
            binding: *binding,
            local: *local,
            name: name.clone(),
            inner_type: *inner_type,
            initializer: lower_expression(initializer, interface_slots),
            then_body: then_body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
            else_body: else_body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::While { condition, body } => HirStatementKind::While {
            condition: lower_expression(condition, interface_slots),
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::OptionalWhile {
            binding,
            local,
            name,
            inner_type,
            initializer,
            body,
        } => HirStatementKind::OptionalWhile {
            binding: *binding,
            local: *local,
            name: name.clone(),
            inner_type: *inner_type,
            initializer: lower_expression(initializer, interface_slots),
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::RepeatUntil { body, condition } => HirStatementKind::RepeatUntil {
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
            condition: lower_expression(condition, interface_slots),
        },
        TypedStatementKind::NumericFor {
            binding,
            local,
            name,
            integer_type,
            first,
            last,
            step,
            body,
        } => HirStatementKind::NumericFor {
            binding: *binding,
            local: *local,
            name: name.clone(),
            integer_type: *integer_type,
            first: lower_expression(first, interface_slots),
            last: lower_expression(last, interface_slots),
            step: lower_expression(step, interface_slots),
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::GeneralizedFor {
            protocol,
            source,
            item_type,
            iterator_type,
            iteration_type,
            bindings,
            iterable,
            body,
        } => HirStatementKind::GeneralizedFor {
            protocol: HirIterationProtocol {
                iteration: protocol.iteration(),
                iterable: protocol.iterable(),
                iterator: protocol.iterator(),
                list: protocol.list(),
                range: protocol.range(),
                item_case: protocol.item_case(),
                end_case: protocol.end_case(),
                iterator_method: protocol.iterator_method(),
                next_method: protocol.next_method(),
            },
            source: match source {
                TypedIterationSource::Array => HirIterationSource::Array,
                TypedIterationSource::List => HirIterationSource::List,
                TypedIterationSource::Range => HirIterationSource::Range,
                TypedIterationSource::Table => HirIterationSource::Table,
                TypedIterationSource::Iterable => HirIterationSource::Iterable,
                TypedIterationSource::Iterator => HirIterationSource::Iterator,
                TypedIterationSource::BoundIterable => HirIterationSource::BoundIterable,
                TypedIterationSource::BoundIterator => HirIterationSource::BoundIterator,
                TypedIterationSource::ClassIterable { iterator_method } => {
                    HirIterationSource::ClassIterable {
                        iterator_method: *iterator_method,
                    }
                }
                TypedIterationSource::ClassIterator {
                    iterator_method,
                    next_method,
                } => HirIterationSource::ClassIterator {
                    iterator_method: *iterator_method,
                    next_method: *next_method,
                },
            },
            item_type: *item_type,
            iterator_type: *iterator_type,
            iteration_type: *iteration_type,
            bindings: bindings
                .iter()
                .map(|binding| HirLocalBinding {
                    binding: binding.binding(),
                    local: binding.local(),
                    name: binding.name().to_owned(),
                    local_type: binding.local_type(),
                    span: binding.span(),
                })
                .collect(),
            iterable: lower_expression(iterable, interface_slots),
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::Break => HirStatementKind::Break,
        TypedStatementKind::Continue => HirStatementKind::Continue,
        TypedStatementKind::Match {
            scrutinee,
            union,
            arms,
        } => HirStatementKind::Match {
            scrutinee: lower_expression(scrutinee, interface_slots),
            union: *union,
            arms: arms
                .iter()
                .map(|arm| lower_match_arm(arm, interface_slots))
                .collect(),
        },
        TypedStatementKind::ErrorMatch {
            scrutinee,
            error,
            arms,
        } => HirStatementKind::ErrorMatch {
            scrutinee: lower_expression(scrutinee, interface_slots),
            error: *error,
            arms: arms
                .iter()
                .map(|arm| HirErrorMatchArm {
                    error: arm.error(),
                    case: arm.case(),
                    bindings: arm.bindings().iter().map(lower_match_binding).collect(),
                    body: arm
                        .body()
                        .iter()
                        .map(|statement| lower_statement(statement, interface_slots))
                        .collect(),
                    span: arm.span(),
                })
                .collect(),
        },
        TypedStatementKind::ResultMatch {
            scrutinee,
            result,
            result_type,
            arms,
        } => HirStatementKind::ResultMatch {
            scrutinee: lower_expression(scrutinee, interface_slots),
            result: *result,
            result_type: *result_type,
            arms: arms
                .iter()
                .map(|arm| HirResultMatchArm {
                    case: arm.case(),
                    bindings: arm.bindings().iter().map(lower_match_binding).collect(),
                    body: arm
                        .body()
                        .iter()
                        .map(|statement| lower_statement(statement, interface_slots))
                        .collect(),
                    span: arm.span(),
                })
                .collect(),
        },
        TypedStatementKind::Defer { body } => HirStatementKind::Defer {
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::AsyncDefer { body } => HirStatementKind::AsyncDefer {
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::FieldSet { base, field, value } => HirStatementKind::FieldSet {
            base: lower_expression(base, interface_slots),
            field: *field,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::CompoundFieldSet {
            base,
            field,
            value_type,
            operator,
            value,
        } => HirStatementKind::CompoundFieldSet {
            base: lower_expression(base, interface_slots),
            field: *field,
            value_type: *value_type,
            operator: *operator,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::ArraySet {
            array,
            index,
            value,
        } => HirStatementKind::ArraySet {
            array: lower_expression(array, interface_slots),
            index: lower_expression(index, interface_slots),
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::ListSet { list, index, value } => HirStatementKind::ListSet {
            list: lower_expression(list, interface_slots),
            index: lower_expression(index, interface_slots),
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::TableSet { table, key, value } => HirStatementKind::TableSet {
            table: lower_expression(table, interface_slots),
            key: lower_expression(key, interface_slots),
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::CompoundArraySet {
            array,
            index,
            element_type,
            operator,
            value,
        } => HirStatementKind::CompoundArraySet {
            array: lower_expression(array, interface_slots),
            index: lower_expression(index, interface_slots),
            element_type: *element_type,
            operator: *operator,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::MultipleAssignment { targets, value } => {
            HirStatementKind::MultipleAssignment {
                targets: targets
                    .iter()
                    .map(|target| lower_assignment_target(target, interface_slots))
                    .collect(),
                value: lower_expression(value, interface_slots),
            }
        }
        TypedStatementKind::Call(call) => HirStatementKind::Call(lower_call(call, interface_slots)),
        TypedStatementKind::Expression(expression) => {
            HirStatementKind::Expression(lower_expression(expression, interface_slots))
        }
    };
    HirStatement {
        kind,
        span: statement.span(),
    }
}

fn lower_assignment_target(
    target: &TypedAssignmentTarget,
    interface_slots: &HirInterfaceSlotMap,
) -> HirAssignmentTarget {
    match target {
        TypedAssignmentTarget::Local {
            binding,
            local,
            value_type,
        } => HirAssignmentTarget::Local {
            binding: *binding,
            local: *local,
            value_type: *value_type,
        },
        TypedAssignmentTarget::Capture {
            binding,
            capture,
            value_type,
        } => HirAssignmentTarget::Capture {
            binding: *binding,
            capture: *capture,
            value_type: *value_type,
        },
        TypedAssignmentTarget::Field {
            base,
            field,
            value_type,
        } => HirAssignmentTarget::Field {
            base: lower_expression(base, interface_slots),
            field: *field,
            value_type: *value_type,
        },
        TypedAssignmentTarget::Array {
            array,
            index,
            element_type,
        } => HirAssignmentTarget::Array {
            array: lower_expression(array, interface_slots),
            index: lower_expression(index, interface_slots),
            element_type: *element_type,
        },
        TypedAssignmentTarget::List {
            list,
            index,
            element_type,
        } => HirAssignmentTarget::List {
            list: lower_expression(list, interface_slots),
            index: lower_expression(index, interface_slots),
            element_type: *element_type,
        },
        TypedAssignmentTarget::Table {
            table,
            key,
            value_type,
        } => HirAssignmentTarget::Table {
            table: lower_expression(table, interface_slots),
            key: lower_expression(key, interface_slots),
            value_type: *value_type,
        },
    }
}

fn lower_call(call: &TypedCall, interface_slots: &HirInterfaceSlotMap) -> HirCall {
    let dispatch = match call.dispatch() {
        TypedCallDispatch::Standard { function } => HirCallDispatch::Standard {
            function: *function,
        },
        TypedCallDispatch::Direct { function } => HirCallDispatch::Direct {
            function: *function,
        },
        TypedCallDispatch::Referenced { function } => HirCallDispatch::Referenced {
            function: *function,
        },
        TypedCallDispatch::DirectMethod { method, receiver } => {
            return HirCall {
                dispatch: HirCallDispatch::DirectMethod { method: *method },
                is_async: call.is_async(),
                type_arguments: call.type_arguments().to_vec(),
                arguments: receiver
                    .iter()
                    .map(|receiver| lower_expression(receiver, interface_slots))
                    .chain(
                        call.arguments()
                            .iter()
                            .map(|argument| lower_expression(argument, interface_slots)),
                    )
                    .collect(),
                span: call.span(),
            };
        }
        TypedCallDispatch::InterfaceMethod {
            interface,
            method,
            receiver,
        } => {
            return HirCall {
                dispatch: HirCallDispatch::InterfaceMethod {
                    interface: *interface,
                    method: *method,
                    slot: interface_slots[&(*interface, *method)],
                },
                is_async: call.is_async(),
                type_arguments: call.type_arguments().to_vec(),
                arguments: std::iter::once(lower_expression(receiver, interface_slots))
                    .chain(
                        call.arguments()
                            .iter()
                            .map(|argument| lower_expression(argument, interface_slots)),
                    )
                    .collect(),
                span: call.span(),
            };
        }
        TypedCallDispatch::BuiltinInterfaceMethod {
            interface,
            method,
            receiver,
        } => {
            return HirCall {
                dispatch: HirCallDispatch::BuiltinInterfaceMethod {
                    interface: *interface,
                    method: *method,
                },
                is_async: call.is_async(),
                type_arguments: call.type_arguments().to_vec(),
                arguments: std::iter::once(lower_expression(receiver, interface_slots))
                    .chain(
                        call.arguments()
                            .iter()
                            .map(|argument| lower_expression(argument, interface_slots)),
                    )
                    .collect(),
                span: call.span(),
            };
        }
        TypedCallDispatch::Indirect { callee } => HirCallDispatch::Indirect {
            callee: Box::new(lower_expression(callee, interface_slots)),
        },
    };
    HirCall {
        dispatch,
        is_async: call.is_async(),
        type_arguments: call.type_arguments().to_vec(),
        arguments: call
            .arguments()
            .iter()
            .map(|argument| lower_expression(argument, interface_slots))
            .collect(),
        span: call.span(),
    }
}

#[allow(clippy::too_many_lines)]
fn lower_expression(
    expression: &TypedExpression,
    interface_slots: &HirInterfaceSlotMap,
) -> HirExpression {
    let kind = match expression.kind() {
        TypedExpressionKind::Integer(value) => HirExpressionKind::Integer(*value),
        TypedExpressionKind::Float(value) => HirExpressionKind::Float(*value),
        TypedExpressionKind::String(value) => HirExpressionKind::String(value.clone()),
        TypedExpressionKind::Boolean(value) => HirExpressionKind::Boolean(*value),
        TypedExpressionKind::Nil => HirExpressionKind::Nil,
        TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. } => {
            unreachable!("compile-time-only attribute queries are rejected before runtime HIR")
        }
        TypedExpressionKind::Closure(closure) => {
            HirExpressionKind::Closure(lower_closure(closure, interface_slots))
        }
        TypedExpressionKind::Local(local) => HirExpressionKind::Local(*local),
        TypedExpressionKind::Parameter(parameter) => HirExpressionKind::Parameter(*parameter),
        TypedExpressionKind::Capture(capture) => HirExpressionKind::Capture(*capture),
        TypedExpressionKind::Function(function) => HirExpressionKind::Function(*function),
        TypedExpressionKind::Field { base, field } => HirExpressionKind::Field {
            base: Box::new(lower_expression(base, interface_slots)),
            field: *field,
        },
        TypedExpressionKind::ArrayGet { array, index } => HirExpressionKind::ArrayGet {
            array: Box::new(lower_expression(array, interface_slots)),
            index: Box::new(lower_expression(index, interface_slots)),
        },
        TypedExpressionKind::TableGet { table, key } => HirExpressionKind::TableGet {
            table: Box::new(lower_expression(table, interface_slots)),
            key: Box::new(lower_expression(key, interface_slots)),
        },
        TypedExpressionKind::TupleGet { tuple, index } => HirExpressionKind::TupleGet {
            tuple: Box::new(lower_expression(tuple, interface_slots)),
            index: *index,
        },
        TypedExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => HirExpressionKind::ArrayCreate {
            length: Box::new(lower_expression(length, interface_slots)),
            initial_value: Box::new(lower_expression(initial_value, interface_slots)),
        },
        TypedExpressionKind::ArrayLength { array } => HirExpressionKind::ArrayLength {
            array: Box::new(lower_expression(array, interface_slots)),
        },
        TypedExpressionKind::ArrayGetChecked { array, index } => {
            HirExpressionKind::ArrayGetChecked {
                array: Box::new(lower_expression(array, interface_slots)),
                index: Box::new(lower_expression(index, interface_slots)),
            }
        }
        TypedExpressionKind::ArrayFill { array, value } => HirExpressionKind::ArrayFill {
            array: Box::new(lower_expression(array, interface_slots)),
            value: Box::new(lower_expression(value, interface_slots)),
        },
        TypedExpressionKind::ListCreate { capacity } => HirExpressionKind::ListCreate {
            capacity: capacity
                .as_ref()
                .map(|capacity| Box::new(lower_expression(capacity, interface_slots))),
        },
        TypedExpressionKind::ListLength { list } => HirExpressionKind::ListLength {
            list: Box::new(lower_expression(list, interface_slots)),
        },
        TypedExpressionKind::ListGet { list, index } => HirExpressionKind::ListGet {
            list: Box::new(lower_expression(list, interface_slots)),
            index: Box::new(lower_expression(index, interface_slots)),
        },
        TypedExpressionKind::ListGetChecked { list, index } => HirExpressionKind::ListGetChecked {
            list: Box::new(lower_expression(list, interface_slots)),
            index: Box::new(lower_expression(index, interface_slots)),
        },
        TypedExpressionKind::ListAdd { list, value } => HirExpressionKind::ListAdd {
            list: Box::new(lower_expression(list, interface_slots)),
            value: Box::new(lower_expression(value, interface_slots)),
        },
        TypedExpressionKind::RangeCreate { first, last, step } => HirExpressionKind::RangeCreate {
            first: Box::new(lower_expression(first, interface_slots)),
            last: Box::new(lower_expression(last, interface_slots)),
            step: Box::new(lower_expression(step, interface_slots)),
        },
        TypedExpressionKind::Record { record, fields } => HirExpressionKind::Record {
            record: *record,
            fields: fields
                .iter()
                .map(|field| lower_field_value(field, interface_slots))
                .collect(),
        },
        TypedExpressionKind::ClassConstruct {
            class,
            definition,
            fields,
        } => HirExpressionKind::ClassConstruct {
            class: *class,
            definition: *definition,
            fields: fields
                .iter()
                .map(|field| lower_field_value(field, interface_slots))
                .collect(),
        },
        TypedExpressionKind::RecordUpdate {
            record,
            base,
            fields,
        } => HirExpressionKind::RecordUpdate {
            record: *record,
            base: Box::new(lower_expression(base, interface_slots)),
            fields: fields
                .iter()
                .map(|field| lower_field_value(field, interface_slots))
                .collect(),
        },
        TypedExpressionKind::Array(elements) => HirExpressionKind::Array(
            elements
                .iter()
                .map(|element| lower_expression(element, interface_slots))
                .collect(),
        ),
        TypedExpressionKind::Table(entries) => HirExpressionKind::Table(
            entries
                .iter()
                .map(|entry| lower_table_entry(entry, interface_slots))
                .collect(),
        ),
        TypedExpressionKind::UnionCase {
            union,
            case,
            arguments,
        } => HirExpressionKind::UnionCase {
            union: *union,
            case: *case,
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::ResultCase {
            result,
            case,
            arguments,
        } => HirExpressionKind::ResultCase {
            result: *result,
            case: *case,
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::IterationCase {
            iteration,
            case,
            arguments,
        } => HirExpressionKind::IterationCase {
            iteration: *iteration,
            case: *case,
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::ErrorCase {
            error,
            case,
            arguments,
        } => HirExpressionKind::ErrorCase {
            error: *error,
            case: *case,
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::EnumCase {
            definition,
            case,
            discriminant,
        } => HirExpressionKind::EnumCase {
            definition: *definition,
            case: *case,
            discriminant: *discriminant,
        },
        TypedExpressionKind::Tuple(elements) => HirExpressionKind::Tuple(
            elements
                .iter()
                .map(|element| lower_expression(element, interface_slots))
                .collect(),
        ),
        TypedExpressionKind::StringConcat { left, right } => HirExpressionKind::StringConcat {
            left: Box::new(lower_expression(left, interface_slots)),
            right: Box::new(lower_expression(right, interface_slots)),
        },
        TypedExpressionKind::StringFormat { kind, value } => HirExpressionKind::StringFormat {
            kind: *kind,
            value: Box::new(lower_expression(value, interface_slots)),
        },
        TypedExpressionKind::Unary { operator, operand } => HirExpressionKind::Unary {
            operator: *operator,
            operand: Box::new(lower_expression(operand, interface_slots)),
        },
        TypedExpressionKind::Binary {
            operator,
            left,
            right,
        } => HirExpressionKind::Binary {
            operator: *operator,
            left: Box::new(lower_expression(left, interface_slots)),
            right: Box::new(lower_expression(right, interface_slots)),
        },
        TypedExpressionKind::OptionalDefault { optional, fallback } => {
            HirExpressionKind::OptionalDefault {
                optional: Box::new(lower_expression(optional, interface_slots)),
                fallback: Box::new(lower_expression(fallback, interface_slots)),
            }
        }
        TypedExpressionKind::OptionalPropagate {
            optional,
            enclosing_result,
        } => HirExpressionKind::OptionalPropagate {
            optional: Box::new(lower_expression(optional, interface_slots)),
            enclosing_result: *enclosing_result,
        },
        TypedExpressionKind::ResultPropagate {
            result,
            result_definition,
            success_type,
            error_type,
            enclosing_result,
        } => HirExpressionKind::ResultPropagate {
            result: Box::new(lower_expression(result, interface_slots)),
            result_definition: *result_definition,
            success_type: *success_type,
            error_type: *error_type,
            enclosing_result: *enclosing_result,
        },
        TypedExpressionKind::OptionalNarrow { optional } => HirExpressionKind::OptionalNarrow {
            optional: Box::new(lower_expression(optional, interface_slots)),
        },
        TypedExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => HirExpressionKind::Conditional {
            condition: Box::new(lower_expression(condition, interface_slots)),
            when_true: Box::new(lower_expression(when_true, interface_slots)),
            when_false: Box::new(lower_expression(when_false, interface_slots)),
        },
        TypedExpressionKind::Await { task } => HirExpressionKind::Await {
            task: Box::new(lower_expression(task, interface_slots)),
        },
        TypedExpressionKind::TaskCancellationSource => HirExpressionKind::TaskCancellationSource,
        TypedExpressionKind::TaskCancelToken { source } => HirExpressionKind::TaskCancelToken {
            source: Box::new(lower_expression(source, interface_slots)),
        },
        TypedExpressionKind::TaskCancel { source } => HirExpressionKind::TaskCancel {
            source: Box::new(lower_expression(source, interface_slots)),
        },
        TypedExpressionKind::TaskGroup { cancel, body } => HirExpressionKind::TaskGroup {
            cancel: Box::new(lower_expression(cancel, interface_slots)),
            body: Box::new(lower_expression(body, interface_slots)),
        },
        TypedExpressionKind::TaskStart { group, task } => HirExpressionKind::TaskStart {
            group: Box::new(lower_expression(group, interface_slots)),
            task: Box::new(lower_expression(task, interface_slots)),
        },
        TypedExpressionKind::FfiHandleOpen { value } => HirExpressionKind::FfiHandleOpen {
            value: Box::new(lower_expression(value, interface_slots)),
        },
        TypedExpressionKind::FfiHandleGet { handle } => HirExpressionKind::FfiHandleGet {
            handle: Box::new(lower_expression(handle, interface_slots)),
        },
        TypedExpressionKind::FfiHandleClose { handle } => HirExpressionKind::FfiHandleClose {
            handle: Box::new(lower_expression(handle, interface_slots)),
        },
        TypedExpressionKind::FfiBufferOpen {
            length,
            element,
            layout_record,
        } => HirExpressionKind::FfiBufferOpen {
            length: Box::new(lower_expression(length, interface_slots)),
            element: *element,
            layout_record: *layout_record,
        },
        TypedExpressionKind::FfiBufferLength { buffer } => HirExpressionKind::FfiBufferLength {
            buffer: Box::new(lower_expression(buffer, interface_slots)),
        },
        TypedExpressionKind::FfiBufferRead { buffer, index } => HirExpressionKind::FfiBufferRead {
            buffer: Box::new(lower_expression(buffer, interface_slots)),
            index: Box::new(lower_expression(index, interface_slots)),
        },
        TypedExpressionKind::FfiBufferWrite {
            buffer,
            index,
            value,
        } => HirExpressionKind::FfiBufferWrite {
            buffer: Box::new(lower_expression(buffer, interface_slots)),
            index: Box::new(lower_expression(index, interface_slots)),
            value: Box::new(lower_expression(value, interface_slots)),
        },
        TypedExpressionKind::FfiBufferClose { buffer } => HirExpressionKind::FfiBufferClose {
            buffer: Box::new(lower_expression(buffer, interface_slots)),
        },
        call @ (TypedExpressionKind::StandardCall { .. }
        | TypedExpressionKind::DirectCall { .. }
        | TypedExpressionKind::ReferencedCall { .. }
        | TypedExpressionKind::IndirectCall { .. }
        | TypedExpressionKind::DirectMethodCall { .. }
        | TypedExpressionKind::InterfaceMethodCall { .. }
        | TypedExpressionKind::BuiltinInterfaceMethodCall { .. }) => {
            lower_call_expression(call, interface_slots)
        }
        TypedExpressionKind::InterfaceUpcast { value, interface } => {
            HirExpressionKind::InterfaceUpcast {
                value: Box::new(lower_expression(value, interface_slots)),
                interface: *interface,
            }
        }
        TypedExpressionKind::NumericConvert { value, conversion } => {
            HirExpressionKind::NumericConvert {
                value: Box::new(lower_expression(value, interface_slots)),
                conversion: *conversion,
            }
        }
    };
    HirExpression {
        kind,
        type_id: expression.type_id(),
        span: expression.span(),
    }
}

fn lower_call_expression(
    call: &TypedExpressionKind,
    interface_slots: &HirInterfaceSlotMap,
) -> HirExpressionKind {
    match call {
        TypedExpressionKind::StandardCall {
            function,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Standard {
                function: *function,
            },
            is_async: false,
            type_arguments: Vec::new(),
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::DirectCall {
            function,
            is_async,
            type_arguments,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Direct {
                function: *function,
            },
            is_async: *is_async,
            type_arguments: type_arguments.clone(),
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::ReferencedCall {
            function,
            is_async,
            type_arguments,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Referenced {
                function: *function,
            },
            is_async: *is_async,
            type_arguments: type_arguments.clone(),
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::IndirectCall {
            callee,
            is_async,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Indirect {
                callee: Box::new(lower_expression(callee, interface_slots)),
            },
            is_async: *is_async,
            type_arguments: Vec::new(),
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::DirectMethodCall {
            method,
            receiver,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::DirectMethod { method: *method },
            is_async: false,
            type_arguments: Vec::new(),
            arguments: receiver
                .iter()
                .map(|receiver| lower_expression(receiver, interface_slots))
                .chain(
                    arguments
                        .iter()
                        .map(|argument| lower_expression(argument, interface_slots)),
                )
                .collect(),
        },
        TypedExpressionKind::InterfaceMethodCall {
            interface,
            method,
            receiver,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::InterfaceMethod {
                interface: *interface,
                method: *method,
                slot: interface_slots[&(*interface, *method)],
            },
            is_async: false,
            type_arguments: Vec::new(),
            arguments: std::iter::once(lower_expression(receiver, interface_slots))
                .chain(
                    arguments
                        .iter()
                        .map(|argument| lower_expression(argument, interface_slots)),
                )
                .collect(),
        },
        TypedExpressionKind::BuiltinInterfaceMethodCall {
            interface,
            method,
            receiver,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::BuiltinInterfaceMethod {
                interface: *interface,
                method: *method,
            },
            is_async: false,
            type_arguments: Vec::new(),
            arguments: std::iter::once(lower_expression(receiver, interface_slots))
                .chain(
                    arguments
                        .iter()
                        .map(|argument| lower_expression(argument, interface_slots)),
                )
                .collect(),
        },
        _ => unreachable!("call lowering accepts only typed call expressions"),
    }
}

fn lower_closure(closure: &TypedClosure, interface_slots: &HirInterfaceSlotMap) -> HirClosure {
    HirClosure {
        function: closure.function(),
        is_async: closure.is_async(),
        parameters: closure
            .parameters()
            .iter()
            .map(|parameter| HirClosureParameter {
                binding: parameter.binding(),
                parameter: parameter.parameter(),
                name: parameter.name().to_owned(),
                type_id: parameter.type_id(),
                span: parameter.span(),
            })
            .collect(),
        results: closure.results().to_vec(),
        captures: closure.captures().iter().map(lower_capture).collect(),
        body: closure
            .body()
            .statements()
            .iter()
            .map(|statement| lower_statement(statement, interface_slots))
            .collect(),
        span: closure.span(),
        effects: pop_types::EffectSummary::empty(),
    }
}

fn lower_capture(capture: &TypedCapture) -> HirCapture {
    HirCapture {
        capture: capture.capture(),
        binding: capture.binding(),
        source: match capture.source() {
            CaptureSource::Local(local) => HirCaptureSource::Local(local),
            CaptureSource::Parameter(parameter) => HirCaptureSource::Parameter(parameter),
            CaptureSource::Capture(capture) => HirCaptureSource::Capture(capture),
        },
        type_id: capture.type_id(),
        mode: match capture.mode() {
            CaptureMode::Value => HirCaptureMode::Value,
            CaptureMode::Cell => HirCaptureMode::Cell,
        },
    }
}

fn lower_match_arm(arm: &TypedMatchArm, interface_slots: &HirInterfaceSlotMap) -> HirMatchArm {
    HirMatchArm {
        union: arm.union(),
        case: arm.case(),
        bindings: arm.bindings().iter().map(lower_match_binding).collect(),
        body: arm
            .body()
            .iter()
            .map(|statement| lower_statement(statement, interface_slots))
            .collect(),
        span: arm.span(),
    }
}

fn lower_match_binding(binding: &TypedMatchBinding) -> HirMatchBinding {
    HirMatchBinding {
        binding: binding.binding(),
        local: binding.local(),
        name: binding.name().to_owned(),
        type_id: binding.type_id(),
        span: binding.span(),
    }
}

pub(crate) fn lower_interface_implementation(
    implementation: &ClassInterfaceImplementation,
) -> HirInterfaceImplementation {
    HirInterfaceImplementation {
        interface: implementation.interface(),
        interface_type: implementation.interface_type(),
        methods: implementation
            .methods()
            .iter()
            .map(|method| HirInterfaceMethodImplementation {
                interface_method: method.interface_method(),
                slot: method.slot(),
                class_method: method.class_method(),
            })
            .collect(),
    }
}

fn collect_interface_slots(interfaces: &[InterfaceDefinition]) -> HirInterfaceSlotMap {
    interfaces
        .iter()
        .flat_map(|interface| {
            interface
                .methods()
                .iter()
                .map(move |method| ((interface.interface(), method.method()), method.slot()))
        })
        .collect()
}

fn first_unknown_interface_call(
    statements: &[TypedStatement],
    slots: &HirInterfaceSlotMap,
) -> Option<(InterfaceId, InterfaceMethodId, SourceSpan)> {
    for statement in statements {
        let found = match statement.kind() {
            TypedStatementKind::Local { initializer, .. } => {
                first_unknown_interface_expression(initializer, slots)
            }
            TypedStatementKind::MultipleLocal { value, .. } => {
                first_unknown_interface_expression(value, slots)
            }
            TypedStatementKind::LocalSet { value, .. }
            | TypedStatementKind::ParameterSet { value, .. }
            | TypedStatementKind::CaptureSet { value, .. }
            | TypedStatementKind::Expression(value) => {
                first_unknown_interface_expression(value, slots)
            }
            TypedStatementKind::Return { values } => values
                .iter()
                .find_map(|value| first_unknown_interface_expression(value, slots)),
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => first_unknown_interface_expression(condition, slots)
                .or_else(|| first_unknown_interface_call(then_body, slots))
                .or_else(|| first_unknown_interface_call(else_body, slots)),
            TypedStatementKind::OptionalIf {
                initializer,
                then_body,
                else_body,
                ..
            } => first_unknown_interface_expression(initializer, slots)
                .or_else(|| first_unknown_interface_call(then_body, slots))
                .or_else(|| first_unknown_interface_call(else_body, slots)),
            TypedStatementKind::While { condition, body } => {
                first_unknown_interface_expression(condition, slots)
                    .or_else(|| first_unknown_interface_call(body, slots))
            }
            TypedStatementKind::OptionalWhile {
                initializer, body, ..
            } => first_unknown_interface_expression(initializer, slots)
                .or_else(|| first_unknown_interface_call(body, slots)),
            TypedStatementKind::RepeatUntil { body, condition } => {
                first_unknown_interface_call(body, slots)
                    .or_else(|| first_unknown_interface_expression(condition, slots))
            }
            TypedStatementKind::NumericFor {
                first,
                last,
                step,
                body,
                ..
            } => first_unknown_interface_expression(first, slots)
                .or_else(|| first_unknown_interface_expression(last, slots))
                .or_else(|| first_unknown_interface_expression(step, slots))
                .or_else(|| first_unknown_interface_call(body, slots)),
            TypedStatementKind::GeneralizedFor { iterable, body, .. } => {
                first_unknown_interface_expression(iterable, slots)
                    .or_else(|| first_unknown_interface_call(body, slots))
            }
            TypedStatementKind::Break | TypedStatementKind::Continue => None,
            TypedStatementKind::Match {
                scrutinee, arms, ..
            } => first_unknown_interface_expression(scrutinee, slots).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_unknown_interface_call(arm.body(), slots))
            }),
            TypedStatementKind::ErrorMatch {
                scrutinee, arms, ..
            } => first_unknown_interface_expression(scrutinee, slots).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_unknown_interface_call(arm.body(), slots))
            }),
            TypedStatementKind::ResultMatch {
                scrutinee, arms, ..
            } => first_unknown_interface_expression(scrutinee, slots).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_unknown_interface_call(arm.body(), slots))
            }),
            TypedStatementKind::Defer { body } | TypedStatementKind::AsyncDefer { body } => {
                first_unknown_interface_call(body, slots)
            }
            TypedStatementKind::FieldSet { base, value, .. } => {
                first_unknown_interface_expression(base, slots)
                    .or_else(|| first_unknown_interface_expression(value, slots))
            }
            TypedStatementKind::CompoundFieldSet { base, value, .. } => {
                first_unknown_interface_expression(base, slots)
                    .or_else(|| first_unknown_interface_expression(value, slots))
            }
            TypedStatementKind::ArraySet {
                array,
                index,
                value,
            } => first_unknown_interface_expression(array, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
                .or_else(|| first_unknown_interface_expression(value, slots)),
            TypedStatementKind::ListSet { list, index, value } => {
                first_unknown_interface_expression(list, slots)
                    .or_else(|| first_unknown_interface_expression(index, slots))
                    .or_else(|| first_unknown_interface_expression(value, slots))
            }
            TypedStatementKind::TableSet { table, key, value } => {
                first_unknown_interface_expression(table, slots)
                    .or_else(|| first_unknown_interface_expression(key, slots))
                    .or_else(|| first_unknown_interface_expression(value, slots))
            }
            TypedStatementKind::CompoundArraySet {
                array,
                index,
                value,
                ..
            } => first_unknown_interface_expression(array, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
                .or_else(|| first_unknown_interface_expression(value, slots)),
            TypedStatementKind::MultipleAssignment { targets, value } => targets
                .iter()
                .find_map(|target| first_unknown_interface_target(target, slots))
                .or_else(|| first_unknown_interface_expression(value, slots)),
            TypedStatementKind::Call(call) => {
                if let TypedCallDispatch::InterfaceMethod {
                    interface, method, ..
                } = call.dispatch()
                    && !slots.contains_key(&(*interface, *method))
                {
                    Some((*interface, *method, call.span()))
                } else {
                    let receiver = match call.dispatch() {
                        TypedCallDispatch::Standard { .. }
                        | TypedCallDispatch::Direct { .. }
                        | TypedCallDispatch::Referenced { .. } => None,
                        TypedCallDispatch::DirectMethod { receiver, .. } => receiver
                            .as_deref()
                            .and_then(|value| first_unknown_interface_expression(value, slots)),
                        TypedCallDispatch::InterfaceMethod { receiver, .. } => {
                            first_unknown_interface_expression(receiver, slots)
                        }
                        TypedCallDispatch::BuiltinInterfaceMethod { receiver, .. } => {
                            first_unknown_interface_expression(receiver, slots)
                        }
                        TypedCallDispatch::Indirect { callee } => {
                            first_unknown_interface_expression(callee, slots)
                        }
                    };
                    receiver.or_else(|| {
                        call.arguments().iter().find_map(|argument| {
                            first_unknown_interface_expression(argument, slots)
                        })
                    })
                }
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn first_unknown_interface_target(
    target: &TypedAssignmentTarget,
    slots: &HirInterfaceSlotMap,
) -> Option<(InterfaceId, InterfaceMethodId, SourceSpan)> {
    match target {
        TypedAssignmentTarget::Local { .. } | TypedAssignmentTarget::Capture { .. } => None,
        TypedAssignmentTarget::Field { base, .. } => {
            first_unknown_interface_expression(base, slots)
        }
        TypedAssignmentTarget::Array { array, index, .. } => {
            first_unknown_interface_expression(array, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
        }
        TypedAssignmentTarget::List { list, index, .. } => {
            first_unknown_interface_expression(list, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
        }
        TypedAssignmentTarget::Table { table, key, .. } => {
            first_unknown_interface_expression(table, slots)
                .or_else(|| first_unknown_interface_expression(key, slots))
        }
    }
}

fn first_unknown_interface_expression(
    expression: &TypedExpression,
    slots: &HirInterfaceSlotMap,
) -> Option<(InterfaceId, InterfaceMethodId, SourceSpan)> {
    match expression.kind() {
        TypedExpressionKind::InterfaceMethodCall {
            interface,
            method,
            receiver,
            arguments,
        } => {
            if !slots.contains_key(&(*interface, *method)) {
                return Some((*interface, *method, expression.span()));
            }
            first_unknown_interface_expression(receiver, slots).or_else(|| {
                arguments
                    .iter()
                    .find_map(|argument| first_unknown_interface_expression(argument, slots))
            })
        }
        TypedExpressionKind::BuiltinInterfaceMethodCall {
            receiver,
            arguments,
            ..
        } => first_unknown_interface_expression(receiver, slots).or_else(|| {
            arguments
                .iter()
                .find_map(|argument| first_unknown_interface_expression(argument, slots))
        }),
        TypedExpressionKind::Closure(closure) => {
            first_unknown_interface_call(closure.body().statements(), slots)
        }
        TypedExpressionKind::Field { base, .. } => first_unknown_interface_expression(base, slots),
        TypedExpressionKind::ClassConstruct { fields, .. }
        | TypedExpressionKind::Record { fields, .. } => fields
            .iter()
            .find_map(|field| first_unknown_interface_expression(field.value(), slots)),
        TypedExpressionKind::ArrayGet { array, index }
        | TypedExpressionKind::ListGet { list: array, index }
        | TypedExpressionKind::ListGetChecked { list: array, index } => {
            first_unknown_interface_expression(array, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
        }
        TypedExpressionKind::TableGet { table, key } => {
            first_unknown_interface_expression(table, slots)
                .or_else(|| first_unknown_interface_expression(key, slots))
        }
        TypedExpressionKind::TupleGet { tuple, .. } => {
            first_unknown_interface_expression(tuple, slots)
        }
        TypedExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => first_unknown_interface_expression(length, slots)
            .or_else(|| first_unknown_interface_expression(initial_value, slots)),
        TypedExpressionKind::ArrayLength { array } => {
            first_unknown_interface_expression(array, slots)
        }
        TypedExpressionKind::ArrayGetChecked { array, index } => {
            first_unknown_interface_expression(array, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
        }
        TypedExpressionKind::ArrayFill { array, value } => {
            first_unknown_interface_expression(array, slots)
                .or_else(|| first_unknown_interface_expression(value, slots))
        }
        TypedExpressionKind::ListCreate { capacity } => capacity
            .as_deref()
            .and_then(|capacity| first_unknown_interface_expression(capacity, slots)),
        TypedExpressionKind::ListLength { list } => first_unknown_interface_expression(list, slots),
        TypedExpressionKind::ListAdd { list, value } => {
            first_unknown_interface_expression(list, slots)
                .or_else(|| first_unknown_interface_expression(value, slots))
        }
        TypedExpressionKind::RangeCreate { first, last, step } => {
            first_unknown_interface_expression(first, slots)
                .or_else(|| first_unknown_interface_expression(last, slots))
                .or_else(|| first_unknown_interface_expression(step, slots))
        }
        TypedExpressionKind::RecordUpdate { base, fields, .. } => {
            first_unknown_interface_expression(base, slots).or_else(|| {
                fields
                    .iter()
                    .find_map(|field| first_unknown_interface_expression(field.value(), slots))
            })
        }
        TypedExpressionKind::Array(elements) | TypedExpressionKind::Tuple(elements) => elements
            .iter()
            .find_map(|element| first_unknown_interface_expression(element, slots)),
        TypedExpressionKind::Table(entries) => entries.iter().find_map(|entry| {
            first_unknown_interface_expression(entry.key(), slots)
                .or_else(|| first_unknown_interface_expression(entry.value(), slots))
        }),
        TypedExpressionKind::UnionCase { arguments, .. }
        | TypedExpressionKind::ResultCase { arguments, .. }
        | TypedExpressionKind::IterationCase { arguments, .. }
        | TypedExpressionKind::ErrorCase { arguments, .. }
        | TypedExpressionKind::DirectCall { arguments, .. }
        | TypedExpressionKind::ReferencedCall { arguments, .. }
        | TypedExpressionKind::StandardCall { arguments, .. } => arguments
            .iter()
            .find_map(|argument| first_unknown_interface_expression(argument, slots)),
        TypedExpressionKind::Unary { operand, .. }
        | TypedExpressionKind::Await { task: operand }
        | TypedExpressionKind::TaskCancelToken { source: operand }
        | TypedExpressionKind::TaskCancel { source: operand } => {
            first_unknown_interface_expression(operand, slots)
        }
        TypedExpressionKind::FfiHandleOpen { value: operand }
        | TypedExpressionKind::FfiHandleGet { handle: operand }
        | TypedExpressionKind::FfiHandleClose { handle: operand }
        | TypedExpressionKind::FfiBufferOpen {
            length: operand, ..
        }
        | TypedExpressionKind::FfiBufferLength { buffer: operand }
        | TypedExpressionKind::FfiBufferClose { buffer: operand } => {
            first_unknown_interface_expression(operand, slots)
        }
        TypedExpressionKind::FfiBufferRead { buffer, index } => {
            first_unknown_interface_expression(buffer, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
        }
        TypedExpressionKind::FfiBufferWrite {
            buffer,
            index,
            value,
        } => first_unknown_interface_expression(buffer, slots)
            .or_else(|| first_unknown_interface_expression(index, slots))
            .or_else(|| first_unknown_interface_expression(value, slots)),
        TypedExpressionKind::TaskGroup { cancel, body } => {
            first_unknown_interface_expression(cancel, slots)
                .or_else(|| first_unknown_interface_expression(body, slots))
        }
        TypedExpressionKind::TaskStart { group, task } => {
            first_unknown_interface_expression(group, slots)
                .or_else(|| first_unknown_interface_expression(task, slots))
        }
        TypedExpressionKind::Binary { left, right, .. } => {
            first_unknown_interface_expression(left, slots)
                .or_else(|| first_unknown_interface_expression(right, slots))
        }
        TypedExpressionKind::OptionalDefault { optional, fallback } => {
            first_unknown_interface_expression(optional, slots)
                .or_else(|| first_unknown_interface_expression(fallback, slots))
        }
        TypedExpressionKind::OptionalPropagate { optional, .. }
        | TypedExpressionKind::OptionalNarrow { optional } => {
            first_unknown_interface_expression(optional, slots)
        }
        TypedExpressionKind::ResultPropagate { result, .. } => {
            first_unknown_interface_expression(result, slots)
        }
        TypedExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => first_unknown_interface_expression(condition, slots)
            .or_else(|| first_unknown_interface_expression(when_true, slots))
            .or_else(|| first_unknown_interface_expression(when_false, slots)),
        TypedExpressionKind::StringConcat { left, right } => {
            first_unknown_interface_expression(left, slots)
                .or_else(|| first_unknown_interface_expression(right, slots))
        }
        TypedExpressionKind::StringFormat { value, .. } => {
            first_unknown_interface_expression(value, slots)
        }
        TypedExpressionKind::IndirectCall {
            callee, arguments, ..
        } => first_unknown_interface_expression(callee, slots).or_else(|| {
            arguments
                .iter()
                .find_map(|argument| first_unknown_interface_expression(argument, slots))
        }),
        TypedExpressionKind::DirectMethodCall {
            receiver,
            arguments,
            ..
        } => receiver
            .as_deref()
            .and_then(|value| first_unknown_interface_expression(value, slots))
            .or_else(|| {
                arguments
                    .iter()
                    .find_map(|argument| first_unknown_interface_expression(argument, slots))
            }),
        TypedExpressionKind::InterfaceUpcast { value, .. } => {
            first_unknown_interface_expression(value, slots)
        }
        TypedExpressionKind::NumericConvert { value, .. } => {
            first_unknown_interface_expression(value, slots)
        }
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
        | TypedExpressionKind::EnumCase { .. } => None,
    }
}

fn first_compile_time_only_statement(statements: &[TypedStatement]) -> Option<SourceSpan> {
    for statement in statements {
        let found = match statement.kind() {
            TypedStatementKind::Local { initializer, .. } => {
                first_compile_time_only_expression(initializer)
            }
            TypedStatementKind::MultipleLocal { value, .. } => {
                first_compile_time_only_expression(value)
            }
            TypedStatementKind::LocalSet { value, .. }
            | TypedStatementKind::ParameterSet { value, .. }
            | TypedStatementKind::CaptureSet { value, .. }
            | TypedStatementKind::Expression(value) => first_compile_time_only_expression(value),
            TypedStatementKind::Return { values } => {
                values.iter().find_map(first_compile_time_only_expression)
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => first_compile_time_only_expression(condition)
                .or_else(|| first_compile_time_only_statement(then_body))
                .or_else(|| first_compile_time_only_statement(else_body)),
            TypedStatementKind::OptionalIf {
                initializer,
                then_body,
                else_body,
                ..
            } => first_compile_time_only_expression(initializer)
                .or_else(|| first_compile_time_only_statement(then_body))
                .or_else(|| first_compile_time_only_statement(else_body)),
            TypedStatementKind::While { condition, body } => {
                first_compile_time_only_expression(condition)
                    .or_else(|| first_compile_time_only_statement(body))
            }
            TypedStatementKind::OptionalWhile {
                initializer, body, ..
            } => first_compile_time_only_expression(initializer)
                .or_else(|| first_compile_time_only_statement(body)),
            TypedStatementKind::RepeatUntil { body, condition } => {
                first_compile_time_only_statement(body)
                    .or_else(|| first_compile_time_only_expression(condition))
            }
            TypedStatementKind::NumericFor {
                first,
                last,
                step,
                body,
                ..
            } => first_compile_time_only_expression(first)
                .or_else(|| first_compile_time_only_expression(last))
                .or_else(|| first_compile_time_only_expression(step))
                .or_else(|| first_compile_time_only_statement(body)),
            TypedStatementKind::GeneralizedFor { iterable, body, .. } => {
                first_compile_time_only_expression(iterable)
                    .or_else(|| first_compile_time_only_statement(body))
            }
            TypedStatementKind::Break | TypedStatementKind::Continue => None,
            TypedStatementKind::Match {
                scrutinee, arms, ..
            } => first_compile_time_only_expression(scrutinee).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_compile_time_only_statement(arm.body()))
            }),
            TypedStatementKind::ErrorMatch {
                scrutinee, arms, ..
            } => first_compile_time_only_expression(scrutinee).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_compile_time_only_statement(arm.body()))
            }),
            TypedStatementKind::ResultMatch {
                scrutinee, arms, ..
            } => first_compile_time_only_expression(scrutinee).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_compile_time_only_statement(arm.body()))
            }),
            TypedStatementKind::Defer { body } | TypedStatementKind::AsyncDefer { body } => {
                first_compile_time_only_statement(body)
            }
            TypedStatementKind::FieldSet { base, value, .. } => {
                first_compile_time_only_expression(base)
                    .or_else(|| first_compile_time_only_expression(value))
            }
            TypedStatementKind::CompoundFieldSet { base, value, .. } => {
                first_compile_time_only_expression(base)
                    .or_else(|| first_compile_time_only_expression(value))
            }
            TypedStatementKind::ArraySet {
                array,
                index,
                value,
            } => first_compile_time_only_expression(array)
                .or_else(|| first_compile_time_only_expression(index))
                .or_else(|| first_compile_time_only_expression(value)),
            TypedStatementKind::ListSet { list, index, value } => {
                first_compile_time_only_expression(list)
                    .or_else(|| first_compile_time_only_expression(index))
                    .or_else(|| first_compile_time_only_expression(value))
            }
            TypedStatementKind::TableSet { table, key, value } => {
                first_compile_time_only_expression(table)
                    .or_else(|| first_compile_time_only_expression(key))
                    .or_else(|| first_compile_time_only_expression(value))
            }
            TypedStatementKind::CompoundArraySet {
                array,
                index,
                value,
                ..
            } => first_compile_time_only_expression(array)
                .or_else(|| first_compile_time_only_expression(index))
                .or_else(|| first_compile_time_only_expression(value)),
            TypedStatementKind::MultipleAssignment { targets, value } => targets
                .iter()
                .find_map(first_compile_time_only_target)
                .or_else(|| first_compile_time_only_expression(value)),
            TypedStatementKind::Call(call) => first_compile_time_only_call(call),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn first_compile_time_only_target(target: &TypedAssignmentTarget) -> Option<SourceSpan> {
    match target {
        TypedAssignmentTarget::Local { .. } | TypedAssignmentTarget::Capture { .. } => None,
        TypedAssignmentTarget::Field { base, .. } => first_compile_time_only_expression(base),
        TypedAssignmentTarget::Array { array, index, .. } => {
            first_compile_time_only_expression(array)
                .or_else(|| first_compile_time_only_expression(index))
        }
        TypedAssignmentTarget::List { list, index, .. } => first_compile_time_only_expression(list)
            .or_else(|| first_compile_time_only_expression(index)),
        TypedAssignmentTarget::Table { table, key, .. } => {
            first_compile_time_only_expression(table)
                .or_else(|| first_compile_time_only_expression(key))
        }
    }
}

fn first_compile_time_only_call(call: &TypedCall) -> Option<SourceSpan> {
    let callee = match call.dispatch() {
        TypedCallDispatch::Standard { .. }
        | TypedCallDispatch::Direct { .. }
        | TypedCallDispatch::Referenced { .. } => None,
        TypedCallDispatch::DirectMethod { receiver, .. } => receiver
            .as_deref()
            .and_then(first_compile_time_only_expression),
        TypedCallDispatch::InterfaceMethod { receiver, .. } => {
            first_compile_time_only_expression(receiver)
        }
        TypedCallDispatch::BuiltinInterfaceMethod { receiver, .. } => {
            first_compile_time_only_expression(receiver)
        }
        TypedCallDispatch::Indirect { callee } => first_compile_time_only_expression(callee),
    };
    callee.or_else(|| {
        call.arguments()
            .iter()
            .find_map(first_compile_time_only_expression)
    })
}

fn first_compile_time_only_expression(expression: &TypedExpression) -> Option<SourceSpan> {
    match expression.kind() {
        TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. } => Some(expression.span()),
        TypedExpressionKind::Closure(closure) => {
            first_compile_time_only_statement(closure.body().statements())
        }
        TypedExpressionKind::Field { base, .. } => first_compile_time_only_expression(base),
        TypedExpressionKind::ClassConstruct { fields, .. }
        | TypedExpressionKind::Record { fields, .. } => fields
            .iter()
            .find_map(|field| first_compile_time_only_expression(field.value())),
        TypedExpressionKind::ArrayGet { array, index }
        | TypedExpressionKind::ListGet { list: array, index }
        | TypedExpressionKind::ListGetChecked { list: array, index } => {
            first_compile_time_only_expression(array)
                .or_else(|| first_compile_time_only_expression(index))
        }
        TypedExpressionKind::TableGet { table, key } => first_compile_time_only_expression(table)
            .or_else(|| first_compile_time_only_expression(key)),
        TypedExpressionKind::TupleGet { tuple, .. } => first_compile_time_only_expression(tuple),
        TypedExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => first_compile_time_only_expression(length)
            .or_else(|| first_compile_time_only_expression(initial_value)),
        TypedExpressionKind::ArrayLength { array } => first_compile_time_only_expression(array),
        TypedExpressionKind::ArrayGetChecked { array, index } => {
            first_compile_time_only_expression(array)
                .or_else(|| first_compile_time_only_expression(index))
        }
        TypedExpressionKind::ArrayFill { array, value } => {
            first_compile_time_only_expression(array)
                .or_else(|| first_compile_time_only_expression(value))
        }
        TypedExpressionKind::ListCreate { capacity } => capacity
            .as_deref()
            .and_then(first_compile_time_only_expression),
        TypedExpressionKind::ListLength { list } => first_compile_time_only_expression(list),
        TypedExpressionKind::ListAdd { list, value } => first_compile_time_only_expression(list)
            .or_else(|| first_compile_time_only_expression(value)),
        TypedExpressionKind::RangeCreate { first, last, step } => {
            first_compile_time_only_expression(first)
                .or_else(|| first_compile_time_only_expression(last))
                .or_else(|| first_compile_time_only_expression(step))
        }
        TypedExpressionKind::RecordUpdate { base, fields, .. } => {
            first_compile_time_only_expression(base).or_else(|| {
                fields
                    .iter()
                    .find_map(|field| first_compile_time_only_expression(field.value()))
            })
        }
        TypedExpressionKind::Array(elements) | TypedExpressionKind::Tuple(elements) => {
            elements.iter().find_map(first_compile_time_only_expression)
        }
        TypedExpressionKind::Table(entries) => entries.iter().find_map(|entry| {
            first_compile_time_only_expression(entry.key())
                .or_else(|| first_compile_time_only_expression(entry.value()))
        }),
        TypedExpressionKind::UnionCase { arguments, .. }
        | TypedExpressionKind::ResultCase { arguments, .. }
        | TypedExpressionKind::IterationCase { arguments, .. }
        | TypedExpressionKind::ErrorCase { arguments, .. }
        | TypedExpressionKind::DirectCall { arguments, .. }
        | TypedExpressionKind::ReferencedCall { arguments, .. }
        | TypedExpressionKind::StandardCall { arguments, .. } => arguments
            .iter()
            .find_map(first_compile_time_only_expression),
        TypedExpressionKind::Unary { operand, .. }
        | TypedExpressionKind::Await { task: operand }
        | TypedExpressionKind::TaskCancelToken { source: operand }
        | TypedExpressionKind::TaskCancel { source: operand } => {
            first_compile_time_only_expression(operand)
        }
        TypedExpressionKind::FfiHandleOpen { value: operand }
        | TypedExpressionKind::FfiHandleGet { handle: operand }
        | TypedExpressionKind::FfiHandleClose { handle: operand }
        | TypedExpressionKind::FfiBufferOpen {
            length: operand, ..
        }
        | TypedExpressionKind::FfiBufferLength { buffer: operand }
        | TypedExpressionKind::FfiBufferClose { buffer: operand } => {
            first_compile_time_only_expression(operand)
        }
        TypedExpressionKind::FfiBufferRead { buffer, index } => {
            first_compile_time_only_expression(buffer)
                .or_else(|| first_compile_time_only_expression(index))
        }
        TypedExpressionKind::FfiBufferWrite {
            buffer,
            index,
            value,
        } => first_compile_time_only_expression(buffer)
            .or_else(|| first_compile_time_only_expression(index))
            .or_else(|| first_compile_time_only_expression(value)),
        TypedExpressionKind::TaskGroup { cancel, body } => {
            first_compile_time_only_expression(cancel)
                .or_else(|| first_compile_time_only_expression(body))
        }
        TypedExpressionKind::TaskStart { group, task } => first_compile_time_only_expression(group)
            .or_else(|| first_compile_time_only_expression(task)),
        TypedExpressionKind::Binary { left, right, .. } => first_compile_time_only_expression(left)
            .or_else(|| first_compile_time_only_expression(right)),
        TypedExpressionKind::OptionalDefault { optional, fallback } => {
            first_compile_time_only_expression(optional)
                .or_else(|| first_compile_time_only_expression(fallback))
        }
        TypedExpressionKind::OptionalPropagate { optional, .. }
        | TypedExpressionKind::OptionalNarrow { optional } => {
            first_compile_time_only_expression(optional)
        }
        TypedExpressionKind::ResultPropagate { result, .. } => {
            first_compile_time_only_expression(result)
        }
        TypedExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => first_compile_time_only_expression(condition)
            .or_else(|| first_compile_time_only_expression(when_true))
            .or_else(|| first_compile_time_only_expression(when_false)),
        TypedExpressionKind::StringConcat { left, right } => {
            first_compile_time_only_expression(left)
                .or_else(|| first_compile_time_only_expression(right))
        }
        TypedExpressionKind::StringFormat { value, .. } => {
            first_compile_time_only_expression(value)
        }
        TypedExpressionKind::IndirectCall {
            callee, arguments, ..
        } => first_compile_time_only_expression(callee).or_else(|| {
            arguments
                .iter()
                .find_map(first_compile_time_only_expression)
        }),
        TypedExpressionKind::DirectMethodCall {
            receiver,
            arguments,
            ..
        } => receiver
            .as_deref()
            .and_then(first_compile_time_only_expression)
            .or_else(|| {
                arguments
                    .iter()
                    .find_map(first_compile_time_only_expression)
            }),
        TypedExpressionKind::InterfaceMethodCall {
            receiver,
            arguments,
            ..
        }
        | TypedExpressionKind::BuiltinInterfaceMethodCall {
            receiver,
            arguments,
            ..
        } => first_compile_time_only_expression(receiver).or_else(|| {
            arguments
                .iter()
                .find_map(first_compile_time_only_expression)
        }),
        TypedExpressionKind::InterfaceUpcast { value, .. } => {
            first_compile_time_only_expression(value)
        }
        TypedExpressionKind::NumericConvert { value, .. } => {
            first_compile_time_only_expression(value)
        }
        TypedExpressionKind::Integer(_)
        | TypedExpressionKind::Float(_)
        | TypedExpressionKind::String(_)
        | TypedExpressionKind::Boolean(_)
        | TypedExpressionKind::Nil
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::Parameter(_)
        | TypedExpressionKind::Capture(_)
        | TypedExpressionKind::Function(_)
        | TypedExpressionKind::TaskCancellationSource
        | TypedExpressionKind::EnumCase { .. } => None,
    }
}

fn lower_field_value(
    field: &TypedFieldValue,
    interface_slots: &HirInterfaceSlotMap,
) -> HirFieldValue {
    HirFieldValue {
        field: field.field(),
        value: lower_expression(field.value(), interface_slots),
        span: field.span(),
    }
}

fn lower_table_entry(
    entry: &TypedTableEntry,
    interface_slots: &HirInterfaceSlotMap,
) -> HirTableEntry {
    HirTableEntry {
        key: lower_expression(entry.key(), interface_slots),
        value: lower_expression(entry.value(), interface_slots),
        span: entry.span(),
    }
}
