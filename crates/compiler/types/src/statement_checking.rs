//! Statement, lexical-scope, closure-shape, and exhaustive-match checking.
//!
//! This module owns control-flow-shaped body mechanics. Expression typing and
//! call/aggregate/operator selection live in their respective checker modules.

use std::collections::BTreeMap;

use pop_diagnostics::types as type_diagnostics;
use pop_foundation::{
    BindingId, LocalId, NestedFunctionId, SourceSpan, TextRange, TextSize, ValueParameterId,
};
use pop_syntax::{
    BinaryOperator as SyntaxBinaryOperator, CaptureFunctionSyntax, ExpressionSyntax,
    ExpressionSyntaxKind, MatchArmSyntax, StatementSyntax, StatementSyntaxKind,
};

use crate::body_checking::{
    ActiveFunction, Binding, BindingKind, BodyChecker, CheckedInvocation, ErrorCaseLookup,
    ExpectedExpressionType, ResolvedClosureShape, UnionCaseLookup, missing_match_arms,
    statements_definitely_return,
};
use crate::typed_body::*;
use crate::{ResolvedFunctionSignature, SemanticType};

impl<'resolver, 'index> BodyChecker<'resolver, 'index> {
    pub(crate) fn check_statement(
        &mut self,
        signature: &ResolvedFunctionSignature,
        statement: &StatementSyntax,
    ) -> Option<TypedStatement> {
        let kind = match statement.kind() {
            StatementSyntaxKind::Local {
                name,
                annotation,
                initializer,
            } => self.check_local(signature, name, annotation.as_ref(), initializer)?,
            StatementSyntaxKind::MultipleLocal { bindings, values } => {
                self.check_multiple_local(signature, bindings, values, statement.span())?
            }
            StatementSyntaxKind::LocalFunction { name, function } => {
                self.check_local_function(signature, name, function)?
            }
            StatementSyntaxKind::Return { values } => {
                if signature.results().len() == 1
                    && let Some(result_type) = signature.results()[0].type_id()
                    && let Some(SemanticType::Tuple(elements)) =
                        self.resolver.arena().get(result_type).cloned()
                {
                    let value = self.check_fixed_pack(
                        values,
                        Some((&elements, result_type)),
                        elements.len(),
                        statement.span(),
                        "return",
                    )?;
                    return Some(TypedStatement {
                        kind: TypedStatementKind::Return {
                            values: vec![value],
                        },
                        span: statement.span(),
                    });
                }
                if signature.results().len() != values.len() {
                    self.diagnostics.push(type_diagnostics::wrong_value_arity(
                        statement.span(),
                        "return",
                        signature.results().len(),
                        values.len(),
                    ));
                    return None;
                }
                let mut typed_values = Vec::new();
                for (value, expected) in values.iter().zip(signature.results()) {
                    let typed = self.check_expression_expected(
                        value,
                        ExpectedExpressionType::resolved(expected),
                    )?;
                    if let Some(expected_id) = expected.type_id() {
                        self.require_same_type(
                            expected_id,
                            typed.type_id(),
                            typed.span(),
                            expected.span(),
                        );
                    }
                    typed_values.push(typed);
                }
                TypedStatementKind::Return {
                    values: typed_values,
                }
            }
            StatementSyntaxKind::If {
                condition,
                then_body,
                else_body,
            } => {
                let narrowing = self.optional_narrowing(condition);
                let condition = self.check_condition(condition)?;
                let then_body = if let Some((binding, inner, true)) = narrowing {
                    self.check_nested_statements_with_narrowing(
                        signature, then_body, binding, inner,
                    )
                } else {
                    self.check_nested_statements(signature, then_body)
                };
                let else_body = if let Some((binding, inner, false)) = narrowing {
                    self.check_nested_statements_with_narrowing(
                        signature, else_body, binding, inner,
                    )
                } else {
                    self.check_nested_statements(signature, else_body)
                };
                TypedStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            StatementSyntaxKind::OptionalIf {
                name,
                initializer,
                then_body,
                else_body,
            } => self.check_optional_if(
                signature,
                name,
                initializer,
                then_body,
                else_body,
                statement.span(),
            )?,
            StatementSyntaxKind::While { condition, body } => {
                let condition = self.check_condition(condition)?;
                self.loop_depth = self.loop_depth.saturating_add(1);
                let body = self.check_nested_statements(signature, body);
                self.loop_depth = self.loop_depth.saturating_sub(1);
                TypedStatementKind::While { condition, body }
            }
            StatementSyntaxKind::OptionalWhile {
                name,
                initializer,
                body,
            } => self.check_optional_while(signature, name, initializer, body, statement.span())?,
            StatementSyntaxKind::RepeatUntil { body, condition } => {
                self.check_repeat_until(signature, body, condition)?
            }
            StatementSyntaxKind::NumericFor {
                name,
                first,
                last,
                step,
                body,
            } => self.check_numeric_for(
                signature,
                name,
                first,
                last,
                step.as_ref(),
                body,
                statement.span(),
            )?,
            StatementSyntaxKind::GeneralizedFor {
                bindings,
                iterable,
                body,
            } => {
                self.check_generalized_for(signature, bindings, iterable, body, statement.span())?
            }
            StatementSyntaxKind::Break => {
                self.check_loop_control("break", true, statement.span())?
            }
            StatementSyntaxKind::Continue => {
                self.check_loop_control("continue", false, statement.span())?
            }
            StatementSyntaxKind::Match { scrutinee, arms } => {
                self.check_match(signature, scrutinee, arms, statement.span())?
            }
            StatementSyntaxKind::Defer { body } => {
                if let Some((control, control_span)) = illegal_cleanup_control(body) {
                    self.diagnostics
                        .push(type_diagnostics::illegal_cleanup_control(
                            control_span,
                            control,
                        ));
                    return None;
                }
                TypedStatementKind::Defer {
                    body: self.check_nested_statements(signature, body),
                }
            }
            StatementSyntaxKind::AsyncDefer { body } => {
                if !signature.is_async() {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        statement.span(),
                        "async defer",
                        "async function",
                    ));
                    let _ = self.check_nested_statements(signature, body);
                    return None;
                }
                if let Some((control, control_span)) = illegal_cleanup_control(body) {
                    self.diagnostics
                        .push(type_diagnostics::illegal_cleanup_control(
                            control_span,
                            control,
                        ));
                    return None;
                }
                TypedStatementKind::AsyncDefer {
                    body: self.check_nested_statements(signature, body),
                }
            }
            StatementSyntaxKind::Assignment {
                target,
                operator,
                value,
            } => self.check_assignment(target, *operator, value, statement.span())?,
            StatementSyntaxKind::MultipleAssignment { targets, values } => {
                self.check_multiple_assignment(targets, values, statement.span())?
            }
            StatementSyntaxKind::Expression(expression) => {
                self.check_expression_statement(expression)?
            }
        };
        Some(TypedStatement {
            kind,
            span: statement.span(),
        })
    }

    fn check_loop_control(
        &mut self,
        name: &str,
        is_break: bool,
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        if self.loop_depth == 0 {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                name,
                "outside loop",
            ));
            return None;
        }
        Some(if is_break {
            TypedStatementKind::Break
        } else {
            TypedStatementKind::Continue
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn check_numeric_for(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        first: &ExpressionSyntax,
        last: &ExpressionSyntax,
        step: Option<&ExpressionSyntax>,
        body: &[StatementSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let first = self.check_expression(first)?;
        let integer_type = first.type_id();
        let Some(SemanticType::Primitive(crate::PrimitiveType::Integer(kind))) =
            self.resolver.arena().get(integer_type).cloned()
        else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                first.span(),
                "numeric for",
                self.type_name(integer_type),
            ));
            return None;
        };
        let last = self
            .check_expression_expected(last, Some(ExpectedExpressionType::plain(integer_type)))?;
        self.require_same_type(integer_type, last.type_id(), last.span(), span);
        let step = if let Some(step) = step {
            let step = self.check_expression_expected(
                step,
                Some(ExpectedExpressionType::plain(integer_type)),
            )?;
            self.require_same_type(integer_type, step.type_id(), step.span(), span);
            step
        } else {
            TypedExpression {
                kind: TypedExpressionKind::Integer(
                    crate::IntegerValue::parse_decimal("1", kind).expect("one fits every integer"),
                ),
                type_id: integer_type,
                span,
            }
        };
        if matches!(step.kind(), TypedExpressionKind::Integer(value)
            if value.signed() == Some(0) || value.unsigned() == Some(0))
        {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                step.span(),
                "numeric for step",
                "zero",
            ));
            return None;
        }

        let local = LocalId::from_raw(self.next_local);
        self.next_local = self.next_local.saturating_add(1);
        let binding = BindingId::from_raw(self.next_binding);
        self.next_binding = self.next_binding.saturating_add(1);
        self.scopes.push(BTreeMap::new());
        self.scopes
            .last_mut()
            .expect("numeric for scope was just pushed")
            .insert(
                name.to_owned(),
                Binding {
                    id: binding,
                    kind: BindingKind::LoopLocal(local),
                    type_id: integer_type,
                    function_depth: self.function_depth,
                },
            );
        self.loop_depth = self.loop_depth.saturating_add(1);
        let body = body
            .iter()
            .filter_map(|statement| self.check_statement(signature, statement))
            .collect();
        self.loop_depth = self.loop_depth.saturating_sub(1);
        self.scopes
            .pop()
            .expect("numeric for scope was just pushed");
        Some(TypedStatementKind::NumericFor {
            binding,
            local,
            name: name.to_owned(),
            integer_type,
            first,
            last,
            step,
            body,
        })
    }

    fn check_generalized_for(
        &mut self,
        signature: &ResolvedFunctionSignature,
        binding_names: &[String],
        iterable: &ExpressionSyntax,
        body: &[StatementSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let iterable = self.check_expression(iterable)?;
        let active_collection = match iterable.kind() {
            TypedExpressionKind::Local(local) => Some(
                crate::body_checking::ActiveCollectionIteration::Local(*local),
            ),
            TypedExpressionKind::Parameter(parameter) => Some(
                crate::body_checking::ActiveCollectionIteration::Parameter(*parameter),
            ),
            TypedExpressionKind::Capture(capture) => Some(
                crate::body_checking::ActiveCollectionIteration::Capture(*capture),
            ),
            _ => None,
        };
        let iterable_type = iterable.type_id();
        let protocol = self.resolver.schema().iteration_protocol()?;
        let semantic = self.resolver.arena().get(iterable_type)?.clone();
        let (source, item_type) = match semantic {
            SemanticType::Array(element) => (TypedIterationSource::Array, element),
            SemanticType::Table { key, value } => {
                let item = self
                    .resolver
                    .arena_mut()
                    .intern(SemanticType::Tuple(vec![key, value]))
                    .ok()?;
                (TypedIterationSource::Table, item)
            }
            SemanticType::Builtin {
                definition,
                arguments,
            } if arguments.len() == 1 => {
                if definition == protocol.list() {
                    (TypedIterationSource::List, arguments[0])
                } else if definition == protocol.range() && self.is_integer(arguments[0]) {
                    (TypedIterationSource::Range, arguments[0])
                } else if definition == protocol.iterable() {
                    (TypedIterationSource::Iterable, arguments[0])
                } else if definition == protocol.iterator() {
                    (TypedIterationSource::Iterator, arguments[0])
                } else {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        span,
                        "for in",
                        self.type_name(iterable_type),
                    ));
                    return None;
                }
            }
            SemanticType::Class { .. } => {
                let Some(class) = self
                    .resolver
                    .class_definition_for_type(iterable_type)
                    .cloned()
                else {
                    return None;
                };
                let Some(implementation) =
                    class.builtin_interfaces().iter().find(|implementation| {
                        implementation.interface() == protocol.iterable()
                            || implementation.interface() == protocol.iterator()
                    })
                else {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        span,
                        "for in",
                        self.type_name(iterable_type),
                    ));
                    return None;
                };
                let Some(SemanticType::Builtin { arguments, .. }) =
                    self.resolver.arena().get(implementation.interface_type())
                else {
                    return None;
                };
                let [item_type] = arguments.as_slice() else {
                    return None;
                };
                let iterator_method = implementation
                    .methods()
                    .iter()
                    .find(|method| method.protocol_method() == protocol.iterator_method())?
                    .class_method();
                if implementation.interface() == protocol.iterator() {
                    let next_method = implementation
                        .methods()
                        .iter()
                        .find(|method| method.protocol_method() == protocol.next_method())?
                        .class_method();
                    (
                        TypedIterationSource::ClassIterator {
                            iterator_method,
                            next_method,
                        },
                        *item_type,
                    )
                } else {
                    (
                        TypedIterationSource::ClassIterable { iterator_method },
                        *item_type,
                    )
                }
            }
            SemanticType::TypeParameter(_) => {
                let bound = signature
                    .type_parameters()
                    .iter()
                    .find(|parameter| parameter.type_id() == iterable_type)
                    .and_then(crate::ResolvedTypeParameter::bound);
                match bound.and_then(|bound| self.resolver.arena().get(bound)) {
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if arguments.len() == 1 && *definition == protocol.iterable() => {
                        (TypedIterationSource::BoundIterable, arguments[0])
                    }
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if arguments.len() == 1 && *definition == protocol.iterator() => {
                        (TypedIterationSource::BoundIterator, arguments[0])
                    }
                    _ => {
                        self.diagnostics.push(type_diagnostics::invalid_operator(
                            span,
                            "for in",
                            self.type_name(iterable_type),
                        ));
                        return None;
                    }
                }
            }
            _ => {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "for in",
                    self.type_name(iterable_type),
                ));
                return None;
            }
        };

        let binding_types = if binding_names.len() == 1 {
            vec![item_type]
        } else if let Some(SemanticType::Tuple(elements)) =
            self.resolver.arena().get(item_type).cloned()
        {
            if elements.len() != binding_names.len() {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    span,
                    "generalized for bindings",
                    elements.len(),
                    binding_names.len(),
                ));
                return None;
            }
            elements
        } else {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "generalized for bindings",
                1,
                binding_names.len(),
            ));
            return None;
        };
        let iterator_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: protocol.iterator(),
                arguments: vec![item_type],
            })
            .ok()?;
        let iteration_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: protocol.iteration(),
                arguments: vec![item_type],
            })
            .ok()?;

        let mut names = BTreeMap::new();
        for name in binding_names {
            if names.insert(name, span).is_some() {
                self.diagnostics
                    .push(type_diagnostics::duplicate_binding(span, name, span));
                return None;
            }
        }
        self.scopes.push(BTreeMap::new());
        let mut bindings = Vec::with_capacity(binding_names.len());
        for (name, local_type) in binding_names.iter().zip(binding_types) {
            let local = LocalId::from_raw(self.next_local);
            self.next_local = self.next_local.saturating_add(1);
            let binding = BindingId::from_raw(self.next_binding);
            self.next_binding = self.next_binding.saturating_add(1);
            self.scopes
                .last_mut()
                .expect("generalized for scope was just pushed")
                .insert(
                    name.clone(),
                    Binding {
                        id: binding,
                        kind: BindingKind::LoopLocal(local),
                        type_id: local_type,
                        function_depth: self.function_depth,
                    },
                );
            bindings.push(TypedLocalBinding {
                binding,
                local,
                name: name.clone(),
                local_type,
                span,
            });
        }
        self.loop_depth = self.loop_depth.saturating_add(1);
        self.active_collection_iterations.extend(active_collection);
        let body = body
            .iter()
            .filter_map(|statement| self.check_statement(signature, statement))
            .collect();
        if active_collection.is_some() {
            self.active_collection_iterations.pop();
        }
        self.loop_depth = self.loop_depth.saturating_sub(1);
        self.scopes
            .pop()
            .expect("generalized for scope was just pushed");
        Some(TypedStatementKind::GeneralizedFor {
            protocol,
            source,
            item_type,
            iterator_type,
            iteration_type,
            bindings,
            iterable,
            body,
        })
    }

    pub(crate) fn check_expression_statement(
        &mut self,
        expression: &ExpressionSyntax,
    ) -> Option<TypedStatementKind> {
        let invocation = match expression.kind() {
            ExpressionSyntaxKind::Call { callee, arguments } => {
                self.check_call_invocation(callee, arguments, None, expression.span())
            }
            ExpressionSyntaxKind::MethodCall {
                receiver,
                method,
                arguments,
            } => self
                .check_receiver_method_invocation(receiver, method, arguments, expression.span())
                .map(CheckedInvocation::Call),
            _ => {
                return Some(TypedStatementKind::Expression(
                    self.check_expression(expression)?,
                ));
            }
        }?;
        let checked = match invocation {
            CheckedInvocation::Call(checked) => {
                self.invalidate_flow_narrowings();
                checked
            }
            CheckedInvocation::Value(value) => {
                return Some(TypedStatementKind::Expression(value));
            }
        };
        if checked.results.is_empty() {
            return Some(TypedStatementKind::Call(checked.call));
        }
        self.checked_call_expression(checked)
            .map(TypedStatementKind::Expression)
    }

    pub(crate) fn check_local(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        annotation: Option<&pop_syntax::TypeSyntax>,
        initializer: &ExpressionSyntax,
    ) -> Option<TypedStatementKind> {
        let annotation_type = if let Some(annotation) = annotation {
            let (resolved, diagnostics) =
                self.resolver
                    .resolve_annotation(self.module, annotation, signature);
            self.diagnostics.extend(diagnostics);
            Some((
                ExpectedExpressionType::resolved(&resolved?)?,
                annotation.span(),
            ))
        } else {
            None
        };
        let initializer = self.check_expression_expected(
            initializer,
            annotation_type.map(|(expected, _)| expected),
        )?;
        let local_type = if let Some((expected, origin)) = annotation_type {
            self.require_same_type(
                expected.type_id,
                initializer.type_id(),
                initializer.span(),
                origin,
            );
            expected.type_id
        } else {
            initializer.type_id()
        };
        let local = LocalId::from_raw(self.next_local);
        self.next_local = self.next_local.saturating_add(1);
        let binding = BindingId::from_raw(self.next_binding);
        self.next_binding = self.next_binding.saturating_add(1);
        self.scopes
            .last_mut()
            .expect("body checker always has a lexical scope")
            .insert(
                name.to_owned(),
                Binding {
                    id: binding,
                    kind: BindingKind::Local(local),
                    type_id: local_type,
                    function_depth: self.function_depth,
                },
            );
        Some(TypedStatementKind::Local {
            binding,
            local,
            name: name.to_owned(),
            local_type,
            initializer,
        })
    }

    fn check_multiple_local(
        &mut self,
        signature: &ResolvedFunctionSignature,
        bindings: &[pop_syntax::LocalBindingSyntax],
        values: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let value = self.check_fixed_pack(values, None, bindings.len(), span, "multiple local")?;
        let Some(SemanticType::Tuple(element_types)) =
            self.resolver.arena().get(value.type_id()).cloned()
        else {
            return None;
        };
        if bindings.len() != element_types.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "multiple local",
                bindings.len(),
                element_types.len(),
            ));
            return None;
        }

        let mut names = BTreeMap::new();
        for binding in bindings {
            if let Some(original) = names.insert(binding.name(), binding.span()) {
                self.diagnostics.push(type_diagnostics::duplicate_binding(
                    binding.span(),
                    binding.name(),
                    original,
                ));
                return None;
            }
        }
        let mut typed_bindings = Vec::with_capacity(bindings.len());
        for (binding_syntax, inferred) in bindings.iter().zip(element_types) {
            let local_type = if let Some(annotation) = binding_syntax.annotation() {
                let (resolved, diagnostics) =
                    self.resolver
                        .resolve_annotation(self.module, annotation, signature);
                self.diagnostics.extend(diagnostics);
                let expected = resolved?.type_id()?;
                self.require_same_type(expected, inferred, span, annotation.span());
                expected
            } else {
                inferred
            };
            let local = LocalId::from_raw(self.next_local);
            self.next_local = self.next_local.saturating_add(1);
            let binding = BindingId::from_raw(self.next_binding);
            self.next_binding = self.next_binding.saturating_add(1);
            typed_bindings.push(TypedLocalBinding {
                binding,
                local,
                name: binding_syntax.name().to_owned(),
                local_type,
                span: binding_syntax.span(),
            });
        }
        for binding in &typed_bindings {
            self.scopes
                .last_mut()
                .expect("body checker always has a lexical scope")
                .insert(
                    binding.name.clone(),
                    Binding {
                        id: binding.binding,
                        kind: BindingKind::Local(binding.local),
                        type_id: binding.local_type,
                        function_depth: self.function_depth,
                    },
                );
        }
        Some(TypedStatementKind::MultipleLocal {
            bindings: typed_bindings,
            value,
        })
    }

    fn check_fixed_pack(
        &mut self,
        values: &[ExpressionSyntax],
        expected: Option<(&[pop_foundation::TypeId], pop_foundation::TypeId)>,
        expected_arity: usize,
        span: SourceSpan,
        context: &str,
    ) -> Option<TypedExpression> {
        if values.len() == 1 {
            let value = self.check_expression_expected(
                &values[0],
                expected.map(|(_, pack)| ExpectedExpressionType::plain(pack)),
            )?;
            let Some(SemanticType::Tuple(elements)) =
                self.resolver.arena().get(value.type_id()).cloned()
            else {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    span,
                    context,
                    expected_arity,
                    1,
                ));
                return None;
            };
            if let Some((expected_elements, expected_pack)) = expected {
                if elements.len() != expected_elements.len() {
                    self.diagnostics.push(type_diagnostics::wrong_value_arity(
                        span,
                        context,
                        expected_elements.len(),
                        elements.len(),
                    ));
                    return None;
                }
                self.require_same_type(expected_pack, value.type_id(), value.span(), span);
            }
            return Some(value);
        }
        if let Some((expected_elements, _)) = expected
            && values.len() != expected_elements.len()
        {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                context,
                expected_elements.len(),
                values.len(),
            ));
            return None;
        }
        let mut typed = Vec::with_capacity(values.len());
        for (index, value) in values.iter().enumerate() {
            let expected_type = expected.and_then(|(elements, _)| elements.get(index).copied());
            let value = self.check_expression_expected(
                value,
                expected_type.map(ExpectedExpressionType::plain),
            )?;
            if let Some(expected_type) = expected_type {
                self.require_same_type(expected_type, value.type_id(), value.span(), span);
            }
            typed.push(value);
        }
        let element_types = typed.iter().map(TypedExpression::type_id).collect();
        let pack_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Tuple(element_types))
            .ok()?;
        Some(TypedExpression {
            kind: TypedExpressionKind::Tuple(typed),
            type_id: pack_type,
            span,
        })
    }

    pub(crate) fn check_local_function(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        function: &CaptureFunctionSyntax,
    ) -> Option<TypedStatementKind> {
        let shape = self.resolve_closure_shape(signature, function)?;
        let local = LocalId::from_raw(self.next_local);
        self.next_local = self.next_local.saturating_add(1);
        let binding = BindingId::from_raw(self.next_binding);
        self.next_binding = self.next_binding.saturating_add(1);
        self.scopes
            .last_mut()
            .expect("body checker always has a lexical scope")
            .insert(
                name.to_owned(),
                Binding {
                    id: binding,
                    kind: BindingKind::Local(local),
                    type_id: shape.function_type,
                    function_depth: self.function_depth,
                },
            );
        self.written_bindings.insert(binding);
        let closure = self.check_resolved_closure(signature, function, shape)?;
        Some(TypedStatementKind::Local {
            binding,
            local,
            name: name.to_owned(),
            local_type: closure.type_id(),
            initializer: closure,
        })
    }

    pub(crate) fn resolve_closure_shape(
        &mut self,
        outer: &ResolvedFunctionSignature,
        function: &CaptureFunctionSyntax,
    ) -> Option<ResolvedClosureShape> {
        let mut names = BTreeMap::new();
        let mut parameters = Vec::new();
        for parameter in function.parameters() {
            if let Some(original) = names.insert(parameter.name().to_owned(), parameter.span()) {
                self.diagnostics.push(type_diagnostics::duplicate_binding(
                    parameter.span(),
                    parameter.name(),
                    original,
                ));
                continue;
            }
            let (resolved, diagnostics) =
                self.resolver
                    .resolve_annotation(self.module, parameter.parameter_type(), outer);
            self.diagnostics.extend(diagnostics);
            parameters.push((
                parameter.name().to_owned(),
                resolved?.type_id()?,
                parameter.span(),
            ));
        }
        let mut results = Vec::new();
        for result in function.results() {
            let (resolved, diagnostics) =
                self.resolver.resolve_annotation(self.module, result, outer);
            self.diagnostics.extend(diagnostics);
            results.push((resolved?.type_id()?, result.span()));
        }
        if !self.diagnostics.is_empty() {
            return None;
        }
        let function_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Function {
                is_async: function.is_async(),
                parameters: parameters.iter().map(|(_, type_id, _)| *type_id).collect(),
                results: results.iter().map(|(type_id, _)| *type_id).collect(),
                effects: crate::EffectSummary::empty(),
            })
            .ok()?;
        Some(ResolvedClosureShape {
            parameters,
            results,
            function_type,
        })
    }

    #[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
    pub(crate) fn check_resolved_closure(
        &mut self,
        outer: &ResolvedFunctionSignature,
        function: &CaptureFunctionSyntax,
        shape: ResolvedClosureShape,
    ) -> Option<TypedExpression> {
        let nested = NestedFunctionId::from_raw(self.next_nested_function);
        self.next_nested_function = self.next_nested_function.saturating_add(1);
        self.function_depth = self.function_depth.saturating_add(1);
        let depth = self.function_depth;
        self.active_functions.push(ActiveFunction {
            function: nested,
            depth,
            next_capture: 0,
            captures: BTreeMap::new(),
        });
        self.scopes.push(BTreeMap::new());

        let mut typed_parameters = Vec::new();
        for (index, (name, type_id, span)) in shape.parameters.iter().enumerate() {
            let parameter = ValueParameterId::from_raw(u32::try_from(index).unwrap_or(u32::MAX));
            let binding = BindingId::from_raw(self.next_binding);
            self.next_binding = self.next_binding.saturating_add(1);
            self.scopes
                .last_mut()
                .expect("closure scope was just pushed")
                .insert(
                    name.clone(),
                    Binding {
                        id: binding,
                        kind: BindingKind::Parameter(parameter),
                        type_id: *type_id,
                        function_depth: depth,
                    },
                );
            typed_parameters.push(TypedClosureParameter {
                binding,
                parameter,
                name: name.clone(),
                type_id: *type_id,
                span: *span,
            });
        }

        let nested_signature = ResolvedFunctionSignature::canonical_with_async(
            outer.symbol(),
            format!("{}$closure{}", outer.name(), nested.raw()),
            Vec::new(),
            shape.parameters.clone(),
            shape.results.clone(),
            function.is_async(),
        );
        self.signature_stack.push(nested_signature.clone());
        let enclosing_loop_depth = std::mem::replace(&mut self.loop_depth, 0);
        let mut statements = Vec::new();
        for statement in function.body() {
            if let Some(typed) = self.check_statement(&nested_signature, statement) {
                statements.push(typed);
            }
        }
        if !shape.results.is_empty() && !statements_definitely_return(&statements) {
            self.diagnostics
                .push(type_diagnostics::not_all_paths_return(function.span()));
        }
        self.signature_stack
            .pop()
            .expect("closure signature was just pushed");
        self.loop_depth = enclosing_loop_depth;

        self.scopes.pop().expect("closure scope was just pushed");
        let active = self
            .active_functions
            .pop()
            .expect("closure capture context was just pushed");
        debug_assert_eq!(active.function, nested);
        self.function_depth = self.function_depth.saturating_sub(1);
        let captures = active
            .captures
            .into_values()
            .map(|capture| TypedCapture {
                capture: capture.capture,
                binding: capture.binding,
                source: capture.source,
                type_id: capture.type_id,
                mode: if self.written_bindings.contains(&capture.binding) {
                    CaptureMode::Cell
                } else {
                    CaptureMode::Value
                },
            })
            .collect();
        Some(TypedExpression {
            kind: TypedExpressionKind::Closure(TypedClosure {
                function: nested,
                parameters: typed_parameters,
                results: shape.results.iter().map(|(type_id, _)| *type_id).collect(),
                captures,
                body: TypedBody { statements },
                span: function.span(),
            }),
            type_id: shape.function_type,
            span: function.span(),
        })
    }

    pub(crate) fn check_closure_expression(
        &mut self,
        outer: &ResolvedFunctionSignature,
        function: &CaptureFunctionSyntax,
    ) -> Option<TypedExpression> {
        let shape = self.resolve_closure_shape(outer, function)?;
        self.check_resolved_closure(outer, function, shape)
    }

    pub(crate) fn check_assignment(
        &mut self,
        target: &ExpressionSyntax,
        operator: Option<SyntaxBinaryOperator>,
        value: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        if let Some(operator) = operator {
            return self.check_compound_assignment(target, operator, value, span);
        }
        self.check_plain_assignment(target, value, span)
    }

    fn check_multiple_assignment(
        &mut self,
        targets: &[ExpressionSyntax],
        values: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let targets: Option<Vec<_>> = targets
            .iter()
            .map(|target| self.check_multiple_assignment_target(target, span))
            .collect();
        let targets = targets?;
        let element_types: Vec<_> = targets
            .iter()
            .map(TypedAssignmentTarget::value_type)
            .collect();
        let pack_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Tuple(element_types.clone()))
            .ok()?;
        let value = self.check_fixed_pack(
            values,
            Some((&element_types, pack_type)),
            targets.len(),
            span,
            "assignment",
        )?;
        Some(TypedStatementKind::MultipleAssignment { targets, value })
    }

    fn check_multiple_assignment_target(
        &mut self,
        target: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedAssignmentTarget> {
        if let ExpressionSyntaxKind::Name(path) = target.kind()
            && path.len() == 1
            && let Some(binding) = self.binding_by_name(&path[0])
        {
            if matches!(
                binding.kind,
                BindingKind::LoopLocal(_) | BindingKind::ImmutableLocal(_)
            ) {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "multiple assignment",
                    "immutable numeric for binding",
                ));
                return None;
            }
            let target_kind = self.binding_reference_kind(binding)?;
            if matches!(target_kind, TypedExpressionKind::Parameter(_)) {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "multiple assignment",
                    "immutable parameter",
                ));
                return None;
            }
            self.written_bindings.insert(binding.id);
            self.invalidate_flow_binding(binding.id);
            return match target_kind {
                TypedExpressionKind::Local(local) => Some(TypedAssignmentTarget::Local {
                    binding: binding.id,
                    local,
                    value_type: binding.type_id,
                }),
                TypedExpressionKind::Capture(capture) => Some(TypedAssignmentTarget::Capture {
                    binding: binding.id,
                    capture,
                    value_type: binding.type_id,
                }),
                _ => None,
            };
        }

        let target = self.check_expression(target)?;
        let target_type = target.type_id();
        if let TypedExpressionKind::ArrayGet { array, index } = target.kind {
            let Some(SemanticType::Array(element_type)) =
                self.resolver.arena().get(array.type_id()).cloned()
            else {
                return None;
            };
            return Some(TypedAssignmentTarget::Array {
                array: *array,
                index: *index,
                element_type,
            });
        }
        if let TypedExpressionKind::ListGet { list, index } = target.kind {
            let Some(SemanticType::Builtin { arguments, .. }) =
                self.resolver.arena().get(list.type_id()).cloned()
            else {
                return None;
            };
            return Some(TypedAssignmentTarget::List {
                list: *list,
                index: *index,
                element_type: *arguments.first()?,
            });
        }
        if let TypedExpressionKind::TableGet { table, key } = target.kind {
            let Some(SemanticType::Table { value, .. }) =
                self.resolver.arena().get(table.type_id()).cloned()
            else {
                return None;
            };
            return Some(TypedAssignmentTarget::Table {
                table: *table,
                key: *key,
                value_type: value,
            });
        }
        let TypedExpressionKind::Field { base, field } = target.kind else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "multiple assignment",
                self.type_name(target_type),
            ));
            return None;
        };
        if self
            .resolver
            .class_definition_for_type(base.type_id())
            .is_none()
        {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "multiple assignment",
                "immutable field",
            ));
            return None;
        }
        Some(TypedAssignmentTarget::Field {
            base: *base,
            field,
            value_type: target_type,
        })
    }

    fn check_plain_assignment(
        &mut self,
        target: &ExpressionSyntax,
        value: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        if let ExpressionSyntaxKind::Name(path) = target.kind()
            && path.len() == 1
            && let Some(binding) = self.binding_by_name(&path[0])
        {
            if matches!(
                binding.kind,
                BindingKind::LoopLocal(_) | BindingKind::ImmutableLocal(_)
            ) {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "assignment",
                    "immutable numeric for binding",
                ));
                return None;
            }
            let target_kind = self.binding_reference_kind(binding)?;
            if matches!(target_kind, TypedExpressionKind::Parameter(_)) {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "assignment",
                    "immutable parameter",
                ));
                return None;
            }
            let value = self.check_expression_expected(
                value,
                Some(ExpectedExpressionType::plain(binding.type_id)),
            )?;
            self.require_same_type(binding.type_id, value.type_id(), value.span(), span);
            self.written_bindings.insert(binding.id);
            self.invalidate_flow_binding(binding.id);
            return match target_kind {
                TypedExpressionKind::Local(local) => {
                    Some(TypedStatementKind::LocalSet { local, value })
                }
                TypedExpressionKind::Parameter(parameter) => {
                    Some(TypedStatementKind::ParameterSet { parameter, value })
                }
                TypedExpressionKind::Capture(capture) => {
                    Some(TypedStatementKind::CaptureSet { capture, value })
                }
                _ => None,
            };
        }
        let target = self.check_expression(target)?;
        let target_type = target.type_id();
        if let TypedExpressionKind::ArrayGet { array, index } = target.kind {
            let Some(SemanticType::Array(element_type)) =
                self.resolver.arena().get(array.type_id()).cloned()
            else {
                return None;
            };
            let value = self.check_expression_expected(
                value,
                Some(ExpectedExpressionType::plain(element_type)),
            )?;
            self.require_same_type(element_type, value.type_id(), value.span(), span);
            return Some(TypedStatementKind::ArraySet {
                array: *array,
                index: *index,
                value,
            });
        }
        if let TypedExpressionKind::ListGet { list, index } = target.kind {
            let Some(SemanticType::Builtin { arguments, .. }) =
                self.resolver.arena().get(list.type_id()).cloned()
            else {
                return None;
            };
            let element_type = *arguments.first()?;
            let value = self.check_expression_expected(
                value,
                Some(ExpectedExpressionType::plain(element_type)),
            )?;
            self.require_same_type(element_type, value.type_id(), value.span(), span);
            return Some(TypedStatementKind::ListSet {
                list: *list,
                index: *index,
                value,
            });
        }
        if let TypedExpressionKind::TableGet { table, key } = target.kind {
            let Some(SemanticType::Table {
                value: value_type, ..
            }) = self.resolver.arena().get(table.type_id()).cloned()
            else {
                return None;
            };
            let value = self.check_expression_expected(
                value,
                Some(ExpectedExpressionType::plain(value_type)),
            )?;
            self.require_same_type(value_type, value.type_id(), value.span(), span);
            return Some(TypedStatementKind::TableSet {
                table: *table,
                key: *key,
                value,
            });
        }
        let TypedExpressionKind::Field { base, field } = target.kind else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "assignment",
                self.type_name(target_type),
            ));
            return None;
        };
        if self
            .resolver
            .class_definition_for_type(base.type_id())
            .is_none()
        {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "assignment",
                "immutable field",
            ));
            return None;
        }
        let value = self
            .check_expression_expected(value, Some(ExpectedExpressionType::plain(target_type)))?;
        self.require_same_type(target_type, value.type_id(), value.span(), span);
        Some(TypedStatementKind::FieldSet {
            base: *base,
            field,
            value,
        })
    }

    fn check_compound_assignment(
        &mut self,
        target: &ExpressionSyntax,
        operator: SyntaxBinaryOperator,
        value: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        if let ExpressionSyntaxKind::Name(path) = target.kind()
            && path.len() == 1
            && let Some(binding) = self.binding_by_name(&path[0])
        {
            if matches!(
                binding.kind,
                BindingKind::LoopLocal(_) | BindingKind::ImmutableLocal(_)
            ) {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "compound assignment",
                    "immutable numeric for binding",
                ));
                return None;
            }
            let target_kind = self.binding_reference_kind(binding)?;
            if matches!(target_kind, TypedExpressionKind::Parameter(_)) {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "compound assignment",
                    "immutable parameter",
                ));
                return None;
            }
            let current = TypedExpression {
                kind: target_kind.clone(),
                type_id: binding.type_id,
                span: target.span(),
            };
            let right = self.check_expression_expected(
                value,
                Some(ExpectedExpressionType::plain(binding.type_id)),
            )?;
            let operator =
                self.check_compound_operator(operator, binding.type_id, right.type_id(), span)?;
            let value = compound_expression(current, right, operator, span);
            self.written_bindings.insert(binding.id);
            self.invalidate_flow_binding(binding.id);
            return match target_kind {
                TypedExpressionKind::Local(local) => {
                    Some(TypedStatementKind::LocalSet { local, value })
                }
                TypedExpressionKind::Capture(capture) => {
                    Some(TypedStatementKind::CaptureSet { capture, value })
                }
                _ => None,
            };
        }

        let target = self.check_expression(target)?;
        if let TypedExpressionKind::ArrayGet { array, index } = target.kind {
            let Some(SemanticType::Array(element_type)) =
                self.resolver.arena().get(array.type_id()).cloned()
            else {
                return None;
            };
            let value = self.check_expression_expected(
                value,
                Some(ExpectedExpressionType::plain(element_type)),
            )?;
            let operator =
                self.check_compound_operator(operator, element_type, value.type_id(), span)?;
            return Some(TypedStatementKind::CompoundArraySet {
                array: *array,
                index: *index,
                element_type,
                operator,
                value,
            });
        }

        let target_type = target.type_id();
        let TypedExpressionKind::Field { base, field } = target.kind else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "compound assignment",
                self.type_name(target_type),
            ));
            return None;
        };
        if self
            .resolver
            .class_definition_for_type(base.type_id())
            .is_none()
        {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "compound assignment",
                "immutable field",
            ));
            return None;
        }
        let value = self
            .check_expression_expected(value, Some(ExpectedExpressionType::plain(target_type)))?;
        let operator =
            self.check_compound_operator(operator, target_type, value.type_id(), span)?;
        Some(TypedStatementKind::CompoundFieldSet {
            base: *base,
            field,
            value_type: target_type,
            operator,
            value,
        })
    }

    fn check_compound_operator(
        &mut self,
        operator: SyntaxBinaryOperator,
        target: pop_foundation::TypeId,
        value: pop_foundation::TypeId,
        span: SourceSpan,
    ) -> Option<TypedCompoundOperator> {
        let operands_match = target == value;
        let valid = match operator {
            SyntaxBinaryOperator::Add
            | SyntaxBinaryOperator::Subtract
            | SyntaxBinaryOperator::Multiply
            | SyntaxBinaryOperator::Divide => operands_match && self.is_numeric(target),
            SyntaxBinaryOperator::Remainder => operands_match && self.is_integer(target),
            SyntaxBinaryOperator::Concat => {
                operands_match && self.is_primitive(target, crate::PrimitiveType::String)
            }
            _ => false,
        };
        if !valid {
            self.invalid_operator(span, compound_operator_text(operator), &[target, value]);
            return None;
        }
        Some(match operator {
            SyntaxBinaryOperator::Add => TypedCompoundOperator::Add,
            SyntaxBinaryOperator::Subtract => TypedCompoundOperator::Subtract,
            SyntaxBinaryOperator::Multiply => TypedCompoundOperator::Multiply,
            SyntaxBinaryOperator::Divide => TypedCompoundOperator::Divide,
            SyntaxBinaryOperator::Remainder => TypedCompoundOperator::Remainder,
            SyntaxBinaryOperator::Concat => TypedCompoundOperator::Concat,
            _ => unreachable!("parser exposes only supported compound operators"),
        })
    }

    pub(crate) fn check_nested_statements(
        &mut self,
        signature: &ResolvedFunctionSignature,
        statements: &[StatementSyntax],
    ) -> Vec<TypedStatement> {
        self.scopes.push(BTreeMap::new());
        let typed = statements
            .iter()
            .filter_map(|statement| self.check_statement(signature, statement))
            .collect();
        self.scopes
            .pop()
            .expect("nested lexical scope was just pushed");
        typed
    }

    fn check_nested_statements_with_narrowing(
        &mut self,
        signature: &ResolvedFunctionSignature,
        statements: &[StatementSyntax],
        binding: BindingId,
        inner_type: pop_foundation::TypeId,
    ) -> Vec<TypedStatement> {
        self.flow_narrowings
            .push(BTreeMap::from([(binding, inner_type)]));
        let typed = self.check_nested_statements(signature, statements);
        self.flow_narrowings
            .pop()
            .expect("flow narrowing was just pushed");
        typed
    }

    fn optional_narrowing(
        &mut self,
        condition: &ExpressionSyntax,
    ) -> Option<(BindingId, pop_foundation::TypeId, bool)> {
        let ExpressionSyntaxKind::Binary {
            operator,
            left,
            right,
        } = condition.kind()
        else {
            return None;
        };
        let present_on_true = match operator {
            SyntaxBinaryOperator::NotEqual => true,
            SyntaxBinaryOperator::Equal => false,
            _ => return None,
        };
        let name = match (left.kind(), right.kind()) {
            (ExpressionSyntaxKind::Name(path), ExpressionSyntaxKind::Nil)
            | (ExpressionSyntaxKind::Nil, ExpressionSyntaxKind::Name(path))
                if path.len() == 1 =>
            {
                &path[0]
            }
            _ => return None,
        };
        let binding = self.binding_by_name(name)?;
        let inner = self.optional_inner(binding.type_id)?;
        Some((binding.id, inner, present_on_true))
    }

    fn check_optional_if(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        initializer: &ExpressionSyntax,
        then_statements: &[StatementSyntax],
        else_statements: &[StatementSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let initializer = self.check_expression(initializer)?;
        let Some(inner_type) = self.optional_inner(initializer.type_id()) else {
            self.invalid_operator(span, "if local", &[initializer.type_id()]);
            return None;
        };
        let (binding, local, then_body) =
            self.check_optional_binding_body(signature, name, inner_type, then_statements, false);
        let else_body = self.check_nested_statements(signature, else_statements);
        Some(TypedStatementKind::OptionalIf {
            binding,
            local,
            name: name.to_owned(),
            inner_type,
            initializer,
            then_body,
            else_body,
        })
    }

    fn check_optional_while(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        initializer: &ExpressionSyntax,
        statements: &[StatementSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let initializer = self.check_expression(initializer)?;
        let Some(inner_type) = self.optional_inner(initializer.type_id()) else {
            self.invalid_operator(span, "while local", &[initializer.type_id()]);
            return None;
        };
        let (binding, local, body) =
            self.check_optional_binding_body(signature, name, inner_type, statements, true);
        Some(TypedStatementKind::OptionalWhile {
            binding,
            local,
            name: name.to_owned(),
            inner_type,
            initializer,
            body,
        })
    }

    fn check_optional_binding_body(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        inner_type: pop_foundation::TypeId,
        statements: &[StatementSyntax],
        is_loop: bool,
    ) -> (BindingId, LocalId, Vec<TypedStatement>) {
        let local = LocalId::from_raw(self.next_local);
        self.next_local = self.next_local.saturating_add(1);
        let binding = BindingId::from_raw(self.next_binding);
        self.next_binding = self.next_binding.saturating_add(1);
        self.scopes.push(BTreeMap::from([(
            name.to_owned(),
            Binding {
                id: binding,
                kind: BindingKind::ImmutableLocal(local),
                type_id: inner_type,
                function_depth: self.function_depth,
            },
        )]));
        if is_loop {
            self.loop_depth = self.loop_depth.saturating_add(1);
        }
        let body = statements
            .iter()
            .filter_map(|statement| self.check_statement(signature, statement))
            .collect();
        if is_loop {
            self.loop_depth = self.loop_depth.saturating_sub(1);
        }
        self.scopes
            .pop()
            .expect("optional binding scope was just pushed");
        (binding, local, body)
    }

    pub(crate) fn check_repeat_until(
        &mut self,
        signature: &ResolvedFunctionSignature,
        statements: &[StatementSyntax],
        condition: &ExpressionSyntax,
    ) -> Option<TypedStatementKind> {
        self.scopes.push(BTreeMap::new());
        self.loop_depth = self.loop_depth.saturating_add(1);
        let body = statements
            .iter()
            .filter_map(|statement| self.check_statement(signature, statement))
            .collect();
        if statements.iter().any(|statement| {
            matches!(
                statement.kind(),
                StatementSyntaxKind::Local { .. }
                    | StatementSyntaxKind::MultipleLocal { .. }
                    | StatementSyntaxKind::LocalFunction { .. }
            )
        }) && contains_continue_for_current_loop(statements)
        {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                condition.span(),
                "repeat continue",
                "body-local condition scope",
            ));
        }
        let condition = self.check_condition(condition);
        self.loop_depth = self.loop_depth.saturating_sub(1);
        self.scopes
            .pop()
            .expect("repeat-until lexical scope was just pushed");
        condition.map(|condition| TypedStatementKind::RepeatUntil { body, condition })
    }

    #[allow(clippy::single_match_else, clippy::too_many_lines)]
    pub(crate) fn check_match(
        &mut self,
        signature: &ResolvedFunctionSignature,
        scrutinee: &ExpressionSyntax,
        arms: &[MatchArmSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let scrutinee = self.check_expression(scrutinee)?;
        if let Some((success, error)) = self.resolver.result_parts(scrutinee.type_id()) {
            return self.check_result_match(signature, scrutinee, success, error, arms, span);
        }
        if let Some(SemanticType::ErrorUnion { source, .. }) =
            self.resolver.arena().get(scrutinee.type_id()).cloned()
        {
            let definition = self
                .resolver
                .error_definition_for_type(scrutinee.type_id())?
                .clone();
            return self.check_error_match(signature, scrutinee, &definition, source, arms, span);
        }
        let definition_symbol = match self.resolver.arena().get(scrutinee.type_id()) {
            Some(SemanticType::TaggedUnion { .. }) => self
                .resolver
                .union_definition_for_type(scrutinee.type_id())?
                .symbol(),
            _ => {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "match",
                    self.type_name(scrutinee.type_id()),
                ));
                return None;
            }
        };
        let definition = self.resolver.union_definition(definition_symbol)?.clone();
        let mut seen = BTreeMap::new();
        let mut typed_arms = Vec::new();
        for arm in arms {
            let (arm_definition, mut case) =
                match self.lookup_union_case(arm.case_path(), arm.span()) {
                    UnionCaseLookup::Found(definition, case) => (definition, case),
                    UnionCaseLookup::Missing | UnionCaseLookup::NotUnion => continue,
                };
            if arm_definition.symbol() != definition.symbol() {
                if arm_definition.symbol() == self.resolver.union_source_symbol(definition.symbol())
                {
                    let Some(concrete_case) = definition
                        .cases()
                        .iter()
                        .find(|candidate| candidate.name() == case.name())
                        .cloned()
                    else {
                        continue;
                    };
                    case = concrete_case;
                } else {
                    self.diagnostics.push(type_diagnostics::foreign_match_case(
                        arm.span(),
                        arm.case_path().join("."),
                    ));
                    continue;
                }
            }
            if let Some(original) = seen.insert(case.case(), arm.span()) {
                self.diagnostics
                    .push(type_diagnostics::duplicate_match_case(
                        arm.span(),
                        case.name(),
                        original,
                    ));
                continue;
            }
            if case.parameters().len() != arm.bindings().len() {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    arm.span(),
                    "match case payload",
                    case.parameters().len(),
                    arm.bindings().len(),
                ));
                continue;
            }

            self.scopes.push(BTreeMap::new());
            let mut names = BTreeMap::new();
            let mut bindings = Vec::new();
            for (name, (_, type_id, parameter_span)) in arm.bindings().iter().zip(case.parameters())
            {
                if name == "_" {
                    bindings.push(TypedMatchBinding {
                        binding: None,
                        local: None,
                        name: name.clone(),
                        type_id: *type_id,
                        span: arm.span(),
                    });
                    continue;
                }
                if let Some(original) = names.insert(name.clone(), arm.span()) {
                    self.diagnostics.push(type_diagnostics::duplicate_binding(
                        arm.span(),
                        name,
                        original,
                    ));
                    continue;
                }
                let local = LocalId::from_raw(self.next_local);
                self.next_local = self.next_local.saturating_add(1);
                let binding = BindingId::from_raw(self.next_binding);
                self.next_binding = self.next_binding.saturating_add(1);
                self.scopes
                    .last_mut()
                    .expect("match arm scope was just pushed")
                    .insert(
                        name.clone(),
                        Binding {
                            id: binding,
                            kind: BindingKind::Local(local),
                            type_id: *type_id,
                            function_depth: self.function_depth,
                        },
                    );
                bindings.push(TypedMatchBinding {
                    binding: Some(binding),
                    local: Some(local),
                    name: name.clone(),
                    type_id: *type_id,
                    span: *parameter_span,
                });
            }
            let body = arm
                .body()
                .iter()
                .filter_map(|statement| self.check_statement(signature, statement))
                .collect();
            self.scopes.pop().expect("match arm scope was just pushed");
            typed_arms.push(TypedMatchArm {
                union: definition.symbol(),
                case: case.case(),
                bindings,
                body,
                span: arm.span(),
            });
        }

        let missing: Vec<_> = definition
            .cases()
            .iter()
            .filter(|case| !seen.contains_key(&case.case()))
            .collect();
        if !missing.is_empty() {
            let declaration_name = self
                .resolver
                .database()
                .index()
                .declaration(definition.symbol())
                .map_or("Union", pop_resolve::Declaration::name);
            let replacement = missing_match_arms(declaration_name, &missing);
            let insert_offset = span.range().end().to_u32().saturating_sub(3);
            let insertion = SourceSpan::new(
                span.file(),
                TextRange::empty(TextSize::from_u32(insert_offset)),
            );
            let missing_names: Vec<_> = missing.iter().map(|case| case.name()).collect();
            self.diagnostics.push(type_diagnostics::missing_match_cases(
                span,
                &missing_names,
                insertion,
                replacement,
            ));
        }

        Some(TypedStatementKind::Match {
            scrutinee,
            union: definition.symbol(),
            arms: typed_arms,
        })
    }

    fn check_error_match(
        &mut self,
        signature: &ResolvedFunctionSignature,
        scrutinee: TypedExpression,
        definition: &crate::ErrorDefinition,
        source: pop_foundation::SymbolId,
        arms: &[MatchArmSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let mut seen = BTreeMap::new();
        let mut typed_arms = Vec::new();
        for arm in arms {
            let (_, source_case) = match self.lookup_error_case(arm.case_path(), arm.span()) {
                ErrorCaseLookup::Found(definition, case) => (definition, case),
                ErrorCaseLookup::Missing | ErrorCaseLookup::NotError => continue,
            };
            let Some(case) = definition
                .cases()
                .iter()
                .find(|candidate| candidate.name() == source_case.name())
                .cloned()
            else {
                self.diagnostics.push(type_diagnostics::foreign_match_case(
                    arm.span(),
                    arm.case_path().join("."),
                ));
                continue;
            };
            if let Some(original) = seen.insert(case.case(), arm.span()) {
                self.diagnostics
                    .push(type_diagnostics::duplicate_match_case(
                        arm.span(),
                        case.name(),
                        original,
                    ));
                continue;
            }
            if case.parameters().len() != arm.bindings().len() {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    arm.span(),
                    "error match case payload",
                    case.parameters().len(),
                    arm.bindings().len(),
                ));
                continue;
            }
            self.scopes.push(BTreeMap::new());
            let mut names = BTreeMap::new();
            let mut bindings = Vec::new();
            for (name, (_, type_id, parameter_span)) in arm.bindings().iter().zip(case.parameters())
            {
                if name == "_" {
                    bindings.push(TypedMatchBinding {
                        binding: None,
                        local: None,
                        name: name.clone(),
                        type_id: *type_id,
                        span: arm.span(),
                    });
                    continue;
                }
                if let Some(original) = names.insert(name.clone(), arm.span()) {
                    self.diagnostics.push(type_diagnostics::duplicate_binding(
                        arm.span(),
                        name,
                        original,
                    ));
                    continue;
                }
                let local = LocalId::from_raw(self.next_local);
                self.next_local = self.next_local.saturating_add(1);
                let binding = BindingId::from_raw(self.next_binding);
                self.next_binding = self.next_binding.saturating_add(1);
                self.scopes
                    .last_mut()
                    .expect("error match scope was just pushed")
                    .insert(
                        name.clone(),
                        Binding {
                            id: binding,
                            kind: BindingKind::Local(local),
                            type_id: *type_id,
                            function_depth: self.function_depth,
                        },
                    );
                bindings.push(TypedMatchBinding {
                    binding: Some(binding),
                    local: Some(local),
                    name: name.clone(),
                    type_id: *type_id,
                    span: *parameter_span,
                });
            }
            let body = arm
                .body()
                .iter()
                .filter_map(|statement| self.check_statement(signature, statement))
                .collect();
            self.scopes
                .pop()
                .expect("error match scope was just pushed");
            typed_arms.push(TypedErrorMatchArm {
                error: definition.error(),
                case: case.case(),
                bindings,
                body,
                span: arm.span(),
            });
        }
        let missing: Vec<_> = definition
            .cases()
            .iter()
            .filter(|case| !seen.contains_key(&case.case()))
            .collect();
        if !missing.is_empty() {
            let declaration_name = self
                .resolver
                .database()
                .index()
                .declaration(source)
                .map_or("Error", pop_resolve::Declaration::name);
            let replacement = missing_error_match_arms(declaration_name, &missing);
            let insert_offset = span.range().end().to_u32().saturating_sub(3);
            let insertion = SourceSpan::new(
                span.file(),
                TextRange::empty(TextSize::from_u32(insert_offset)),
            );
            let missing_names: Vec<_> = missing.iter().map(|case| case.name()).collect();
            self.diagnostics.push(type_diagnostics::missing_match_cases(
                span,
                &missing_names,
                insertion,
                replacement,
            ));
        }
        Some(TypedStatementKind::ErrorMatch {
            scrutinee,
            error: definition.error(),
            arms: typed_arms,
        })
    }

    fn check_result_match(
        &mut self,
        signature: &ResolvedFunctionSignature,
        scrutinee: TypedExpression,
        success_type: pop_foundation::TypeId,
        error_type: pop_foundation::TypeId,
        arms: &[MatchArmSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let result_type = scrutinee.type_id();
        let mut seen = BTreeMap::new();
        let mut typed_arms = Vec::new();
        for arm in arms {
            let (case, payload_type) = match arm.case_path() {
                [result, case] if result == "Result" && case == "Ok" => {
                    (pop_foundation::ResultCaseId::from_raw(0), success_type)
                }
                [result, case] if result == "Result" && case == "Error" => {
                    (pop_foundation::ResultCaseId::from_raw(1), error_type)
                }
                _ => {
                    self.diagnostics.push(type_diagnostics::foreign_match_case(
                        arm.span(),
                        arm.case_path().join("."),
                    ));
                    continue;
                }
            };
            if let Some(original) = seen.insert(case, arm.span()) {
                self.diagnostics
                    .push(type_diagnostics::duplicate_match_case(
                        arm.span(),
                        arm.case_path().last().map_or("Result", String::as_str),
                        original,
                    ));
                continue;
            }
            if arm.bindings().len() != 1 {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    arm.span(),
                    "Result match case payload",
                    1,
                    arm.bindings().len(),
                ));
                continue;
            }
            self.scopes.push(BTreeMap::new());
            let name = &arm.bindings()[0];
            let binding = if name == "_" {
                TypedMatchBinding {
                    binding: None,
                    local: None,
                    name: name.clone(),
                    type_id: payload_type,
                    span: arm.span(),
                }
            } else {
                let local = LocalId::from_raw(self.next_local);
                self.next_local = self.next_local.saturating_add(1);
                let binding = BindingId::from_raw(self.next_binding);
                self.next_binding = self.next_binding.saturating_add(1);
                self.scopes
                    .last_mut()
                    .expect("result match scope was just pushed")
                    .insert(
                        name.clone(),
                        Binding {
                            id: binding,
                            kind: BindingKind::Local(local),
                            type_id: payload_type,
                            function_depth: self.function_depth,
                        },
                    );
                TypedMatchBinding {
                    binding: Some(binding),
                    local: Some(local),
                    name: name.clone(),
                    type_id: payload_type,
                    span: arm.span(),
                }
            };
            let body = arm
                .body()
                .iter()
                .filter_map(|statement| self.check_statement(signature, statement))
                .collect();
            self.scopes
                .pop()
                .expect("result match scope was just pushed");
            typed_arms.push(TypedResultMatchArm {
                case,
                bindings: vec![binding],
                body,
                span: arm.span(),
            });
        }
        let missing = [
            (pop_foundation::ResultCaseId::from_raw(0), "Ok", "value"),
            (pop_foundation::ResultCaseId::from_raw(1), "Error", "error"),
        ]
        .into_iter()
        .filter(|(case, _, _)| !seen.contains_key(case))
        .collect::<Vec<_>>();
        if !missing.is_empty() {
            let replacement = missing
                .iter()
                .map(|(_, case, binding)| format!("when Result.{case}({binding}) then\n"))
                .collect::<String>();
            let insert_offset = span.range().end().to_u32().saturating_sub(3);
            let insertion = SourceSpan::new(
                span.file(),
                TextRange::empty(TextSize::from_u32(insert_offset)),
            );
            let missing_names = missing.iter().map(|(_, case, _)| *case).collect::<Vec<_>>();
            self.diagnostics.push(type_diagnostics::missing_match_cases(
                span,
                &missing_names,
                insertion,
                replacement,
            ));
        }
        Some(TypedStatementKind::ResultMatch {
            scrutinee,
            result: self.resolver.result_definition()?,
            result_type,
            arms: typed_arms,
        })
    }
}

