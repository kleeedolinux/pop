//! Closed effect inference for verified source HIR.

use std::collections::BTreeMap;

use pop_foundation::{
    BuiltinTypeId, CaptureId, InterfaceId, InterfaceMethodId, IterationProtocolMethodId, LocalId,
    MethodId, SymbolId, SymbolIdentity, TypeId,
};
use pop_types::{
    Effect, EffectSummary, PrimitiveType, SemanticType, TypeArena, TypedBinaryOperator,
    TypedCompoundOperator, TypedUnaryOperator,
};

use crate::*;

type InterfaceEffectMap = BTreeMap<(InterfaceId, InterfaceMethodId), EffectSummary>;
type BuiltinInterfaceEffectMap =
    BTreeMap<(BuiltinTypeId, IterationProtocolMethodId), EffectSummary>;

/// One source-facing ADR 0022 contract violation found after closing effects.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EffectInferenceViolation {
    InterfaceImplementationWidening {
        class: String,
        interface: String,
        method: String,
        span: pop_foundation::SourceSpan,
    },
    CallableParameterWidening {
        expected: TypeId,
        found: TypeId,
        span: pop_foundation::SourceSpan,
    },
}

/// Computes ADR 0022's least fixed point and closes callable value types.
pub fn infer_hir_effects(
    bubble: &mut HirBubble,
    arena: &mut TypeArena,
) -> Vec<EffectInferenceViolation> {
    let foreign = bubble
        .foreign_functions
        .iter()
        .map(|function| (function.symbol(), function.effects()))
        .collect::<BTreeMap<_, _>>();
    let referenced = bubble
        .function_references
        .iter()
        .map(|function| (function.identity(), function.effects()))
        .collect::<BTreeMap<_, _>>();
    let mut functions = bubble
        .functions
        .iter()
        .map(|function| (function.symbol(), EffectSummary::empty()))
        .chain(foreign.iter().map(|(symbol, effects)| (*symbol, *effects)))
        .collect::<BTreeMap<_, _>>();
    let mut methods = bubble
        .methods
        .iter()
        .map(|method| (method.method(), EffectSummary::empty()))
        .collect::<BTreeMap<_, _>>();
    let (interface_methods, builtin_interface_methods) = declared_interface_effects(bubble);

    loop {
        let prior_functions = functions.clone();
        let prior_methods = methods.clone();
        let context = EffectContext {
            functions: &prior_functions,
            referenced: &referenced,
            methods: &prior_methods,
            interface_methods: &interface_methods,
            builtin_interface_methods: &builtin_interface_methods,
            arena,
        };
        let mut changed = false;
        for function in &mut bubble.functions {
            let effects = infer_statements(
                &mut function.body,
                &context,
                &mut CallableEnvironment::default(),
            );
            changed |= functions.get(&function.symbol()).copied() != Some(effects);
            function.effects = effects;
            functions.insert(function.symbol(), effects);
        }
        for method in &mut bubble.methods {
            let effects = infer_statements(
                &mut method.function.body,
                &context,
                &mut CallableEnvironment::default(),
            );
            changed |= methods.get(&method.method()).copied() != Some(effects);
            method.function.effects = effects;
            methods.insert(method.method(), effects);
        }
        if !changed {
            break;
        }
    }

    for function in &mut bubble.functions {
        rewrite_statement_types(&mut function.body, &functions, arena, &mut BTreeMap::new());
    }
    for method in &mut bubble.methods {
        rewrite_statement_types(
            &mut method.function.body,
            &functions,
            arena,
            &mut BTreeMap::new(),
        );
    }

    effect_contract_violations(bubble, arena)
}

struct EffectContext<'a> {
    functions: &'a BTreeMap<SymbolId, EffectSummary>,
    referenced: &'a BTreeMap<SymbolIdentity, EffectSummary>,
    methods: &'a BTreeMap<MethodId, EffectSummary>,
    interface_methods: &'a InterfaceEffectMap,
    builtin_interface_methods: &'a BuiltinInterfaceEffectMap,
    arena: &'a TypeArena,
}

fn declared_interface_effects(
    bubble: &HirBubble,
) -> (InterfaceEffectMap, BuiltinInterfaceEffectMap) {
    let mut interfaces = BTreeMap::new();
    let mut builtins = BTreeMap::new();
    if let Some(protocol) = pop_types::embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol())
    {
        for (interface, method) in [
            (protocol.iterable(), protocol.iterator_method()),
            (protocol.iterator(), protocol.iterator_method()),
            (protocol.iterator(), protocol.next_method()),
        ] {
            if let Some(effects) = protocol.method_effects(interface, method) {
                builtins.insert((interface, method), effects);
            }
        }
    }
    for declaration in &bubble.declarations {
        if let HirDeclarationKind::Interface(interface) = &declaration.kind {
            for method in &interface.methods {
                interfaces.insert((interface.interface, method.method), method.effects);
            }
            continue;
        }
    }
    (interfaces, builtins)
}

