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
    CaptureFunctionSyntax, ExpressionSyntax, ExpressionSyntaxKind, MatchArmSyntax, StatementSyntax,
    StatementSyntaxKind,
};

use crate::body_checking::{
    ActiveFunction, Binding, BindingKind, BodyChecker, CheckedInvocation, ExpectedExpressionType,
    ResolvedClosureShape, UnionCaseLookup, missing_match_arms, statements_definitely_return,
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
            StatementSyntaxKind::LocalFunction { name, function } => {
                self.check_local_function(signature, name, function)?
            }
            StatementSyntaxKind::Return { values } => {
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
                let condition = self.check_condition(condition)?;
                let then_body = self.check_nested_statements(signature, then_body);
                let else_body = self.check_nested_statements(signature, else_body);
                TypedStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            StatementSyntaxKind::While { condition, body } => {
                let condition = self.check_condition(condition)?;
                let body = self.check_nested_statements(signature, body);
                TypedStatementKind::While { condition, body }
            }
            StatementSyntaxKind::RepeatUntil { body, condition } => {
                self.check_repeat_until(signature, body, condition)?
            }
            StatementSyntaxKind::Match { scrutinee, arms } => {
                self.check_match(signature, scrutinee, arms, statement.span())?
            }
            StatementSyntaxKind::Assignment { target, value } => {
                self.check_assignment(target, value, statement.span())?
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

    pub(crate) fn check_expression_statement(
        &mut self,
        expression: &ExpressionSyntax,
    ) -> Option<TypedStatementKind> {
        let invocation = match expression.kind() {
            ExpressionSyntaxKind::Call { callee, arguments } => {
                self.check_call_invocation(callee, arguments, expression.span())
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
            CheckedInvocation::Call(checked) => checked,
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

        let nested_signature = ResolvedFunctionSignature::canonical(
            outer.symbol(),
            format!("{}$closure{}", outer.name(), nested.raw()),
            shape.parameters.clone(),
            shape.results.clone(),
        );
        self.signature_stack.push(nested_signature.clone());
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
        value: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        if let ExpressionSyntaxKind::Name(path) = target.kind()
            && path.len() == 1
            && let Some(binding) = self.binding_by_name(&path[0])
        {
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

    pub(crate) fn check_repeat_until(
        &mut self,
        signature: &ResolvedFunctionSignature,
        statements: &[StatementSyntax],
        condition: &ExpressionSyntax,
    ) -> Option<TypedStatementKind> {
        self.scopes.push(BTreeMap::new());
        let body = statements
            .iter()
            .filter_map(|statement| self.check_statement(signature, statement))
            .collect();
        let condition = self.check_condition(condition);
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
        let definition_symbol = match self.resolver.arena().get(scrutinee.type_id()) {
            Some(SemanticType::TaggedUnion { definition }) => *definition,
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
            let (arm_definition, case) = match self.lookup_union_case(arm.case_path(), arm.span()) {
                UnionCaseLookup::Found(definition, case) => (definition, case),
                UnionCaseLookup::Missing | UnionCaseLookup::NotUnion => continue,
            };
            if arm_definition.symbol() != definition.symbol() {
                self.diagnostics.push(type_diagnostics::foreign_match_case(
                    arm.span(),
                    arm.case_path().join("."),
                ));
                continue;
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
}