fn missing_error_match_arms(error_name: &str, cases: &[&crate::ErrorCaseDefinition]) -> String {
    let mut replacement = String::new();
    for case in cases {
        replacement.push_str("when ");
        replacement.push_str(error_name);
        replacement.push('.');
        replacement.push_str(case.name());
        if !case.parameters().is_empty() {
            replacement.push('(');
            for (index, (name, _, _)) in case.parameters().iter().enumerate() {
                if index != 0 {
                    replacement.push_str(", ");
                }
                replacement.push_str(name);
            }
            replacement.push(')');
        }
        replacement.push_str(" then\n");
    }
    replacement
}

fn compound_expression(
    left: TypedExpression,
    right: TypedExpression,
    operator: TypedCompoundOperator,
    span: SourceSpan,
) -> TypedExpression {
    let type_id = left.type_id();
    let kind = match operator {
        TypedCompoundOperator::Concat => TypedExpressionKind::StringConcat {
            left: Box::new(left),
            right: Box::new(right),
        },
        operator => TypedExpressionKind::Binary {
            operator: match operator {
                TypedCompoundOperator::Add => TypedBinaryOperator::Add,
                TypedCompoundOperator::Subtract => TypedBinaryOperator::Subtract,
                TypedCompoundOperator::Multiply => TypedBinaryOperator::Multiply,
                TypedCompoundOperator::Divide => TypedBinaryOperator::Divide,
                TypedCompoundOperator::Remainder => TypedBinaryOperator::Remainder,
                TypedCompoundOperator::Concat => unreachable!(),
            },
            left: Box::new(left),
            right: Box::new(right),
        },
    };
    TypedExpression {
        kind,
        type_id,
        span,
    }
}