fn effect_contract_violations(
    bubble: &HirBubble,
    arena: &TypeArena,
) -> Vec<EffectInferenceViolation> {
    let methods = bubble
        .methods
        .iter()
        .map(|method| (method.method(), method.function.effects))
        .collect::<BTreeMap<_, _>>();
    let interfaces = bubble
        .declarations
        .iter()
        .filter_map(|declaration| match &declaration.kind {
            HirDeclarationKind::Interface(interface) => Some((interface.interface, declaration)),
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut violations = Vec::new();
    let iteration_protocol = pop_types::embedded_bootstrap_schema()
        .ok()
        .and_then(|schema| schema.iteration_protocol());
    for declaration in &bubble.declarations {
        let HirDeclarationKind::Class(class) = &declaration.kind else {
            continue;
        };
        for implementation in &class.interfaces {
            let Some(interface_declaration) = interfaces.get(&implementation.interface) else {
                continue;
            };
            let HirDeclarationKind::Interface(interface) = &interface_declaration.kind else {
                continue;
            };
            for mapping in &implementation.methods {
                let Some(required) = interface
                    .methods
                    .iter()
                    .find(|method| method.method == mapping.interface_method)
                else {
                    continue;
                };
                let found = methods
                    .get(&mapping.class_method)
                    .copied()
                    .unwrap_or_default();
                if found.is_subset_of(required.effects) {
                    continue;
                }
                let implementation_method = class
                    .methods
                    .iter()
                    .find(|method| method.method == mapping.class_method);
                violations.push(EffectInferenceViolation::InterfaceImplementationWidening {
                    class: declaration.name.clone(),
                    interface: interface_declaration.name.clone(),
                    method: required.name.clone(),
                    span: implementation_method.map_or(required.span, |method| method.span),
                });
            }
        }
        let Some(protocol) = iteration_protocol else {
            continue;
        };
        for implementation in &class.builtin_interfaces {
            for mapping in &implementation.methods {
                let Some(required) =
                    protocol.method_effects(implementation.interface, mapping.protocol_method)
                else {
                    continue;
                };
                let found = methods
                    .get(&mapping.class_method)
                    .copied()
                    .unwrap_or_default();
                if found.is_subset_of(required) {
                    continue;
                }
                let implementation_method = class
                    .methods
                    .iter()
                    .find(|method| method.method == mapping.class_method);
                let interface = if implementation.interface == protocol.iterator() {
                    "Iterator"
                } else {
                    "Iterable"
                };
                let method = if mapping.protocol_method == protocol.next_method() {
                    "next"
                } else {
                    "iterator"
                };
                violations.push(EffectInferenceViolation::InterfaceImplementationWidening {
                    class: declaration.name.clone(),
                    interface: interface.to_owned(),
                    method: method.to_owned(),
                    span: implementation_method.map_or(declaration.span, |method| method.span),
                });
            }
        }
    }

    if let Err(errors) = crate::verify_hir_bubble(bubble, arena) {
        for error in errors {
            let crate::HirVerificationError::CallArgumentTypeMismatch {
                expected,
                found,
                span,
                ..
            } = error
            else {
                continue;
            };
            let (
                Some(SemanticType::Function {
                    effects: expected_effects,
                    ..
                }),
                Some(SemanticType::Function {
                    effects: found_effects,
                    ..
                }),
            ) = (arena.get(expected), arena.get(found))
            else {
                continue;
            };
            if !found_effects.is_subset_of(*expected_effects) {
                violations.push(EffectInferenceViolation::CallableParameterWidening {
                    expected,
                    found,
                    span,
                });
            }
        }
    }
    violations.sort_by_key(|violation| match violation {
        EffectInferenceViolation::InterfaceImplementationWidening { span, .. }
        | EffectInferenceViolation::CallableParameterWidening { span, .. } => {
            (span.file(), span.range().start())
        }
    });
    violations
}

#[derive(Clone, Default)]
struct CallableEnvironment {
    locals: BTreeMap<LocalId, EffectSummary>,
    captures: BTreeMap<CaptureId, EffectSummary>,
}

fn infer_statements(
    statements: &mut [HirStatement],
    context: &EffectContext<'_>,
    environment: &mut CallableEnvironment,
) -> EffectSummary {
    let mut summary = EffectSummary::empty();
    for statement in statements {
        summary = summary.union(infer_statement(statement, context, environment));
    }
    summary
}

#[allow(clippy::too_many_lines)]
fn infer_statement(
    statement: &mut HirStatement,
    context: &EffectContext<'_>,
    environment: &mut CallableEnvironment,
) -> EffectSummary {
    match &mut statement.kind {
        HirStatementKind::Local {
            local, initializer, ..
        } => {
            let effects = infer_expression(initializer, context, environment);
            if let Some(callable) = callable_effects(initializer, context, environment) {
                environment.locals.insert(*local, callable);
            }
            effects
        }
        HirStatementKind::MultipleLocal { value, .. }
        | HirStatementKind::ParameterSet { value, .. } => {
            infer_expression(value, context, environment)
        }
        HirStatementKind::LocalSet { local, value } => {
            let effects = infer_expression(value, context, environment);
            if let Some(callable) = callable_effects(value, context, environment) {
                environment.locals.insert(*local, callable);
            }
            effects
        }
        HirStatementKind::CaptureSet { capture, value } => {
            let effects =
                infer_expression(value, context, environment).with(Effect::WritesManagedReference);
            if let Some(callable) = callable_effects(value, context, environment) {
                environment.captures.insert(*capture, callable);
            }
            effects
        }
        HirStatementKind::Return { values } => infer_expressions(values, context, environment),
        HirStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            let condition = infer_expression(condition, context, environment);
            let mut then_environment = environment.clone();
            let mut else_environment = environment.clone();
            condition
                .union(infer_statements(then_body, context, &mut then_environment))
                .union(infer_statements(else_body, context, &mut else_environment))
        }
        HirStatementKind::OptionalIf {
            initializer,
            then_body,
            else_body,
            ..
        } => {
            let initializer = infer_expression(initializer, context, environment);
            let mut then_environment = environment.clone();
            let mut else_environment = environment.clone();
            initializer
                .union(infer_statements(then_body, context, &mut then_environment))
                .union(infer_statements(else_body, context, &mut else_environment))
        }
        HirStatementKind::While { condition, body } => {
            let condition = infer_expression(condition, context, environment);
            let mut body_environment = environment.clone();
            condition.union(infer_statements(body, context, &mut body_environment))
        }
        HirStatementKind::OptionalWhile {
            initializer, body, ..
        } => {
            let initializer = infer_expression(initializer, context, environment);
            let mut body_environment = environment.clone();
            initializer.union(infer_statements(body, context, &mut body_environment))
        }
        HirStatementKind::RepeatUntil { body, condition } => {
            let mut body_environment = environment.clone();
            infer_statements(body, context, &mut body_environment).union(infer_expression(
                condition,
                context,
                &body_environment,
            ))
        }
        HirStatementKind::NumericFor {
            first,
            last,
            step,
            body,
            ..
        } => {
            let mut body_environment = environment.clone();
            infer_expression(first, context, environment)
                .union(infer_expression(last, context, environment))
                .union(infer_expression(step, context, environment))
                .union(infer_statements(body, context, &mut body_environment))
                .with(Effect::MayTrap)
        }
        HirStatementKind::GeneralizedFor {
            protocol,
            source,
            iterable,
            body,
            ..
        } => {
            let mut body_environment = environment.clone();
            let iteration_effects = match source {
                HirIterationSource::ClassIterable { iterator_method } => context
                    .methods
                    .get(iterator_method)
                    .copied()
                    .unwrap_or_default(),
                HirIterationSource::ClassIterator {
                    iterator_method,
                    next_method,
                } => context
                    .methods
                    .get(iterator_method)
                    .copied()
                    .unwrap_or_default()
                    .union(
                        context
                            .methods
                            .get(next_method)
                            .copied()
                            .unwrap_or_default(),
                    ),
                HirIterationSource::Iterable | HirIterationSource::BoundIterable => context
                    .builtin_interface_methods
                    .get(&(protocol.iterable(), protocol.iterator_method()))
                    .copied()
                    .unwrap_or_default()
                    .union(
                        context
                            .builtin_interface_methods
                            .get(&(protocol.iterator(), protocol.next_method()))
                            .copied()
                            .unwrap_or_default(),
                    ),
                HirIterationSource::Iterator | HirIterationSource::BoundIterator => context
                    .builtin_interface_methods
                    .get(&(protocol.iterator(), protocol.iterator_method()))
                    .copied()
                    .unwrap_or_default()
                    .union(
                        context
                            .builtin_interface_methods
                            .get(&(protocol.iterator(), protocol.next_method()))
                            .copied()
                            .unwrap_or_default(),
                    ),
                HirIterationSource::Array
                | HirIterationSource::List
                | HirIterationSource::Range
                | HirIterationSource::Table => EffectSummary::empty(),
            };
            infer_expression(iterable, context, environment)
                .union(iteration_effects)
                .union(infer_statements(body, context, &mut body_environment))
        }
        HirStatementKind::Match {
            scrutinee, arms, ..
        } => arms.iter_mut().fold(
            infer_expression(scrutinee, context, environment),
            |summary, arm| {
                let mut arm_environment = environment.clone();
                summary.union(infer_statements(
                    &mut arm.body,
                    context,
                    &mut arm_environment,
                ))
            },
        ),
        HirStatementKind::ErrorMatch {
            scrutinee, arms, ..
        } => arms.iter_mut().fold(
            infer_expression(scrutinee, context, environment),
            |summary, arm| {
                let mut arm_environment = environment.clone();
                summary.union(infer_statements(
                    &mut arm.body,
                    context,
                    &mut arm_environment,
                ))
            },
        ),
        HirStatementKind::ResultMatch {
            scrutinee, arms, ..
        } => arms.iter_mut().fold(
            infer_expression(scrutinee, context, environment),
            |summary, arm| {
                let mut arm_environment = environment.clone();
                summary.union(infer_statements(
                    &mut arm.body,
                    context,
                    &mut arm_environment,
                ))
            },
        ),
        HirStatementKind::CodecErrorMatch { scrutinee, arms } => arms.iter_mut().fold(
            infer_expression(scrutinee, context, environment),
            |summary, arm| {
                let mut arm_environment = environment.clone();
                summary.union(infer_statements(
                    &mut arm.body,
                    context,
                    &mut arm_environment,
                ))
            },
        ),
        HirStatementKind::Defer { body } | HirStatementKind::AsyncDefer { body } => {
            let mut body_environment = environment.clone();
            infer_statements(body, context, &mut body_environment)
        }
        HirStatementKind::FieldSet { base, value, .. } => {
            infer_expression(base, context, environment)
                .union(infer_expression(value, context, environment))
                .union(managed_write_effect(value.type_id, context.arena))
        }
        HirStatementKind::CompoundFieldSet {
            base,
            value,
            value_type,
            operator,
            ..
        } => infer_expression(base, context, environment)
            .union(infer_expression(value, context, environment))
            .union(compound_effect(*operator, *value_type, context.arena))
            .union(managed_write_effect(*value_type, context.arena)),
        HirStatementKind::ArraySet {
            array,
            index,
            value,
        }
        | HirStatementKind::ListSet {
            list: array,
            index,
            value,
        } => infer_expression(array, context, environment)
            .union(infer_expression(index, context, environment))
            .union(infer_expression(value, context, environment))
            .with(Effect::MayTrap)
            .union(managed_write_effect(value.type_id, context.arena)),
        HirStatementKind::TableSet { table, key, value } => {
            infer_expression(table, context, environment)
                .union(infer_expression(key, context, environment))
                .union(infer_expression(value, context, environment))
                .union(allocating_effects())
                .union(managed_write_effect(key.type_id, context.arena))
                .union(managed_write_effect(value.type_id, context.arena))
        }
        HirStatementKind::CompoundArraySet {
            array,
            index,
            element_type,
            operator,
            value,
        } => infer_expression(array, context, environment)
            .union(infer_expression(index, context, environment))
            .union(infer_expression(value, context, environment))
            .with(Effect::MayTrap)
            .union(compound_effect(*operator, *element_type, context.arena))
            .union(managed_write_effect(*element_type, context.arena)),
        HirStatementKind::MultipleAssignment { targets, value } => targets.iter_mut().fold(
            infer_expression(value, context, environment),
            |summary, target| summary.union(infer_assignment_target(target, context, environment)),
        ),
        HirStatementKind::Call(call) => infer_call(
            &mut call.dispatch,
            call.is_async,
            &mut call.arguments,
            context,
            environment,
        ),
        HirStatementKind::Expression(expression) => {
            infer_expression(expression, context, environment)
        }
        HirStatementKind::Break | HirStatementKind::Continue => EffectSummary::empty(),
    }
}

fn infer_assignment_target(
    target: &mut HirAssignmentTarget,
    context: &EffectContext<'_>,
    environment: &CallableEnvironment,
) -> EffectSummary {
    match target {
        HirAssignmentTarget::Local { .. } => EffectSummary::empty(),
        HirAssignmentTarget::Capture { value_type, .. } => {
            managed_write_effect(*value_type, context.arena)
        }
        HirAssignmentTarget::Field {
            base, value_type, ..
        } => infer_expression(base, context, environment)
            .union(managed_write_effect(*value_type, context.arena)),
        HirAssignmentTarget::Array {
            array,
            index,
            element_type,
        }
        | HirAssignmentTarget::List {
            list: array,
            index,
            element_type,
        } => infer_expression(array, context, environment)
            .union(infer_expression(index, context, environment))
            .with(Effect::MayTrap)
            .union(managed_write_effect(*element_type, context.arena)),
        HirAssignmentTarget::Table {
            table,
            key,
            value_type,
        } => infer_expression(table, context, environment)
            .union(infer_expression(key, context, environment))
            .union(allocating_effects())
            .union(managed_write_effect(key.type_id, context.arena))
            .union(managed_write_effect(*value_type, context.arena)),
    }
}

#[allow(clippy::too_many_lines)]
fn infer_expression(
    expression: &mut HirExpression,
    context: &EffectContext<'_>,
    environment: &CallableEnvironment,
) -> EffectSummary {
    match &mut expression.kind {
        HirExpressionKind::Closure(closure) => {
            let mut nested_environment = CallableEnvironment::default();
            for capture in &closure.captures {
                let effects = match capture.source {
                    HirCaptureSource::Local(local) => environment.locals.get(&local).copied(),
                    HirCaptureSource::Capture(capture) => {
                        environment.captures.get(&capture).copied()
                    }
                    HirCaptureSource::Parameter(_) => {
                        function_type_effects(capture.type_id, context.arena)
                    }
                };
                if let Some(effects) = effects {
                    nested_environment.captures.insert(capture.capture, effects);
                }
            }
            closure.effects = infer_statements(&mut closure.body, context, &mut nested_environment);
            allocating_effects()
        }
        HirExpressionKind::Function(_)
        | HirExpressionKind::Tuple(_)
        | HirExpressionKind::Array(_)
        | HirExpressionKind::Table(_)
        | HirExpressionKind::ClassConstruct { .. } => {
            infer_expression_children(expression, context, environment).union(allocating_effects())
        }
        HirExpressionKind::StringConcat { .. } | HirExpressionKind::StringFormat { .. } => {
            infer_expression_children(expression, context, environment).union(allocating_effects())
        }
        HirExpressionKind::ArrayCreate { .. }
        | HirExpressionKind::ListCreate { .. }
        | HirExpressionKind::RangeCreate { .. } => {
            infer_expression_children(expression, context, environment)
                .union(allocating_effects())
                .with(Effect::MayTrap)
        }
        HirExpressionKind::ListAdd { list, value } => infer_expression(list, context, environment)
            .union(infer_expression(value, context, environment))
            .union(allocating_effects())
            .union(managed_write_effect(value.type_id, context.arena)),
        HirExpressionKind::ArrayGetChecked { .. }
        | HirExpressionKind::ListGetChecked { .. }
        | HirExpressionKind::FfiBufferLength { .. }
        | HirExpressionKind::FfiBufferRead { .. }
        | HirExpressionKind::FfiBufferWrite { .. }
        | HirExpressionKind::FfiBufferClose { .. }
        | HirExpressionKind::FfiPointerRequire { .. } => {
            infer_expression_children(expression, context, environment).with(Effect::MayTrap)
        }
        HirExpressionKind::Unary { operator, operand } => {
            let effects = infer_expression(operand, context, environment);
            if *operator == TypedUnaryOperator::Negate
                && is_integer(expression.type_id, context.arena)
            {
                effects.with(Effect::MayTrap)
            } else {
                effects
            }
        }
        HirExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            let effects = infer_expression(left, context, environment).union(infer_expression(
                right,
                context,
                environment,
            ));
            if matches!(
                operator,
                TypedBinaryOperator::Add
                    | TypedBinaryOperator::Subtract
                    | TypedBinaryOperator::Multiply
                    | TypedBinaryOperator::Divide
                    | TypedBinaryOperator::Remainder
            ) && is_integer(expression.type_id, context.arena)
            {
                effects.with(Effect::MayTrap)
            } else {
                effects
            }
        }
        HirExpressionKind::NumericConvert { conversion, value } => {
            let effects = infer_expression(value, context, environment);
            if conversion.may_trap() {
                effects.with(Effect::MayTrap)
            } else {
                effects
            }
        }
        HirExpressionKind::Await { .. } => {
            infer_expression_children(expression, context, environment).union(suspending_effects())
        }
        HirExpressionKind::TaskCancellationSource => allocating_effects(),
        HirExpressionKind::TaskCancel { .. } => {
            infer_expression_children(expression, context, environment).with(Effect::Synchronizes)
        }
        HirExpressionKind::TaskGroup { .. } => {
            infer_expression_children(expression, context, environment)
                .union(allocating_effects())
                .with(Effect::Synchronizes)
        }
        HirExpressionKind::TaskStart { .. } => {
            infer_expression_children(expression, context, environment)
                .with(Effect::Synchronizes)
                .with(Effect::MayUnwind)
                .with(Effect::GcSafePoint)
        }
        HirExpressionKind::FfiHandleOpen { .. }
        | HirExpressionKind::FfiHandleGet { .. }
        | HirExpressionKind::FfiHandleClose { .. } => {
            infer_expression_children(expression, context, environment)
                .with(Effect::MayTrap)
                .with(Effect::Roots)
        }
        HirExpressionKind::FfiBufferOpen { .. } => {
            infer_expression_children(expression, context, environment)
                .with(Effect::Allocates)
                .with(Effect::MayTrap)
                .with(Effect::GcSafePoint)
                .with(Effect::Roots)
        }
        HirExpressionKind::FfiBufferWithPointer { buffer, body, .. } => {
            let children = infer_expression(buffer, context, environment);
            let mut nested = environment.clone();
            body.effects = infer_statements(&mut body.body, context, &mut nested);
            children
                .union(body.effects)
                .with(Effect::MayTrap)
                .with(Effect::Roots)
        }
        HirExpressionKind::FfiBytesWithPin { bytes, body, .. } => {
            let children = infer_expression(bytes, context, environment);
            let mut nested = environment.clone();
            body.effects = infer_statements(&mut body.body, context, &mut nested);
            children
                .union(body.effects)
                .with(Effect::MayTrap)
                .with(Effect::Roots)
        }
        HirExpressionKind::FfiWithCallback { callback, body, .. } => {
            let mut callback_environment = environment.clone();
            callback.effects =
                infer_statements(&mut callback.body, context, &mut callback_environment);
            let mut body_environment = environment.clone();
            body.effects = infer_statements(&mut body.body, context, &mut body_environment);
            callback.effects.union(body.effects).union(
                allocating_effects()
                    .with(Effect::MayTrap)
                    .with(Effect::Roots),
            )
        }
        HirExpressionKind::FfiCallbackOpen { callback, .. } => {
            let mut callback_environment = environment.clone();
            callback.effects =
                infer_statements(&mut callback.body, context, &mut callback_environment);
            callback.effects.union(
                allocating_effects()
                    .with(Effect::MayTrap)
                    .with(Effect::Roots),
            )
        }
        HirExpressionKind::FfiCallbackWithPair { callback, body, .. } => {
            let children = infer_expression(callback, context, environment);
            let mut body_environment = environment.clone();
            body.effects = infer_statements(&mut body.body, context, &mut body_environment);
            children.union(body.effects)
        }
        HirExpressionKind::FfiCallbackClose { .. } => {
            infer_expression_children(expression, context, environment)
                .with(Effect::MayTrap)
                .with(Effect::Roots)
        }
        HirExpressionKind::FfiUnsafeLoad { .. }
        | HirExpressionKind::FfiUnsafeStore { .. }
        | HirExpressionKind::FfiUnsafeAdvance { .. }
        | HirExpressionKind::FfiUnsafeCopy { .. } => {
            infer_expression_children(expression, context, environment)
                .with(Effect::UnsafeMemory)
                .with(Effect::MayTrap)
        }
        HirExpressionKind::FfiUnsafeAddress { .. }
        | HirExpressionKind::FfiUnsafePointerFromAddress { .. } => {
            infer_expression_children(expression, context, environment).with(Effect::UnsafeMemory)
        }
        HirExpressionKind::Call {
            dispatch,
            is_async,
            arguments,
            ..
        } => infer_call(dispatch, *is_async, arguments, context, environment),
        _ => infer_expression_children(expression, context, environment),
    }
}

