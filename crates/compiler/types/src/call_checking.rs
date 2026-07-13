//! Typed call, method, interface, and compiler-known invocation selection.
//!
//! Every call leaves this module with a closed static dispatch category,
//! exact argument/result types, and no unknown-effect or runtime lookup path.

use pop_diagnostics::{resolution as resolution_diagnostics, types as type_diagnostics};
use pop_foundation::SourceSpan;
use pop_resolve::SymbolSpace;
use pop_syntax::{ExpressionSyntax, ExpressionSyntaxKind};

use crate::body_checking::{
    BodyChecker, CheckedCall, CheckedInvocation, ExpectedExpressionType, UnionCaseLookup,
};
use crate::typed_body::*;
use crate::{NumericConversionKind, PrimitiveType, SemanticType, StringFormatKind};

impl<'resolver, 'index> BodyChecker<'resolver, 'index> {
    pub(crate) fn check_call(
        &mut self,
        callee: &ExpressionSyntax,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        match self.check_call_invocation(callee, arguments, span)? {
            CheckedInvocation::Call(checked) => self.checked_call_expression(checked),
            CheckedInvocation::Value(value) => Some(value),
        }
    }

    pub(crate) fn check_call_invocation(
        &mut self,
        callee: &ExpressionSyntax,
        arguments: &[ExpressionSyntax],
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
            if let Some(checked) = self.check_standard_invocation(path, arguments, span) {
                return Some(CheckedInvocation::Call(checked));
            }
            if let Some(checked) = self.check_static_method_invocation(path, arguments, span) {
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
        }
        let callee = self.check_expression(callee)?;
        let Some(SemanticType::Function {
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
        Some(CheckedInvocation::Call(CheckedCall {
            call: TypedCall {
                dispatch,
                arguments: typed_arguments,
                span,
            },
            results,
        }))
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
        if let Some(interface) = self
            .resolver
            .interface_definition_for_type(receiver.type_id())
            .cloned()
        {
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
                arguments,
            },
            TypedCallDispatch::Referenced { function } => TypedExpressionKind::ReferencedCall {
                function,
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
            TypedCallDispatch::Indirect { callee } => {
                TypedExpressionKind::IndirectCall { callee, arguments }
            }
        };
        Some(TypedExpression {
            kind,
            type_id: result_type,
            span,
        })
    }
}