const fn compound_operator_text(operator: SyntaxBinaryOperator) -> &'static str {
    match operator {
        SyntaxBinaryOperator::Add => "+=",
        SyntaxBinaryOperator::Subtract => "-=",
        SyntaxBinaryOperator::Multiply => "*=",
        SyntaxBinaryOperator::Divide => "/=",
        SyntaxBinaryOperator::Remainder => "%=",
        SyntaxBinaryOperator::Concat => "..=",
        _ => "compound assignment",
    }
}

fn contains_continue_for_current_loop(statements: &[StatementSyntax]) -> bool {
    statements.iter().any(|statement| match statement.kind() {
        StatementSyntaxKind::Continue => true,
        StatementSyntaxKind::If {
            then_body,
            else_body,
            ..
        }
        | StatementSyntaxKind::OptionalIf {
            then_body,
            else_body,
            ..
        } => {
            contains_continue_for_current_loop(then_body)
                || contains_continue_for_current_loop(else_body)
        }
        StatementSyntaxKind::Match { arms, .. } => arms
            .iter()
            .any(|arm| contains_continue_for_current_loop(arm.body())),
        StatementSyntaxKind::While { .. }
        | StatementSyntaxKind::OptionalWhile { .. }
        | StatementSyntaxKind::RepeatUntil { .. }
        | StatementSyntaxKind::NumericFor { .. }
        | StatementSyntaxKind::GeneralizedFor { .. }
        | StatementSyntaxKind::Defer { .. }
        | StatementSyntaxKind::AsyncDefer { .. }
        | StatementSyntaxKind::Local { .. }
        | StatementSyntaxKind::MultipleLocal { .. }
        | StatementSyntaxKind::LocalFunction { .. }
        | StatementSyntaxKind::Return { .. }
        | StatementSyntaxKind::Break
        | StatementSyntaxKind::Assignment { .. }
        | StatementSyntaxKind::MultipleAssignment { .. }
        | StatementSyntaxKind::Expression(_) => false,
    })
}