fn infer_call(
    dispatch: &mut HirCallDispatch,
    is_async: bool,
    arguments: &mut [HirExpression],
    context: &EffectContext<'_>,
    environment: &CallableEnvironment,
) -> EffectSummary {
    let mut effects = infer_expressions(arguments, context, environment);
    let callee_effects = match dispatch {
        HirCallDispatch::Standard { .. } => EffectSummary::empty().with(Effect::AmbientIo),
        HirCallDispatch::Direct { function } => {
            context.functions.get(function).copied().unwrap_or_default()
        }
        HirCallDispatch::Referenced { function } => context
            .referenced
            .get(function)
            .copied()
            .unwrap_or_default(),
        HirCallDispatch::DirectMethod { method } => {
            context.methods.get(method).copied().unwrap_or_default()
        }
        HirCallDispatch::Indirect { callee } => {
            effects = effects.union(infer_expression(callee, context, environment));
            callable_effects(callee, context, environment).unwrap_or_default()
        }
        HirCallDispatch::InterfaceMethod {
            interface,
            method,
            effects,
            ..
        } => {
            *effects = context
                .interface_methods
                .get(&(*interface, *method))
                .copied()
                .unwrap_or_default();
            *effects
        }
        HirCallDispatch::BuiltinInterfaceMethod {
            interface,
            method,
            effects,
        } => {
            *effects = context
                .builtin_interface_methods
                .get(&(*interface, *method))
                .copied()
                .unwrap_or_default();
            *effects
        }
    };
    if is_async {
        effects.union(allocating_effects())
    } else {
        effects.union(callee_effects)
    }
}

