//! Typed call, method, interface, and compiler-known invocation selection.
//!
//! Every call leaves this module with a closed static dispatch category,
//! exact argument/result types, and no unknown-effect or runtime lookup path.

use pop_diagnostics::{resolution as resolution_diagnostics, types as type_diagnostics};
use std::collections::BTreeMap;

use pop_foundation::{BuiltinTypeId, ParameterId, ResultCaseId, SourceSpan, TypeId};
use pop_resolve::SymbolSpace;
use pop_syntax::{ExpressionSyntax, ExpressionSyntaxKind};

use crate::body_checking::{
    BodyChecker, CheckedCall, CheckedInvocation, ErrorCaseLookup, ExpectedExpressionType,
    UnionCaseLookup,
};
use crate::typed_body::*;
use crate::{NumericConversionKind, PrimitiveType, SemanticType, StringFormatKind};

impl<'resolver, 'index> BodyChecker<'resolver, 'index> {
    pub(crate) fn check_call(
        &mut self,
        callee: &ExpressionSyntax,
        arguments: &[ExpressionSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if let ExpressionSyntaxKind::Name(path) = callee.kind()
            && matches!(path.as_slice(), [result, case] if result == "Result" && matches!(case.as_str(), "Ok" | "Error"))
        {
            return self.check_result_case(path, arguments, expected, span);
        }
        match self.check_call_invocation(callee, arguments, expected, span)? {
            CheckedInvocation::Call(checked) => self.checked_call_expression(checked),
            CheckedInvocation::Value(value) => Some(value),
        }
    }

    fn check_result_case(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let Some(expected) = expected else {
            self.diagnostics
                .push(type_diagnostics::ambiguous_result_case(
                    span,
                    path.join("."),
                ));
            return None;
        };
        let Some((success, error)) = self.resolver.result_parts(expected.type_id) else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "Result case construction",
                self.type_name(expected.type_id),
            ));
            return None;
        };
        if arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "Result case construction",
                1,
                arguments.len(),
            ));
            return None;
        }
        let ok = path.last().is_some_and(|case| case == "Ok");
        let payload_type = if ok { success } else { error };
        let argument = self.check_expression_expected(
            &arguments[0],
            Some(ExpectedExpressionType::plain(payload_type)),
        )?;
        self.require_same_type(payload_type, argument.type_id(), argument.span(), span);
        Some(TypedExpression {
            kind: TypedExpressionKind::ResultCase {
                result: self.resolver.result_definition()?,
                case: ResultCaseId::from_raw(u32::from(!ok)),
                arguments: vec![argument],
            },
            type_id: expected.type_id,
            span,
        })
    }

    pub(crate) fn check_call_invocation(
        &mut self,
        callee: &ExpressionSyntax,
        arguments: &[ExpressionSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<CheckedInvocation> {
        if let ExpressionSyntaxKind::Name(path) = callee.kind() {
            if path.as_slice() == ["String"] {
                return self
                    .check_string_conversion(arguments, span)
                    .map(CheckedInvocation::Value);
            }
            if matches!(path.as_slice(), [target]
                if self.resolver.arena().source_type(target)
                    .and_then(|type_id| self.numeric_target(type_id))
                    .is_some())
            {
                return self
                    .check_numeric_conversion(path, arguments, span)
                    .map(CheckedInvocation::Value);
            }
            if matches!(
                path.as_slice(),
                [array, operation]
                    if array == "Array"
                        && matches!(operation.as_str(), "length" | "get" | "fill")
            ) {
                return self
                    .check_array_invocation(path, arguments, span)
                    .map(CheckedInvocation::Value);
            }
            if matches!(
                path.as_slice(),
                [iteration, item] if iteration == "Iteration" && item == "Item"
            ) {
                return self
                    .check_iteration_item_invocation(arguments, expected, span)
                    .map(CheckedInvocation::Value);
            }
            if matches!(
                path.as_slice(),
                [list, operation]
                    if list == "List"
                        && matches!(operation.as_str(), "add" | "length" | "get")
            ) {
                return self
                    .check_list_invocation(path, arguments, span)
                    .map(CheckedInvocation::Value);
            }
            if matches!(path.as_slice(), [range, create]
                if range == "Range" && create == "create")
            {
                return self
                    .check_range_create(arguments, span)
                    .map(CheckedInvocation::Value);
            }
            if let Some(checked) = self.check_standard_invocation(path, arguments, span) {
                return Some(CheckedInvocation::Call(checked));
            }
            if let Some(checked) =
                self.check_static_method_invocation(path, arguments, expected, span)
            {
                return Some(CheckedInvocation::Call(checked));
            }
            match self.lookup_union_case(path, callee.span()) {
                UnionCaseLookup::Found(definition, case) => {
                    return self
                        .check_union_case_call(&definition, &case, arguments, span)
                        .map(CheckedInvocation::Value);
                }
                UnionCaseLookup::Missing => return None,
                UnionCaseLookup::NotUnion => {}
            }
            match self.lookup_error_case(path, callee.span()) {
                ErrorCaseLookup::Found(definition, case) => {
                    return self
                        .check_error_case_call(&definition, &case, arguments, span)
                        .map(CheckedInvocation::Value);
                }
                ErrorCaseLookup::Missing => return None,
                ErrorCaseLookup::NotError => {}
            }
            let name = path.join(".");
            let resolution = self.resolver.database().resolve(
                self.module,
                &name,
                SymbolSpace::Value,
                callee.span(),
            );
            if let Some(symbol) = resolution.symbol()
                && let Some(signature) = self.signatures.get(&symbol).cloned()
                && !signature.type_parameters().is_empty()
            {
                return self
                    .check_inferred_generic_call(symbol, &signature, arguments, expected, span)
                    .map(CheckedInvocation::Call);
            }
        }
        let callee = self.check_expression(callee)?;
        let Some(SemanticType::Function {
            is_async,
            parameters,
            results,
            ..
        }) = self.resolver.arena().get(callee.type_id()).cloned()
        else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                callee.span(),
                "call",
                self.type_name(callee.type_id()),
            ));
            return None;
        };
        if parameters.len() != arguments.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "call",
                parameters.len(),
                arguments.len(),
            ));
            return None;
        }
        let resolved_parameter_types = match callee.kind() {
            TypedExpressionKind::Function(function) => self
                .signatures
                .get(function)
                .map(|signature| {
                    signature
                        .parameters()
                        .iter()
                        .map(|parameter| {
                            ExpectedExpressionType::resolved(parameter.parameter_type())
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };
        let mut typed_arguments = Vec::new();
        for (index, (argument, parameter_type)) in arguments.iter().zip(parameters).enumerate() {
            let expected = resolved_parameter_types
                .get(index)
                .copied()
                .flatten()
                .unwrap_or_else(|| ExpectedExpressionType::plain(parameter_type));
            let typed = self.check_expression_expected(argument, Some(expected))?;
            self.require_same_type(parameter_type, typed.type_id(), typed.span(), callee.span());
            typed_arguments.push(typed);
        }
        let dispatch = self.call_dispatch(callee);
        let results = self.call_result_types(is_async, results)?;
        Some(CheckedInvocation::Call(CheckedCall {
            call: TypedCall {
                dispatch,
                is_async,
                type_arguments: Vec::new(),
                arguments: typed_arguments,
                span,
            },
            results,
        }))
    }

    fn check_inferred_generic_call(
        &mut self,
        symbol: pop_foundation::SymbolId,
        signature: &crate::ResolvedFunctionSignature,
        arguments: &[ExpressionSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        if signature.parameters().len() != arguments.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                signature.name(),
                signature.parameters().len(),
                arguments.len(),
            ));
            return None;
        }

        let mut substitutions = BTreeMap::new();
        if let Some(expected) = expected
            && let [result] = signature.results()
            && !self.infer_type_pattern(result.type_id()?, expected.type_id, &mut substitutions)
        {
            self.diagnostics
                .push(type_diagnostics::generic_inference_failure(
                    span,
                    signature
                        .type_parameters()
                        .first()
                        .map_or("T", crate::ResolvedTypeParameter::name),
                    "expected result conflicts with call",
                ));
            return None;
        }

        let mut typed_arguments = Vec::with_capacity(arguments.len());
        for (argument, parameter) in arguments.iter().zip(signature.parameters()) {
            let typed = self.check_expression(argument)?;
            if !self.infer_type_pattern(
                parameter.parameter_type().type_id()?,
                typed.type_id(),
                &mut substitutions,
            ) {
                let parameter_name = signature
                    .type_parameters()
                    .iter()
                    .find(|parameter| substitutions.contains_key(&parameter.parameter()))
                    .map_or("T", crate::ResolvedTypeParameter::name);
                self.diagnostics
                    .push(type_diagnostics::generic_inference_failure(
                        argument.span(),
                        parameter_name,
                        "argument constraints conflict",
                    ));
                return None;
            }
            typed_arguments.push(typed);
        }

        for parameter in signature.type_parameters() {
            if let (Some(actual), Some(bound)) = (
                substitutions.get(&parameter.parameter()).copied(),
                parameter.bound(),
            ) && !self.infer_type_pattern(bound, actual, &mut substitutions)
            {
                self.diagnostics
                    .push(type_diagnostics::generic_inference_failure(
                        span,
                        parameter.name(),
                        "nominal interface bound is not satisfied",
                    ));
                return None;
            }
        }

        let mut type_arguments = Vec::with_capacity(signature.type_parameters().len());
        for parameter in signature.type_parameters() {
            let Some(argument) = substitutions.get(&parameter.parameter()).copied() else {
                self.diagnostics
                    .push(type_diagnostics::generic_inference_failure(
                        span,
                        parameter.name(),
                        "type argument is ambiguous",
                    ));
                return None;
            };
            type_arguments.push(argument);
        }
        let substitution_map: BTreeMap<_, _> = signature
            .type_parameters()
            .iter()
            .zip(&type_arguments)
            .map(|(parameter, argument)| (parameter.parameter(), *argument))
            .collect();
        self.resolver
            .materialize_class_instances_for_substitutions(&substitution_map)?;
        for parameter in signature.type_parameters() {
            if let Some(bound) = parameter.bound() {
                self.materialize_generic_bound_types(bound, &substitution_map)?;
            }
        }
        let parameter_types = signature
            .parameters()
            .iter()
            .map(|parameter| {
                self.resolver.substitute_type_parameters(
                    parameter.parameter_type().type_id()?,
                    &substitution_map,
                )
            })
            .collect::<Option<Vec<_>>>()?;
        for ((typed, expected), source) in
            typed_arguments.iter().zip(&parameter_types).zip(arguments)
        {
            self.require_same_type(*expected, typed.type_id(), typed.span(), source.span());
        }
        let results = signature
            .results()
            .iter()
            .map(|result| {
                self.resolver
                    .substitute_type_parameters(result.type_id()?, &substitution_map)
            })
            .collect::<Option<Vec<_>>>()?;
        let results = self.call_result_types(signature.is_async(), results)?;
        let dispatch = self
            .resolver
            .database()
            .index()
            .declaration(symbol)
            .and_then(pop_resolve::Declaration::reference_identity)
            .map_or(TypedCallDispatch::Direct { function: symbol }, |function| {
                TypedCallDispatch::Referenced { function }
            });
        Some(CheckedCall {
            call: TypedCall {
                dispatch,
                is_async: signature.is_async(),
                type_arguments,
                arguments: typed_arguments,
                span,
            },
            results,
        })
    }

    pub(crate) fn infer_type_pattern(
        &mut self,
        pattern: TypeId,
        actual: TypeId,
        substitutions: &mut BTreeMap<ParameterId, TypeId>,
    ) -> bool {
        if pattern == actual {
            return true;
        }
        let Some(pattern_type) = self.resolver.arena().get(pattern).cloned() else {
            return false;
        };
        if let SemanticType::TypeParameter(parameter) = pattern_type {
            return substitutions
                .get(&parameter)
                .is_none_or(|known| *known == actual)
                && {
                    substitutions.entry(parameter).or_insert(actual);
                    true
                };
        }
        let Some(actual_type) = self.resolver.arena().get(actual).cloned() else {
            return false;
        };

        if let SemanticType::Builtin {
            definition,
            arguments,
        } = &pattern_type
            && let Some(protocol) = self.resolver.schema().iteration_protocol()
            && *definition == protocol.iterable()
            && arguments.len() == 1
            && let Some(item) = self.iteration_item_type(actual, &actual_type)
        {
            return self.infer_type_pattern(arguments[0], item, substitutions);
        }

        match (pattern_type, actual_type) {
            (SemanticType::Primitive(left), SemanticType::Primitive(right)) => left == right,
            (SemanticType::Tuple(left), SemanticType::Tuple(right))
            | (SemanticType::Union(left), SemanticType::Union(right)) => {
                self.infer_type_lists(&left, &right, substitutions)
            }
            (
                SemanticType::Function {
                    is_async: left_async,
                    parameters: left_parameters,
                    results: left_results,
                    effects: left_effects,
                },
                SemanticType::Function {
                    is_async: right_async,
                    parameters: right_parameters,
                    results: right_results,
                    effects: right_effects,
                },
            ) => {
                left_async == right_async
                    && left_effects == right_effects
                    && self.infer_type_lists(&left_parameters, &right_parameters, substitutions)
                    && self.infer_type_lists(&left_results, &right_results, substitutions)
            }
            (SemanticType::Array(left), SemanticType::Array(right))
            | (SemanticType::Optional(left), SemanticType::Optional(right)) => {
                self.infer_type_pattern(left, right, substitutions)
            }
            (
                SemanticType::Table {
                    key: left_key,
                    value: left_value,
                },
                SemanticType::Table {
                    key: right_key,
                    value: right_value,
                },
            ) => {
                self.infer_type_pattern(left_key, right_key, substitutions)
                    && self.infer_type_pattern(left_value, right_value, substitutions)
            }
            (
                SemanticType::Class {
                    class: left,
                    arguments: left_arguments,
                },
                SemanticType::Class {
                    class: right,
                    arguments: right_arguments,
                },
            ) => {
                (left == right
                    || matches!(
                        (
                            self.resolver.class_source_identity(left),
                            self.resolver.class_source_identity(right),
                        ),
                        (Some(left), Some(right)) if left == right
                    ))
                    && self.infer_type_lists(&left_arguments, &right_arguments, substitutions)
            }
            (
                SemanticType::Interface {
                    interface: left,
                    arguments: left_arguments,
                },
                SemanticType::Interface {
                    interface: right,
                    arguments: right_arguments,
                },
            ) => {
                (left == right
                    || matches!(
                        (
                            self.resolver.interface_source_identity(left),
                            self.resolver.interface_source_identity(right),
                        ),
                        (Some(left), Some(right)) if left == right
                    ))
                    && self.infer_type_lists(&left_arguments, &right_arguments, substitutions)
            }
            (
                SemanticType::Interface {
                    interface: pattern_interface,
                    ..
                },
                SemanticType::Class { .. },
            ) => self
                .resolver
                .class_definition_for_type(actual)
                .and_then(|class| {
                    class.interfaces().iter().find(|implementation| {
                        self.resolver
                            .interface_definition_for_type(implementation.interface_type())
                            .is_some_and(|implemented| {
                                self.resolver
                                    .interface_source_identity(implemented.interface())
                                    == self.resolver.interface_source_identity(pattern_interface)
                            })
                    })
                })
                .map(|implementation| implementation.interface_type())
                .is_some_and(|implemented| {
                    self.infer_type_pattern(pattern, implemented, substitutions)
                }),
            (
                SemanticType::Builtin {
                    definition: left,
                    arguments: left_arguments,
                },
                SemanticType::Builtin {
                    definition: right,
                    arguments: right_arguments,
                },
            ) => {
                left == right
                    && self.infer_type_lists(&left_arguments, &right_arguments, substitutions)
            }
            (
                SemanticType::TaggedUnion {
                    source: left,
                    arguments: left_arguments,
                    ..
                },
                SemanticType::TaggedUnion {
                    source: right,
                    arguments: right_arguments,
                    ..
                },
            ) => {
                left == right
                    && self.infer_type_lists(&left_arguments, &right_arguments, substitutions)
            }
            (
                SemanticType::ErrorUnion {
                    source: left,
                    arguments: left_arguments,
                    ..
                },
                SemanticType::ErrorUnion {
                    source: right,
                    arguments: right_arguments,
                    ..
                },
            ) => {
                left == right
                    && self.infer_type_lists(&left_arguments, &right_arguments, substitutions)
            }
            (SemanticType::Record(left), SemanticType::Record(right)) => {
                left.len() == right.len()
                    && left
                        .iter()
                        .zip(right)
                        .all(|((left_name, left), (right_name, right))| {
                            left_name == &right_name
                                && self.infer_type_pattern(*left, right, substitutions)
                        })
            }
            (SemanticType::Enum { definition: left }, SemanticType::Enum { definition: right }) => {
                left == right
            }
            (SemanticType::Opaque(left), SemanticType::Opaque(right)) => left == right,
            (SemanticType::Error, SemanticType::Error) => true,
            _ => false,
        }
    }

    fn infer_type_lists(
        &mut self,
        patterns: &[TypeId],
        actual: &[TypeId],
        substitutions: &mut BTreeMap<ParameterId, TypeId>,
    ) -> bool {
        patterns.len() == actual.len()
            && patterns
                .iter()
                .zip(actual)
                .all(|(pattern, actual)| self.infer_type_pattern(*pattern, *actual, substitutions))
    }

    fn iteration_item_type(&mut self, actual: TypeId, semantic: &SemanticType) -> Option<TypeId> {
        let protocol = self.resolver.schema().iteration_protocol()?;
        match semantic {
            SemanticType::Array(element) => Some(*element),
            SemanticType::Table { key, value } => self
                .resolver
                .arena_mut()
                .intern(SemanticType::Tuple(vec![*key, *value]))
                .ok(),
            SemanticType::Builtin {
                definition,
                arguments,
            } if arguments.len() == 1
                && matches!(
                    *definition,
                    definition
                        if definition == protocol.list()
                            || definition == protocol.iterable()
                            || definition == protocol.iterator()
                ) =>
            {
                Some(arguments[0])
            }
            _ => {
                let _ = actual;
                None
            }
        }
    }

    pub(crate) fn materialize_generic_bound_types(
        &mut self,
        bound: TypeId,
        substitutions: &BTreeMap<ParameterId, TypeId>,
    ) -> Option<()> {
        let concrete = self
            .resolver
            .substitute_type_parameters(bound, substitutions)?;
        let protocol = self.resolver.schema().iteration_protocol()?;
        let SemanticType::Builtin {
            definition,
            arguments,
        } = self.resolver.arena().get(concrete)?.clone()
        else {
            return Some(());
        };
        if arguments.len() != 1
            || (definition != protocol.iterable() && definition != protocol.iterator())
        {
            return Some(());
        }
        for definition in [protocol.iterator(), protocol.iteration()] {
            self.resolver
                .arena_mut()
                .intern(SemanticType::Builtin {
                    definition,
                    arguments: arguments.clone(),
                })
                .ok()?;
        }
        Some(())
    }

    fn check_string_conversion(
        &mut self,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "string formatting",
                1,
                arguments.len(),
            ));
            return None;
        }
        let value = self.check_expression(&arguments[0])?;
        let string = self.resolver.arena().source_type("String")?;
        if value.type_id() == string {
            return Some(TypedExpression {
                type_id: string,
                span,
                ..value
            });
        }
        let kind = match self.resolver.arena().get(value.type_id()) {
            Some(SemanticType::Primitive(PrimitiveType::Boolean)) => StringFormatKind::Boolean,
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
                StringFormatKind::Integer(*kind)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
                StringFormatKind::Float(crate::FloatKind::Float32)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
                StringFormatKind::Float(crate::FloatKind::Float64)
            }
            _ => {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "string formatting",
                    self.type_name(value.type_id()),
                ));
                return None;
            }
        };
        Some(TypedExpression {
            kind: TypedExpressionKind::StringFormat {
                kind,
                value: Box::new(value),
            },
            type_id: string,
            span,
        })
    }

    fn check_numeric_conversion(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let [target_name] = path else {
            return None;
        };
        let target_type = self.resolver.arena().source_type(target_name)?;
        let target = self.numeric_target(target_type)?;
        if arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "numeric conversion",
                1,
                arguments.len(),
            ));
            return None;
        }
        let value = self.check_expression(&arguments[0])?;
        let Some(source) = self.numeric_target(value.type_id()) else {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                arguments[0].span(),
                "numeric value",
                self.type_name(value.type_id()),
                span,
            ));
            return None;
        };
        let conversion = match (source, target) {
            (
                crate::body_checking::NumericTarget::Integer(source),
                crate::body_checking::NumericTarget::Integer(target),
            ) => NumericConversionKind::IntegerToInteger { source, target },
            (
                crate::body_checking::NumericTarget::Integer(source),
                crate::body_checking::NumericTarget::Float(target),
            ) => NumericConversionKind::IntegerToFloat { source, target },
            (
                crate::body_checking::NumericTarget::Float(source),
                crate::body_checking::NumericTarget::Integer(target),
            ) => NumericConversionKind::FloatToInteger { source, target },
            (
                crate::body_checking::NumericTarget::Float(source),
                crate::body_checking::NumericTarget::Float(target),
            ) => NumericConversionKind::FloatToFloat { source, target },
        };
        Some(TypedExpression {
            kind: TypedExpressionKind::NumericConvert {
                value: Box::new(value),
                conversion,
            },
            type_id: target_type,
            span,
        })
    }

    pub(crate) fn call_dispatch(&self, callee: TypedExpression) -> TypedCallDispatch {
        let TypedExpressionKind::Function(function) = callee.kind() else {
            return TypedCallDispatch::Indirect {
                callee: Box::new(callee),
            };
        };
        let function = *function;
        self.resolver
            .database()
            .index()
            .declaration(function)
            .and_then(pop_resolve::Declaration::reference_identity)
            .map_or(TypedCallDispatch::Direct { function }, |identity| {
                TypedCallDispatch::Referenced { function: identity }
            })
    }

    pub(crate) fn check_array_create(
        &mut self,
        type_arguments: &[pop_syntax::TypeSyntax],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if type_arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_type_arity(
                span,
                "Array.create",
                1,
                type_arguments.len(),
            ));
            return None;
        }
        if arguments.len() != 2 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "Array.create",
                2,
                arguments.len(),
            ));
            return None;
        }
        let signature = self.signature_stack.last()?.clone();
        let (resolved, diagnostics) =
            self.resolver
                .resolve_annotation(self.module, &type_arguments[0], &signature);
        self.diagnostics.extend(diagnostics);
        let element_type = resolved?.type_id()?;
        let integer = self.resolver.arena().source_type("Int")?;
        let length = self.check_expression_expected(
            &arguments[0],
            Some(ExpectedExpressionType::plain(integer)),
        )?;
        self.require_same_type(
            integer,
            length.type_id(),
            length.span(),
            arguments[0].span(),
        );
        let initial_value = self.check_expression_expected(
            &arguments[1],
            Some(ExpectedExpressionType::plain(element_type)),
        )?;
        self.require_same_type(
            element_type,
            initial_value.type_id(),
            initial_value.span(),
            type_arguments[0].span(),
        );
        let array_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Array(element_type))
            .ok()?;
        Some(TypedExpression {
            kind: TypedExpressionKind::ArrayCreate {
                length: Box::new(length),
                initial_value: Box::new(initial_value),
            },
            type_id: array_type,
            span,
        })
    }

    pub(crate) fn check_array_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let operation = path.get(1)?.as_str();
        let expected_arity = if operation == "fill" {
            2
        } else {
            1 + usize::from(operation == "get")
        };
        if arguments.len() != expected_arity {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                format!("Array.{operation}"),
                expected_arity,
                arguments.len(),
            ));
            return None;
        }
        let array = self.check_expression(&arguments[0])?;
        let Some(SemanticType::Array(element_type)) =
            self.resolver.arena().get(array.type_id()).cloned()
        else {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                arguments[0].span(),
                "Array<T>",
                self.type_name(array.type_id()),
                span,
            ));
            return None;
        };
        match operation {
            "length" => Some(TypedExpression {
                kind: TypedExpressionKind::ArrayLength {
                    array: Box::new(array),
                },
                type_id: self.resolver.arena().source_type("Int")?,
                span,
            }),
            "get" => {
                let integer = self.resolver.arena().source_type("Int")?;
                let index = self.check_expression_expected(
                    &arguments[1],
                    Some(ExpectedExpressionType::plain(integer)),
                )?;
                self.require_same_type(integer, index.type_id(), index.span(), arguments[1].span());
                Some(TypedExpression {
                    kind: TypedExpressionKind::ArrayGetChecked {
                        array: Box::new(array),
                        index: Box::new(index),
                    },
                    type_id: element_type,
                    span,
                })
            }
            "fill" => {
                let value = self.check_expression_expected(
                    &arguments[1],
                    Some(ExpectedExpressionType::plain(element_type)),
                )?;
                self.require_same_type(
                    element_type,
                    value.type_id(),
                    value.span(),
                    arguments[1].span(),
                );
                Some(TypedExpression {
                    kind: TypedExpressionKind::ArrayFill {
                        array: Box::new(array),
                        value: Box::new(value),
                    },
                    type_id: self.resolver.arena().source_type("nil")?,
                    span,
                })
            }
            _ => None,
        }
    }

    pub(crate) fn check_list_create(
        &mut self,
        path: &[String],
        type_arguments: &[pop_syntax::TypeSyntax],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let operation = path.get(1)?.as_str();
        if type_arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_type_arity(
                span,
                format!("List.{operation}"),
                1,
                type_arguments.len(),
            ));
            return None;
        }
        let expected_arguments = usize::from(operation == "withCapacity");
        if arguments.len() != expected_arguments {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                format!("List.{operation}"),
                expected_arguments,
                arguments.len(),
            ));
            return None;
        }
        let signature = self.signature_stack.last()?.clone();
        let (resolved, diagnostics) =
            self.resolver
                .resolve_annotation(self.module, &type_arguments[0], &signature);
        self.diagnostics.extend(diagnostics);
        let element_type = resolved?.type_id()?;
        let protocol = self.resolver.schema().iteration_protocol()?;
        let list_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: protocol.list(),
                arguments: vec![element_type],
            })
            .ok()?;
        let capacity = if operation == "withCapacity" {
            let integer = self.resolver.arena().source_type("Int")?;
            let value = self.check_expression_expected(
                &arguments[0],
                Some(ExpectedExpressionType::plain(integer)),
            )?;
            self.require_same_type(integer, value.type_id(), value.span(), arguments[0].span());
            Some(Box::new(value))
        } else {
            None
        };
        Some(TypedExpression {
            kind: TypedExpressionKind::ListCreate { capacity },
            type_id: list_type,
            span,
        })
    }

    pub(crate) fn check_range_create(
        &mut self,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if !matches!(arguments.len(), 2 | 3) {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "Range.create",
                2,
                arguments.len(),
            ));
            return None;
        }
        let first = self.check_expression(&arguments[0])?;
        let integer_type = first.type_id();
        let Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) =
            self.resolver.arena().get(integer_type).cloned()
        else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                first.span(),
                "Range.create",
                self.type_name(integer_type),
            ));
            return None;
        };
        let last = self.check_expression_expected(
            &arguments[1],
            Some(ExpectedExpressionType::plain(integer_type)),
        )?;
        self.require_same_type(
            integer_type,
            last.type_id(),
            last.span(),
            arguments[1].span(),
        );
        let step = if let Some(argument) = arguments.get(2) {
            let step = self.check_expression_expected(
                argument,
                Some(ExpectedExpressionType::plain(integer_type)),
            )?;
            self.require_same_type(integer_type, step.type_id(), step.span(), argument.span());
            step
        } else {
            TypedExpression {
                kind: TypedExpressionKind::Integer(
                    crate::IntegerValue::parse_decimal("1", kind)
                        .expect("one fits every integer range"),
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
                "Range.create step",
                "zero",
            ));
            return None;
        }
        let protocol = self.resolver.schema().iteration_protocol()?;
        let range_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: protocol.range(),
                arguments: vec![integer_type],
            })
            .ok()?;
        Some(TypedExpression {
            kind: TypedExpressionKind::RangeCreate {
                first: Box::new(first),
                last: Box::new(last),
                step: Box::new(step),
            },
            type_id: range_type,
            span,
        })
    }

    pub(crate) fn check_list_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let operation = path.get(1)?.as_str();
        let expected_arity = 1 + usize::from(matches!(operation, "add" | "get"));
        if arguments.len() != expected_arity {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                format!("List.{operation}"),
                expected_arity,
                arguments.len(),
            ));
            return None;
        }
        let list = self.check_expression(&arguments[0])?;
        let protocol = self.resolver.schema().iteration_protocol()?;
        let Some(SemanticType::Builtin {
            definition,
            arguments: list_arguments,
        }) = self.resolver.arena().get(list.type_id()).cloned()
        else {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                arguments[0].span(),
                "List<T>",
                self.type_name(list.type_id()),
                span,
            ));
            return None;
        };
        if definition != protocol.list() || list_arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                arguments[0].span(),
                "List<T>",
                self.type_name(list.type_id()),
                span,
            ));
            return None;
        }
        let element_type = list_arguments[0];
        match operation {
            "length" => Some(TypedExpression {
                kind: TypedExpressionKind::ListLength {
                    list: Box::new(list),
                },
                type_id: self.resolver.arena().source_type("Int")?,
                span,
            }),
            "get" => {
                let integer = self.resolver.arena().source_type("Int")?;
                let index = self.check_expression_expected(
                    &arguments[1],
                    Some(ExpectedExpressionType::plain(integer)),
                )?;
                self.require_same_type(integer, index.type_id(), index.span(), arguments[1].span());
                Some(TypedExpression {
                    kind: TypedExpressionKind::ListGetChecked {
                        list: Box::new(list),
                        index: Box::new(index),
                    },
                    type_id: element_type,
                    span,
                })
            }
            "add" => {
                let active = match list.kind() {
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
                if active.is_some_and(|active| self.active_collection_iterations.contains(&active))
                {
                    self.diagnostics
                        .push(type_diagnostics::structural_mutation_during_iteration(
                            span, "List.add",
                        ));
                }
                let value = self.check_expression_expected(
                    &arguments[1],
                    Some(ExpectedExpressionType::plain(element_type)),
                )?;
                self.require_same_type(
                    element_type,
                    value.type_id(),
                    value.span(),
                    arguments[1].span(),
                );
                Some(TypedExpression {
                    kind: TypedExpressionKind::ListAdd {
                        list: Box::new(list),
                        value: Box::new(value),
                    },
                    type_id: self.resolver.arena().source_type("nil")?,
                    span,
                })
            }
            _ => None,
        }
    }

    fn check_iteration_item_invocation(
        &mut self,
        arguments: &[ExpressionSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "Iteration.Item",
                1,
                arguments.len(),
            ));
            return None;
        }
        let protocol = self.resolver.schema().iteration_protocol()?;
        let expected_item =
            expected.and_then(
                |expected| match self.resolver.arena().get(expected.type_id) {
                    Some(SemanticType::Builtin {
                        definition,
                        arguments,
                    }) if *definition == protocol.iteration() && arguments.len() == 1 => {
                        Some(arguments[0])
                    }
                    _ => None,
                },
            );
        let value = self.check_expression_expected(
            &arguments[0],
            expected_item.map(ExpectedExpressionType::plain),
        )?;
        if let Some(expected_item) = expected_item {
            self.require_same_type(
                expected_item,
                value.type_id(),
                value.span(),
                arguments[0].span(),
            );
        }
        let iteration_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: protocol.iteration(),
                arguments: vec![value.type_id()],
            })
            .ok()?;
        if let Some(expected) = expected {
            self.require_same_type(expected.type_id, iteration_type, span, span);
        }
        Some(TypedExpression {
            kind: TypedExpressionKind::IterationCase {
                iteration: protocol.iteration(),
                case: protocol.item_case(),
                arguments: vec![value],
            },
            type_id: iteration_type,
            span,
        })
    }

    pub(crate) fn check_standard_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        let [name] = path else {
            return None;
        };
        if self.binding_by_name(name).is_some() {
            return None;
        }
        if self
            .resolver
            .database()
            .resolve(self.module, name, SymbolSpace::Value, span)
            .symbol()
            .is_some()
        {
            return None;
        }
        let entries: Vec<_> = self
            .resolver
            .schema()
            .standard_functions_by_source_name(name)
            .map(|entry| {
                (
                    entry.id(),
                    entry.parameter_types().to_vec(),
                    entry.result_types().to_vec(),
                )
            })
            .collect();
        let first = entries.first()?;
        let arity_candidates: Vec<_> = entries
            .iter()
            .filter(|(_, parameters, _)| parameters.len() == arguments.len())
            .collect();
        if arity_candidates.is_empty() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "call",
                first.1.len(),
                arguments.len(),
            ));
            return None;
        }
        let mut typed_arguments = Vec::with_capacity(arguments.len());
        for argument in arguments {
            typed_arguments.push(self.check_expression(argument)?);
        }
        let argument_types: Vec<_> = typed_arguments
            .iter()
            .map(TypedExpression::type_id)
            .collect();
        let candidates: Vec<_> = arity_candidates
            .into_iter()
            .filter_map(|(function, parameter_names, result_names)| {
                let parameter_types = parameter_names
                    .iter()
                    .map(|name| self.resolver.arena().source_type(name))
                    .collect::<Option<Vec<_>>>()?;
                let result_types = result_names
                    .iter()
                    .map(|name| self.resolver.arena().source_type(name))
                    .collect::<Option<Vec<_>>>()?;
                Some((*function, parameter_types, result_types))
            })
            .collect();
        let Some((function, _, result_types)) = candidates
            .iter()
            .find(|(_, parameter_types, _)| *parameter_types == argument_types)
        else {
            if let Some((_, expected_types, _)) = candidates.first() {
                for (argument, expected) in typed_arguments.iter().zip(expected_types) {
                    self.require_same_type(*expected, argument.type_id(), argument.span(), span);
                }
            }
            return None;
        };
        Some(CheckedCall {
            call: TypedCall {
                dispatch: TypedCallDispatch::Standard {
                    function: *function,
                },
                is_async: false,
                type_arguments: Vec::new(),
                arguments: typed_arguments,
                span,
            },
            results: result_types.clone(),
        })
    }

    pub(crate) fn check_static_method_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        let (method_name, class_path) = path.split_last()?;
        if class_path.is_empty() {
            return None;
        }
        let resolution = self.resolver.database().resolve(
            self.module,
            &class_path.join("."),
            SymbolSpace::Type,
            span,
        );
        let definition = resolution
            .symbol()
            .and_then(|symbol| self.resolver.class_definition(symbol))?
            .clone();
        let method = definition
            .methods()
            .iter()
            .find(|method| {
                method.name() == method_name
                    && method.dispatch() == crate::ClassMethodDispatch::Static
            })?
            .clone();
        if !self.can_access_class_member(&definition, method.visibility()) {
            self.diagnostics
                .push(resolution_diagnostics::inaccessible_name(
                    span,
                    method.name(),
                    method.span(),
                ));
            return None;
        }
        if !definition.type_parameters().is_empty() {
            let signature = self.resolver.method_signature(&definition, &method);
            let inferred = self.check_inferred_generic_call(
                definition.symbol(),
                &signature,
                arguments,
                expected,
                span,
            )?;
            let symbolic = inferred
                .call
                .type_arguments
                .iter()
                .any(|argument| self.resolver.arena().contains_type_parameter(*argument));
            let instance = self
                .resolver
                .instantiate_class(definition.symbol(), &inferred.call.type_arguments)?;
            let instance_method = instance.methods().iter().find(|candidate| {
                candidate.name() == method.name()
                    && candidate.dispatch() == crate::ClassMethodDispatch::Static
            })?;
            if symbolic {
                return Some(CheckedCall {
                    call: TypedCall {
                        dispatch: TypedCallDispatch::DirectMethod {
                            method: method.method(),
                            receiver: None,
                        },
                        is_async: false,
                        type_arguments: Vec::new(),
                        arguments: inferred.call.arguments,
                        span,
                    },
                    results: instance_method.results().to_vec(),
                });
            }
            return Some(CheckedCall {
                call: TypedCall {
                    dispatch: TypedCallDispatch::DirectMethod {
                        method: instance_method.method(),
                        receiver: None,
                    },
                    is_async: false,
                    type_arguments: Vec::new(),
                    arguments: inferred.call.arguments,
                    span,
                },
                results: instance_method.results().to_vec(),
            });
        }
        self.check_direct_method_invocation(&method, None, arguments, span)
    }

    pub(crate) fn check_receiver_method_call(
        &mut self,
        receiver: &ExpressionSyntax,
        method_name: &str,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let checked =
            self.check_receiver_method_invocation(receiver, method_name, arguments, span)?;
        self.checked_call_expression(checked)
    }

    pub(crate) fn check_receiver_method_invocation(
        &mut self,
        receiver: &ExpressionSyntax,
        method_name: &str,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        let receiver = self.check_expression(receiver)?;
        if let Some((interface, item_type)) = self.builtin_iteration_interface(receiver.type_id()) {
            let protocol = self.resolver.schema().iteration_protocol()?;
            let (method, result_definition) = match (interface, method_name) {
                (candidate, "iterator")
                    if candidate == protocol.iterable() || candidate == protocol.iterator() =>
                {
                    (protocol.iterator_method(), protocol.iterator())
                }
                (candidate, "next") if candidate == protocol.iterator() => {
                    (protocol.next_method(), protocol.iteration())
                }
                _ => {
                    self.diagnostics
                        .push(type_diagnostics::unknown_record_field(span, method_name));
                    return None;
                }
            };
            if !arguments.is_empty() {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    span,
                    "iteration protocol method call",
                    0,
                    arguments.len(),
                ));
                return None;
            }
            let result = self
                .resolver
                .arena_mut()
                .intern(SemanticType::Builtin {
                    definition: result_definition,
                    arguments: vec![item_type],
                })
                .ok()?;
            return Some(CheckedCall {
                call: TypedCall {
                    dispatch: TypedCallDispatch::BuiltinInterfaceMethod {
                        interface,
                        method,
                        receiver: Box::new(receiver),
                    },
                    is_async: false,
                    type_arguments: Vec::new(),
                    arguments: Vec::new(),
                    span,
                },
                results: vec![result],
            });
        }
        let interface = self
            .resolver
            .interface_definition_for_type(receiver.type_id())
            .cloned()
            .or_else(|| {
                let SemanticType::TypeParameter(parameter) =
                    self.resolver.arena().get(receiver.type_id())?
                else {
                    return None;
                };
                let bound = self
                    .signature_stack
                    .last()?
                    .type_parameters()
                    .iter()
                    .find(|candidate| candidate.parameter() == *parameter)?
                    .bound()?;
                self.resolver.interface_definition_for_type(bound).cloned()
            });
        if let Some(interface) = interface {
            let Some(method) = interface
                .methods()
                .iter()
                .find(|method| method.name() == method_name)
                .cloned()
            else {
                self.diagnostics
                    .push(type_diagnostics::unknown_record_field(span, method_name));
                return None;
            };
            return self
                .check_interface_method_invocation(&interface, &method, receiver, arguments, span);
        }
        let Some(definition) = self
            .resolver
            .class_definition_for_type(receiver.type_id())
            .cloned()
        else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "method call",
                self.type_name(receiver.type_id()),
            ));
            return None;
        };
        let Some(method) = definition
            .methods()
            .iter()
            .find(|method| {
                method.name() == method_name
                    && method.dispatch() == crate::ClassMethodDispatch::Receiver
            })
            .cloned()
        else {
            self.diagnostics
                .push(type_diagnostics::unknown_record_field(span, method_name));
            return None;
        };
        if !self.can_access_class_member(&definition, method.visibility()) {
            self.diagnostics
                .push(resolution_diagnostics::inaccessible_name(
                    span,
                    method.name(),
                    method.span(),
                ));
            return None;
        }
        self.check_direct_method_invocation(&method, Some(receiver), arguments, span)
    }

    pub(crate) fn check_interface_method_invocation(
        &mut self,
        interface: &crate::InterfaceDefinition,
        method: &crate::InterfaceMethodDefinition,
        receiver: TypedExpression,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        if method.parameters().len() != arguments.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "interface method call",
                method.parameters().len(),
                arguments.len(),
            ));
            return None;
        }
        let mut typed_arguments = Vec::new();
        for (argument, (_, parameter_type, parameter_span)) in
            arguments.iter().zip(method.parameters())
        {
            let typed = self.check_expression_expected(
                argument,
                Some(ExpectedExpressionType::plain(*parameter_type)),
            )?;
            self.require_same_type(
                *parameter_type,
                typed.type_id(),
                typed.span(),
                *parameter_span,
            );
            typed_arguments.push(typed);
        }
        Some(CheckedCall {
            call: TypedCall {
                dispatch: TypedCallDispatch::InterfaceMethod {
                    interface: interface.interface(),
                    method: method.method(),
                    receiver: Box::new(receiver),
                },
                is_async: false,
                type_arguments: Vec::new(),
                arguments: typed_arguments,
                span,
            },
            results: method.results().to_vec(),
        })
    }

    pub(crate) fn check_direct_method_invocation(
        &mut self,
        method: &crate::ClassMethodDefinition,
        receiver: Option<TypedExpression>,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        if method.parameters().len() != arguments.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "method call",
                method.parameters().len(),
                arguments.len(),
            ));
            return None;
        }
        let mut typed_arguments = Vec::new();
        for (argument, (_, parameter_type, parameter_span)) in
            arguments.iter().zip(method.parameters())
        {
            let typed = self.check_expression_expected(
                argument,
                Some(ExpectedExpressionType::plain(*parameter_type)),
            )?;
            self.require_same_type(
                *parameter_type,
                typed.type_id(),
                typed.span(),
                *parameter_span,
            );
            typed_arguments.push(typed);
        }
        Some(CheckedCall {
            call: TypedCall {
                dispatch: TypedCallDispatch::DirectMethod {
                    method: method.method(),
                    receiver: receiver.map(Box::new),
                },
                is_async: false,
                type_arguments: Vec::new(),
                arguments: typed_arguments,
                span,
            },
            results: method.results().to_vec(),
        })
    }

    pub(crate) fn checked_call_expression(
        &mut self,
        checked: CheckedCall,
    ) -> Option<TypedExpression> {
        if checked.results.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                checked.call.span,
                "call expression result",
                1,
                checked.results.len(),
            ));
            return None;
        }
        let result_type = checked.results[0];
        let TypedCall {
            dispatch,
            is_async,
            type_arguments,
            arguments,
            span,
        } = checked.call;
        let kind = match dispatch {
            TypedCallDispatch::Standard { function } => TypedExpressionKind::StandardCall {
                function,
                arguments,
            },
            TypedCallDispatch::Direct { function } => TypedExpressionKind::DirectCall {
                function,
                is_async,
                type_arguments,
                arguments,
            },
            TypedCallDispatch::Referenced { function } => TypedExpressionKind::ReferencedCall {
                function,
                is_async,
                type_arguments,
                arguments,
            },
            TypedCallDispatch::DirectMethod { method, receiver } => {
                TypedExpressionKind::DirectMethodCall {
                    method,
                    receiver,
                    arguments,
                }
            }
            TypedCallDispatch::InterfaceMethod {
                interface,
                method,
                receiver,
            } => TypedExpressionKind::InterfaceMethodCall {
                interface,
                method,
                receiver,
                arguments,
            },
            TypedCallDispatch::BuiltinInterfaceMethod {
                interface,
                method,
                receiver,
            } => TypedExpressionKind::BuiltinInterfaceMethodCall {
                interface,
                method,
                receiver,
                arguments,
            },
            TypedCallDispatch::Indirect { callee } => TypedExpressionKind::IndirectCall {
                callee,
                is_async,
                arguments,
            },
        };
        Some(TypedExpression {
            kind,
            type_id: result_type,
            span,
        })
    }
}

impl BodyChecker<'_, '_> {
    fn builtin_iteration_interface(&self, type_id: TypeId) -> Option<(BuiltinTypeId, TypeId)> {
        let semantic = self.resolver.arena().get(type_id)?;
        let bound = if let SemanticType::TypeParameter(parameter) = semantic {
            self.signature_stack
                .last()?
                .type_parameters()
                .iter()
                .find(|candidate| candidate.parameter() == *parameter)?
                .bound()?
        } else {
            type_id
        };
        let SemanticType::Builtin {
            definition,
            arguments,
        } = self.resolver.arena().get(bound)?
        else {
            return None;
        };
        let protocol = self.resolver.schema().iteration_protocol()?;
        ((*definition == protocol.iterable() || *definition == protocol.iterator())
            && arguments.len() == 1)
            .then_some((*definition, arguments[0]))
    }
}