fn illegal_cleanup_control(statements: &[StatementSyntax]) -> Option<(&'static str, SourceSpan)> {
    for statement in statements {
        match statement.kind() {
            StatementSyntaxKind::Return { .. } => return Some(("return", statement.span())),
            StatementSyntaxKind::Break => return Some(("break", statement.span())),
            StatementSyntaxKind::Continue => return Some(("continue", statement.span())),
            StatementSyntaxKind::Defer { .. } => return Some(("defer", statement.span())),
            StatementSyntaxKind::AsyncDefer { .. } => {
                return Some(("async defer", statement.span()));
            }
            StatementSyntaxKind::Local { initializer, .. } => {
                if expression_contains_result_propagation(initializer) {
                    return Some(("try", initializer.span()));
                }
            }
            StatementSyntaxKind::MultipleLocal { values, .. } => {
                if let Some(expression) = values
                    .iter()
                    .find(|value| expression_contains_result_propagation(value))
                {
                    return Some(("try", expression.span()));
                }
            }
            StatementSyntaxKind::If {
                condition,
                then_body,
                else_body,
            } => {
                if expression_contains_result_propagation(condition) {
                    return Some(("try", condition.span()));
                }
                if let Some(found) = illegal_cleanup_control(then_body) {
                    return Some(found);
                }
                if let Some(found) = illegal_cleanup_control(else_body) {
                    return Some(found);
                }
            }
            StatementSyntaxKind::OptionalIf {
                initializer,
                then_body,
                else_body,
                ..
            } => {
                if expression_contains_result_propagation(initializer) {
                    return Some(("try", initializer.span()));
                }
                if let Some(found) = illegal_cleanup_control(then_body) {
                    return Some(found);
                }
                if let Some(found) = illegal_cleanup_control(else_body) {
                    return Some(found);
                }
            }
            StatementSyntaxKind::While { condition, body }
            | StatementSyntaxKind::RepeatUntil { condition, body } => {
                if expression_contains_result_propagation(condition) {
                    return Some(("try", condition.span()));
                }
                if let Some(found) = illegal_cleanup_control(body) {
                    return Some(found);
                }
            }
            StatementSyntaxKind::OptionalWhile {
                initializer, body, ..
            } => {
                if expression_contains_result_propagation(initializer) {
                    return Some(("try", initializer.span()));
                }
                if let Some(found) = illegal_cleanup_control(body) {
                    return Some(found);
                }
            }
            StatementSyntaxKind::NumericFor {
                first,
                last,
                step,
                body,
                ..
            } => {
                if [Some(first), Some(last), step.as_ref()]
                    .into_iter()
                    .flatten()
                    .any(expression_contains_result_propagation)
                {
                    return Some(("try", statement.span()));
                }
                if let Some(found) = illegal_cleanup_control(body) {
                    return Some(found);
                }
            }
            StatementSyntaxKind::GeneralizedFor { iterable, body, .. } => {
                if expression_contains_result_propagation(iterable) {
                    return Some(("try", iterable.span()));
                }
                if let Some(found) = illegal_cleanup_control(body) {
                    return Some(found);
                }
            }
            StatementSyntaxKind::Match { scrutinee, arms } => {
                if expression_contains_result_propagation(scrutinee) {
                    return Some(("try", scrutinee.span()));
                }
                for arm in arms {
                    if let Some(found) = illegal_cleanup_control(arm.body()) {
                        return Some(found);
                    }
                }
            }
            StatementSyntaxKind::Assignment { target, value, .. } => {
                if expression_contains_result_propagation(target)
                    || expression_contains_result_propagation(value)
                {
                    return Some(("try", statement.span()));
                }
            }
            StatementSyntaxKind::MultipleAssignment { targets, values } => {
                if targets
                    .iter()
                    .chain(values)
                    .any(expression_contains_result_propagation)
                {
                    return Some(("try", statement.span()));
                }
            }
            StatementSyntaxKind::Expression(expression) => {
                if expression_contains_result_propagation(expression) {
                    return Some(("try", expression.span()));
                }
            }
            StatementSyntaxKind::LocalFunction { .. } => {}
        }
    }
    None
}