fn callable_effects(
    expression: &HirExpression,
    context: &EffectContext<'_>,
    environment: &CallableEnvironment,
) -> Option<EffectSummary> {
    match &expression.kind {
        HirExpressionKind::Closure(closure) => Some(closure.effects),
        HirExpressionKind::Function(function) => context.functions.get(function).copied(),
        HirExpressionKind::Local(local) => environment.locals.get(local).copied(),
        HirExpressionKind::Capture(capture) => environment.captures.get(capture).copied(),
        _ => function_type_effects(expression.type_id, context.arena),
    }
}

fn function_type_effects(type_id: TypeId, arena: &TypeArena) -> Option<EffectSummary> {
    match arena.get(type_id) {
        Some(SemanticType::Function { effects, .. }) => Some(*effects),
        _ => None,
    }
}

fn infer_expressions(
    expressions: &mut [HirExpression],
    context: &EffectContext<'_>,
    environment: &CallableEnvironment,
) -> EffectSummary {
    expressions
        .iter_mut()
        .fold(EffectSummary::empty(), |summary, expression| {
            summary.union(infer_expression(expression, context, environment))
        })
}

// Recurses through expression children that do not define their own call edge.
fn infer_expression_children(
    expression: &mut HirExpression,
    context: &EffectContext<'_>,
    environment: &CallableEnvironment,
) -> EffectSummary {
    let mut summary = EffectSummary::empty();
    visit_expression_children_mut(expression, |child| {
        summary = summary.union(infer_expression(child, context, environment));
    });
    summary
}

fn allocating_effects() -> EffectSummary {
    EffectSummary::empty()
        .with(Effect::Allocates)
        .with(Effect::MayUnwind)
        .with(Effect::GcSafePoint)
}

fn suspending_effects() -> EffectSummary {
    EffectSummary::empty()
        .with(Effect::Suspends)
        .with(Effect::MayUnwind)
        .with(Effect::GcSafePoint)
}

fn managed_write_effect(type_id: TypeId, arena: &TypeArena) -> EffectSummary {
    if is_managed_reference(type_id, arena) {
        EffectSummary::empty().with(Effect::WritesManagedReference)
    } else {
        EffectSummary::empty()
    }
}

fn is_managed_reference(type_id: TypeId, arena: &TypeArena) -> bool {
    match arena.get(type_id) {
        Some(SemanticType::Builtin { definition, .. }) => {
            !pop_types::is_ffi_abi_builtin_type(*definition)
        }
        Some(
            SemanticType::Primitive(PrimitiveType::String)
            | SemanticType::Tuple(_)
            | SemanticType::Array(_)
            | SemanticType::Table { .. }
            | SemanticType::Class { .. }
            | SemanticType::Interface { .. }
            | SemanticType::Function { .. }
            | SemanticType::ErrorUnion { .. },
        ) => true,
        _ => false,
    }
}