fn expression_contains_result_propagation(expression: &ExpressionSyntax) -> bool {
    match expression.kind() {
        ExpressionSyntaxKind::ResultPropagate { .. } => true,
        ExpressionSyntaxKind::Call { callee, arguments }
        | ExpressionSyntaxKind::GenericCall {
            callee, arguments, ..
        } => {
            expression_contains_result_propagation(callee)
                || arguments.iter().any(expression_contains_result_propagation)
        }
        ExpressionSyntaxKind::MethodCall {
            receiver,
            arguments,
            ..
        } => {
            expression_contains_result_propagation(receiver)
                || arguments.iter().any(expression_contains_result_propagation)
        }
        ExpressionSyntaxKind::Index { base, index } => {
            expression_contains_result_propagation(base)
                || expression_contains_result_propagation(index)
        }
        ExpressionSyntaxKind::Construct { fields, .. }
        | ExpressionSyntaxKind::Aggregate { fields } => fields
            .iter()
            .any(|field| expression_contains_result_propagation(field.value())),
        ExpressionSyntaxKind::Array(elements) | ExpressionSyntaxKind::Tuple(elements) => {
            elements.iter().any(expression_contains_result_propagation)
        }
        ExpressionSyntaxKind::Unary { operand, .. }
        | ExpressionSyntaxKind::OptionalPropagate { operand }
        | ExpressionSyntaxKind::Await { operand } => {
            expression_contains_result_propagation(operand)
        }
        ExpressionSyntaxKind::Binary { left, right, .. } => {
            expression_contains_result_propagation(left)
                || expression_contains_result_propagation(right)
        }
        ExpressionSyntaxKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            expression_contains_result_propagation(condition)
                || expression_contains_result_propagation(when_true)
                || expression_contains_result_propagation(when_false)
        }
        ExpressionSyntaxKind::With { base, fields } => {
            expression_contains_result_propagation(base)
                || fields
                    .iter()
                    .any(|field| expression_contains_result_propagation(field.value()))
        }
        ExpressionSyntaxKind::Function(_)
        | ExpressionSyntaxKind::Integer(_)
        | ExpressionSyntaxKind::Float(_)
        | ExpressionSyntaxKind::String(_)
        | ExpressionSyntaxKind::InterpolatedString(_)
        | ExpressionSyntaxKind::Boolean(_)
        | ExpressionSyntaxKind::Nil
        | ExpressionSyntaxKind::Name(_) => false,
    }
}