fn is_integer(type_id: TypeId, arena: &TypeArena) -> bool {
    matches!(
        arena.get(type_id),
        Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
    )
}

fn compound_effect(
    operator: TypedCompoundOperator,
    type_id: TypeId,
    arena: &TypeArena,
) -> EffectSummary {
    if operator == TypedCompoundOperator::Concat {
        allocating_effects()
    } else if is_integer(type_id, arena) {
        EffectSummary::empty().with(Effect::MayTrap)
    } else {
        EffectSummary::empty()
    }
}

fn rewrite_statement_types(
    statements: &mut [HirStatement],
    functions: &BTreeMap<SymbolId, EffectSummary>,
    arena: &mut TypeArena,
    locals: &mut BTreeMap<LocalId, TypeId>,
) {
    for statement in statements {
        rewrite_statement_type(statement, functions, arena, locals);
    }
}

#[allow(clippy::too_many_lines)]
fn rewrite_statement_type(
    statement: &mut HirStatement,
    functions: &BTreeMap<SymbolId, EffectSummary>,
    arena: &mut TypeArena,
    locals: &mut BTreeMap<LocalId, TypeId>,
) {
    match &mut statement.kind {
        HirStatementKind::Local {
            local,
            local_type,
            initializer,
            ..
        } => {
            rewrite_expression_type(initializer, functions, arena, locals);
            if function_type_effects(initializer.type_id, arena).is_some() {
                *local_type = initializer.type_id;
                locals.insert(*local, initializer.type_id);
            }
        }
        HirStatementKind::LocalSet { local, value } => {
            rewrite_expression_type(value, functions, arena, locals);
            if function_type_effects(value.type_id, arena).is_some() {
                locals.insert(*local, value.type_id);
            }
        }
        HirStatementKind::MultipleLocal { value, .. }
        | HirStatementKind::ParameterSet { value, .. }
        | HirStatementKind::CaptureSet { value, .. }
        | HirStatementKind::Expression(value) => {
            rewrite_expression_type(value, functions, arena, locals);
        }
        HirStatementKind::Return { values } => {
            rewrite_expressions(values, functions, arena, locals);
        }
        HirStatementKind::If {
            condition,
            then_body,
            else_body,
        }
        | HirStatementKind::OptionalIf {
            initializer: condition,
            then_body,
            else_body,
            ..
        } => {
            rewrite_expression_type(condition, functions, arena, locals);
            rewrite_statement_types(then_body, functions, arena, &mut locals.clone());
            rewrite_statement_types(else_body, functions, arena, &mut locals.clone());
        }
        HirStatementKind::While { condition, body }
        | HirStatementKind::OptionalWhile {
            initializer: condition,
            body,
            ..
        } => {
            rewrite_expression_type(condition, functions, arena, locals);
            rewrite_statement_types(body, functions, arena, &mut locals.clone());
        }
        HirStatementKind::RepeatUntil { body, condition } => {
            rewrite_statement_types(body, functions, arena, &mut locals.clone());
            rewrite_expression_type(condition, functions, arena, locals);
        }
        HirStatementKind::NumericFor {
            first,
            last,
            step,
            body,
            ..
        } => {
            rewrite_expression_type(first, functions, arena, locals);
            rewrite_expression_type(last, functions, arena, locals);
            rewrite_expression_type(step, functions, arena, locals);
            rewrite_statement_types(body, functions, arena, &mut locals.clone());
        }
        HirStatementKind::GeneralizedFor { iterable, body, .. } => {
            rewrite_expression_type(iterable, functions, arena, locals);
            rewrite_statement_types(body, functions, arena, &mut locals.clone());
        }
        HirStatementKind::Match {
            scrutinee, arms, ..
        } => {
            rewrite_expression_type(scrutinee, functions, arena, locals);
            for arm in arms {
                rewrite_statement_types(&mut arm.body, functions, arena, &mut locals.clone());
            }
        }
        HirStatementKind::ErrorMatch {
            scrutinee, arms, ..
        } => {
            rewrite_expression_type(scrutinee, functions, arena, locals);
            for arm in arms {
                rewrite_statement_types(&mut arm.body, functions, arena, &mut locals.clone());
            }
        }
        HirStatementKind::ResultMatch {
            scrutinee, arms, ..
        } => {
            rewrite_expression_type(scrutinee, functions, arena, locals);
            for arm in arms {
                rewrite_statement_types(&mut arm.body, functions, arena, &mut locals.clone());
            }
        }
        HirStatementKind::CodecErrorMatch { scrutinee, arms } => {
            rewrite_expression_type(scrutinee, functions, arena, locals);
            for arm in arms {
                rewrite_statement_types(&mut arm.body, functions, arena, &mut locals.clone());
            }
        }
        HirStatementKind::Defer { body } | HirStatementKind::AsyncDefer { body } => {
            rewrite_statement_types(body, functions, arena, &mut locals.clone());
        }
        HirStatementKind::FieldSet { base, value, .. }
        | HirStatementKind::CompoundFieldSet { base, value, .. } => {
            rewrite_expression_type(base, functions, arena, locals);
            rewrite_expression_type(value, functions, arena, locals);
        }
        HirStatementKind::ArraySet {
            array,
            index,
            value,
        }
        | HirStatementKind::ListSet {
            list: array,
            index,
            value,
        }
        | HirStatementKind::CompoundArraySet {
            array,
            index,
            value,
            ..
        } => {
            rewrite_expression_type(array, functions, arena, locals);
            rewrite_expression_type(index, functions, arena, locals);
            rewrite_expression_type(value, functions, arena, locals);
        }
        HirStatementKind::TableSet { table, key, value } => {
            rewrite_expression_type(table, functions, arena, locals);
            rewrite_expression_type(key, functions, arena, locals);
            rewrite_expression_type(value, functions, arena, locals);
        }
        HirStatementKind::MultipleAssignment { targets, value } => {
            rewrite_expression_type(value, functions, arena, locals);
            for target in targets {
                rewrite_assignment_target_type(target, functions, arena, locals);
            }
        }
        HirStatementKind::Call(call) => {
            if let HirCallDispatch::Indirect { callee } = &mut call.dispatch {
                rewrite_expression_type(callee, functions, arena, locals);
            }
            rewrite_expressions(&mut call.arguments, functions, arena, locals);
        }
        HirStatementKind::Break | HirStatementKind::Continue => {}
    }
}

fn rewrite_assignment_target_type(
    target: &mut HirAssignmentTarget,
    functions: &BTreeMap<SymbolId, EffectSummary>,
    arena: &mut TypeArena,
    locals: &BTreeMap<LocalId, TypeId>,
) {
    match target {
        HirAssignmentTarget::Field { base, .. } => {
            rewrite_expression_type(base, functions, arena, locals)
        }
        HirAssignmentTarget::Array { array, index, .. }
        | HirAssignmentTarget::List {
            list: array, index, ..
        } => {
            rewrite_expression_type(array, functions, arena, locals);
            rewrite_expression_type(index, functions, arena, locals);
        }
        HirAssignmentTarget::Table { table, key, .. } => {
            rewrite_expression_type(table, functions, arena, locals);
            rewrite_expression_type(key, functions, arena, locals);
        }
        HirAssignmentTarget::Local { .. } | HirAssignmentTarget::Capture { .. } => {}
    }
}

fn rewrite_expression_type(
    expression: &mut HirExpression,
    functions: &BTreeMap<SymbolId, EffectSummary>,
    arena: &mut TypeArena,
    locals: &BTreeMap<LocalId, TypeId>,
) {
    visit_expression_children_mut(expression, |child| {
        rewrite_expression_type(child, functions, arena, locals);
    });
    let effects = match &mut expression.kind {
        HirExpressionKind::Closure(closure) => {
            let mut nested_locals = BTreeMap::new();
            for capture in &mut closure.captures {
                if let HirCaptureSource::Local(local) = capture.source
                    && let Some(type_id) = locals.get(&local).copied()
                {
                    capture.type_id = type_id;
                }
            }
            rewrite_statement_types(&mut closure.body, functions, arena, &mut nested_locals);
            Some(closure.effects)
        }
        HirExpressionKind::Function(function) => functions.get(function).copied(),
        HirExpressionKind::Local(local) => {
            if let Some(type_id) = locals.get(local).copied() {
                expression.type_id = type_id;
            }
            None
        }
        _ => None,
    };
    if let Some(effects) = effects
        && let Some(SemanticType::Function {
            is_async,
            parameters,
            results,
            lifetime_summary,
            ..
        }) = arena.get(expression.type_id).cloned()
        && let Ok(type_id) = arena.intern(SemanticType::Function {
            is_async,
            parameters,
            results,
            effects,
            lifetime_summary,
        })
    {
        expression.type_id = type_id;
    }
}

fn rewrite_expressions(
    expressions: &mut [HirExpression],
    functions: &BTreeMap<SymbolId, EffectSummary>,
    arena: &mut TypeArena,
    locals: &BTreeMap<LocalId, TypeId>,
) {
    for expression in expressions {
        rewrite_expression_type(expression, functions, arena, locals);
    }
}

// Keeps child traversal in one place so new expression forms cannot silently
// skip operand effects or callable-type closure.
#[allow(clippy::too_many_lines)]
fn visit_expression_children_mut(
    expression: &mut HirExpression,
    mut visit: impl FnMut(&mut HirExpression),
) {
    match &mut expression.kind {
        HirExpressionKind::Field { base, .. }
        | HirExpressionKind::ArrayLength { array: base }
        | HirExpressionKind::ListLength { list: base }
        | HirExpressionKind::OptionalNarrow { optional: base }
        | HirExpressionKind::Await { task: base }
        | HirExpressionKind::TaskCancelToken { source: base }
        | HirExpressionKind::TaskCancel { source: base }
        | HirExpressionKind::FfiHandleOpen { value: base }
        | HirExpressionKind::FfiHandleGet { handle: base }
        | HirExpressionKind::FfiHandleClose { handle: base }
        | HirExpressionKind::FfiBufferLength { buffer: base }
        | HirExpressionKind::FfiBufferClose { buffer: base }
        | HirExpressionKind::FfiCallbackClose { callback: base, .. }
        | HirExpressionKind::FfiPointerToOptional { pointer: base }
        | HirExpressionKind::FfiPointerReadOnly { pointer: base }
        | HirExpressionKind::FfiPointerIsPresent { pointer: base }
        | HirExpressionKind::FfiPointerRequire { pointer: base, .. }
        | HirExpressionKind::FfiUnsafeLoad { pointer: base, .. }
        | HirExpressionKind::FfiUnsafeAddress { pointer: base, .. }
        | HirExpressionKind::FfiUnsafePointerFromAddress { address: base, .. }
        | HirExpressionKind::InterfaceUpcast { value: base, .. }
        | HirExpressionKind::CheckedNominalCast { value: base, .. }
        | HirExpressionKind::NumericConvert { value: base, .. }
        | HirExpressionKind::StringFormat { value: base, .. }
        | HirExpressionKind::Unary { operand: base, .. }
        | HirExpressionKind::OptionalPropagate { optional: base, .. }
        | HirExpressionKind::ResultPropagate { result: base, .. } => visit(base),
        HirExpressionKind::ViewCreate { lender: base, .. }
        | HirExpressionKind::ViewLength { view: base, .. }
        | HirExpressionKind::ViewMaterialize { view: base, .. } => visit(base),
        HirExpressionKind::ArrayGet { array, index }
        | HirExpressionKind::ArrayGetChecked { array, index }
        | HirExpressionKind::ListGet { list: array, index }
        | HirExpressionKind::ListGetChecked { list: array, index }
        | HirExpressionKind::ViewGetByte { view: array, index }
        | HirExpressionKind::TableGet {
            table: array,
            key: index,
        }
        | HirExpressionKind::StringConcat {
            left: array,
            right: index,
        }
        | HirExpressionKind::Binary {
            left: array,
            right: index,
            ..
        }
        | HirExpressionKind::OptionalDefault {
            optional: array,
            fallback: index,
        } => {
            visit(array);
            visit(index);
        }
        HirExpressionKind::ViewSlice {
            view,
            start,
            length,
            ..
        } => {
            visit(view);
            visit(start);
            visit(length);
        }
        HirExpressionKind::TupleGet { tuple, .. } => visit(tuple),
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        }
        | HirExpressionKind::ArrayFill {
            array: length,
            value: initial_value,
        }
        | HirExpressionKind::ListAdd {
            list: length,
            value: initial_value,
        }
        | HirExpressionKind::TaskGroup {
            cancel: length,
            body: initial_value,
        }
        | HirExpressionKind::TaskStart {
            group: length,
            task: initial_value,
        } => {
            visit(length);
            visit(initial_value);
        }
        HirExpressionKind::ListCreate { capacity } => {
            if let Some(capacity) = capacity {
                visit(capacity);
            }
        }
        HirExpressionKind::RangeCreate { first, last, step } => {
            visit(first);
            visit(last);
            visit(step);
        }
        HirExpressionKind::Record { fields, .. }
        | HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                visit(&mut field.value);
            }
        }
        HirExpressionKind::RecordUpdate { base, fields, .. } => {
            visit(base);
            for field in fields {
                visit(&mut field.value);
            }
        }
        HirExpressionKind::Array(values)
        | HirExpressionKind::Tuple(values)
        | HirExpressionKind::UnionCase {
            arguments: values, ..
        }
        | HirExpressionKind::ResultCase {
            arguments: values, ..
        }
        | HirExpressionKind::IterationCase {
            arguments: values, ..
        }
        | HirExpressionKind::ErrorCase {
            arguments: values, ..
        } => {
            for value in values {
                visit(value);
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                visit(&mut entry.key);
                visit(&mut entry.value);
            }
        }
        HirExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            visit(condition);
            visit(when_true);
            visit(when_false);
        }
        HirExpressionKind::FfiBufferOpen { length, .. } => visit(length),
        HirExpressionKind::FfiBufferRead { buffer, index } => {
            visit(buffer);
            visit(index);
        }
        HirExpressionKind::FfiBufferWrite {
            buffer,
            index,
            value,
        } => {
            visit(buffer);
            visit(index);
            visit(value);
        }
        HirExpressionKind::FfiBufferWithPointer { buffer, .. } => visit(buffer),
        HirExpressionKind::FfiBytesWithPin { bytes, .. } => visit(bytes),
        HirExpressionKind::FfiCallbackWithPair { callback, .. } => visit(callback),
        HirExpressionKind::FfiUnsafeStore { pointer, value, .. } => {
            visit(pointer);
            visit(value);
        }
        HirExpressionKind::FfiUnsafeAdvance {
            pointer, elements, ..
        } => {
            visit(pointer);
            visit(elements);
        }
        HirExpressionKind::FfiUnsafeCopy {
            source,
            destination,
            count,
            ..
        } => {
            visit(source);
            visit(destination);
            visit(count);
        }
        HirExpressionKind::Call {
            dispatch,
            arguments,
            ..
        } => {
            if let HirCallDispatch::Indirect { callee } = dispatch {
                visit(callee);
            }
            for argument in arguments {
                visit(argument);
            }
        }
        HirExpressionKind::Closure(_)
        | HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::GeneratedCodecSchema(_)
        | HirExpressionKind::TaskCancellationSource
        | HirExpressionKind::FfiWithCallback { .. }
        | HirExpressionKind::FfiCallbackOpen { .. }
        | HirExpressionKind::FfiPointerNone { .. }
        | HirExpressionKind::EnumCase { .. }
        | HirExpressionKind::CodecErrorCase(_) => {}
    }
}
