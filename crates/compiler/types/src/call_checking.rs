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

#[derive(Clone, Copy)]
enum FfiBufferElementKind {
    Scalar,
    LayoutRecord(pop_foundation::SymbolId),
}

impl FfiBufferElementKind {
    const fn layout_record(self) -> Option<pop_foundation::SymbolId> {
        match self {
            Self::Scalar => None,
            Self::LayoutRecord(record) => Some(record),
        }
    }
}

enum FfiPointerOperationKind {
    ToOptional,
    ReadOnly,
    IsPresent,
    Require,
}

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
            if matches!(path.as_slice(), [ffi, handle, operation]
                if ffi == "Ffi" && handle == "Handle"
                    && matches!(operation.as_str(), "open" | "get" | "close"))
                && self.resolver.has_ffi_dependency()
                && self
                    .resolver
                    .database()
                    .resolve(
                        self.module,
                        &path.join("."),
                        SymbolSpace::Value,
                        callee.span(),
                    )
                    .symbol()
                    .is_none()
            {
                return self
                    .check_ffi_handle_invocation(path, arguments, None, span)
                    .map(CheckedInvocation::Value);
            }
            if matches!(path.as_slice(), [ffi, operation]
                if ffi == "Ffi" && operation == "withPin")
                && self.resolver.has_ffi_dependency()
                && self
                    .resolver
                    .database()
                    .resolve(
                        self.module,
                        &path.join("."),
                        SymbolSpace::Value,
                        callee.span(),
                    )
                    .symbol()
                    .is_none()
            {
                return self
                    .check_ffi_with_pin_invocation(arguments, span)
                    .map(CheckedInvocation::Value);
            }
            if matches!(path.as_slice(), [ffi, buffer, operation]
                if ffi == "Ffi" && buffer == "Buffer"
                    && matches!(operation.as_str(), "length" | "read" | "write" | "close" | "withPointer"))
                && self.resolver.has_ffi_dependency()
                && self
                    .resolver
                    .database()
                    .resolve(
                        self.module,
                        &path.join("."),
                        SymbolSpace::Value,
                        callee.span(),
                    )
                    .symbol()
                    .is_none()
            {
                return self
                    .check_ffi_buffer_invocation(path, arguments, None, span)
                    .map(CheckedInvocation::Value);
            }
            if is_ffi_pointer_operation(path)
                && self.resolver.has_ffi_dependency()
                && self
                    .resolver
                    .database()
                    .resolve(
                        self.module,
                        &path.join("."),
                        SymbolSpace::Value,
                        callee.span(),
                    )
                    .symbol()
                    .is_none()
            {
                return self
                    .check_ffi_pointer_invocation(path, arguments, None, span)
                    .map(CheckedInvocation::Value);
            }
            if is_ffi_unsafe_operation(path)
                && self.resolver.has_ffi_dependency()
                && self
                    .resolver
                    .database()
                    .resolve(
                        self.module,
                        &path.join("."),
                        SymbolSpace::Value,
                        callee.span(),
                    )
                    .symbol()
                    .is_none()
            {
                return self
                    .check_ffi_unsafe_invocation(path, arguments, None, span)
                    .map(CheckedInvocation::Value);
            }
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
            if matches!(path.as_slice(), [task, operation]
            if task == "Task"
                && matches!(
                    operation.as_str(),
                    "cancellationSource" | "cancelToken" | "cancel" | "group" | "start"
                ))
            {
                return self
                    .check_task_invocation(path, arguments, span)
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
            if resolution.symbols().len() > 1 {
                return self
                    .check_exact_source_overload(&name, resolution.symbols(), arguments, span)
                    .map(CheckedInvocation::Call);
            }
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

    pub(crate) fn check_ffi_handle_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        explicit_payload: Option<TypeId>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let operation = path.get(2)?.as_str();
        if arguments.len() != 1 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                format!("Ffi.Handle.{operation}"),
                1,
                arguments.len(),
            ));
            return None;
        }
        let argument = self.check_expression(&arguments[0])?;
        let payload = if operation == "open" {
            argument.type_id()
        } else {
            let Some(SemanticType::Builtin {
                definition,
                arguments,
            }) = self.resolver.arena().get(argument.type_id())
            else {
                self.diagnostics.push(type_diagnostics::type_mismatch(
                    argument.span(),
                    "Ffi.Handle<T>",
                    self.type_name(argument.type_id()),
                    span,
                ));
                return None;
            };
            if *definition != crate::FFI_HANDLE_TYPE_ID || arguments.len() != 1 {
                self.diagnostics.push(type_diagnostics::type_mismatch(
                    argument.span(),
                    "Ffi.Handle<T>",
                    self.type_name(argument.type_id()),
                    span,
                ));
                return None;
            }
            arguments[0]
        };
        if explicit_payload.is_some_and(|expected| expected != payload) {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                argument.span(),
                self.type_name(explicit_payload?),
                self.type_name(payload),
                span,
            ));
            return None;
        }
        if !self.resolver.ffi_handle_payload_is_valid(payload) {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                argument.span(),
                "managed reference",
                self.type_name(payload),
                span,
            ));
            return None;
        }
        let handle_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: crate::FFI_HANDLE_TYPE_ID,
                arguments: vec![payload],
            })
            .ok()?;
        let type_id = match operation {
            "open" => handle_type,
            "get" => payload,
            "close" => self.resolver.arena().source_type("nil")?,
            _ => return None,
        };
        let argument = Box::new(argument);
        let kind = match operation {
            "open" => TypedExpressionKind::FfiHandleOpen { value: argument },
            "get" => TypedExpressionKind::FfiHandleGet { handle: argument },
            "close" => TypedExpressionKind::FfiHandleClose { handle: argument },
            _ => return None,
        };
        Some(TypedExpression {
            kind,
            type_id,
            span,
        })
    }

    pub(crate) fn check_ffi_buffer_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        explicit_element: Option<(TypeId, Option<pop_foundation::SymbolId>)>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let operation = path.get(2)?.as_str();
        let expected_arity = match operation {
            "open" | "length" | "close" => 1,
            "withPointer" => 2,
            "read" => 2,
            "write" => 3,
            _ => return None,
        };
        if arguments.len() != expected_arity {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                format!("Ffi.Buffer.{operation}"),
                expected_arity,
                arguments.len(),
            ));
            return None;
        }
        let size = self.ffi_builtin_type("Ffi.C.Size", Vec::new())?;
        if operation == "open" {
            let (element, record) = explicit_element?;
            let Some(element_kind) = self.ffi_buffer_element(element, record) else {
                self.diagnostics.push(type_diagnostics::type_mismatch(
                    span,
                    "FFI ABI storage type",
                    self.type_name(element),
                    span,
                ));
                return None;
            };
            let layout_record = match element_kind {
                FfiBufferElementKind::Scalar => None,
                FfiBufferElementKind::LayoutRecord(record) => Some(record),
            };
            let length = self.check_expression_expected(
                &arguments[0],
                Some(ExpectedExpressionType::plain(size)),
            )?;
            self.require_same_type(size, length.type_id(), length.span(), span);
            let buffer = self.ffi_builtin_type("Ffi.Buffer", vec![element])?;
            let allocation_error = self.ffi_builtin_type("Ffi.AllocationError", Vec::new())?;
            let type_id = self.resolver.result_type(buffer, allocation_error)?;
            return Some(TypedExpression {
                kind: TypedExpressionKind::FfiBufferOpen {
                    length: Box::new(length),
                    element,
                    layout_record,
                },
                type_id,
                span,
            });
        }

        let buffer = self.check_expression(&arguments[0])?;
        let element = self.ffi_buffer_payload(buffer.type_id()).or_else(|| {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                buffer.span(),
                "Ffi.Buffer<T>",
                self.type_name(buffer.type_id()),
                span,
            ));
            None
        })?;
        if explicit_element.is_some_and(|(expected, _)| expected != element) {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                buffer.span(),
                self.type_name(explicit_element?.0),
                self.type_name(element),
                span,
            ));
            return None;
        }
        let element_kind = self.ffi_buffer_element(element, None).or_else(|| {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                span,
                "FFI ABI storage type",
                self.type_name(element),
                span,
            ));
            None
        })?;
        let layout_record = element_kind.layout_record();
        if operation == "withPointer" {
            let body_expression = self.check_expression(&arguments[1])?;
            let body_type = body_expression.type_id();
            let TypedExpressionKind::Closure(body) = body_expression.kind else {
                self.diagnostics.push(type_diagnostics::type_mismatch(
                    arguments[1].span(),
                    "immediate non-async inline closure",
                    self.type_name(body_expression.type_id()),
                    span,
                ));
                return None;
            };
            let optional_pointer = self.ffi_builtin_type("Ffi.OptionalPointer", vec![element])?;
            let valid_shape = !body.is_async()
                && matches!(body.parameters(), [pointer, length]
                    if pointer.type_id() == optional_pointer && length.type_id() == size)
                && body.results().len() == 1;
            if !valid_shape || !scoped_borrow_body_is_valid(&body, self.signatures) {
                self.diagnostics.push(type_diagnostics::type_mismatch(
                    arguments[1].span(),
                    "non-escaping scoped FFI borrow body",
                    "incompatible closure",
                    span,
                ));
                return None;
            }
            return Some(TypedExpression {
                type_id: body.results()[0],
                kind: TypedExpressionKind::FfiBufferWithPointer {
                    buffer: Box::new(buffer),
                    body,
                    body_type,
                    element,
                    layout_record,
                    region: {
                        let region =
                            pop_foundation::BorrowRegionId::from_raw(self.next_borrow_region);
                        self.next_borrow_region = self.next_borrow_region.saturating_add(1);
                        region
                    },
                },
                span,
            });
        }
        let buffer = Box::new(buffer);
        let nil = self.resolver.arena().source_type("nil")?;
        let (kind, type_id) = match operation {
            "length" => (TypedExpressionKind::FfiBufferLength { buffer }, size),
            "read" => {
                let index = self.check_expression_expected(
                    &arguments[1],
                    Some(ExpectedExpressionType::plain(size)),
                )?;
                self.require_same_type(size, index.type_id(), index.span(), span);
                (
                    TypedExpressionKind::FfiBufferRead {
                        buffer,
                        index: Box::new(index),
                    },
                    element,
                )
            }
            "write" => {
                let index = self.check_expression_expected(
                    &arguments[1],
                    Some(ExpectedExpressionType::plain(size)),
                )?;
                let value = self.check_expression_expected(
                    &arguments[2],
                    Some(ExpectedExpressionType::plain(element)),
                )?;
                self.require_same_type(size, index.type_id(), index.span(), span);
                self.require_same_type(element, value.type_id(), value.span(), span);
                (
                    TypedExpressionKind::FfiBufferWrite {
                        buffer,
                        index: Box::new(index),
                        value: Box::new(value),
                    },
                    nil,
                )
            }
            "close" => (TypedExpressionKind::FfiBufferClose { buffer }, nil),
            _ => return None,
        };
        Some(TypedExpression {
            kind,
            type_id,
            span,
        })
    }

    fn check_ffi_with_pin_invocation(
        &mut self,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if arguments.len() != 2 {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "Ffi.withPin",
                2,
                arguments.len(),
            ));
            return None;
        }
        let bytes_type = self.ffi_builtin_type("Bytes", Vec::new())?;
        let byte = self.resolver.arena().source_type("Byte")?;
        let size = self.ffi_builtin_type("Ffi.C.Size", Vec::new())?;
        let bytes = self.check_expression_expected(
            &arguments[0],
            Some(ExpectedExpressionType::plain(bytes_type)),
        )?;
        self.require_same_type(bytes_type, bytes.type_id(), bytes.span(), span);
        let body_expression = self.check_expression(&arguments[1])?;
        let body_type = body_expression.type_id();
        let TypedExpressionKind::Closure(body) = body_expression.kind else {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                arguments[1].span(),
                "immediate non-async inline closure",
                self.type_name(body_expression.type_id()),
                span,
            ));
            return None;
        };
        let optional_pointer = self.ffi_builtin_type("Ffi.OptionalReadOnlyPointer", vec![byte])?;
        let valid_shape = !body.is_async()
            && matches!(body.parameters(), [pointer, length]
                if pointer.type_id() == optional_pointer && length.type_id() == size)
            && body.results().len() == 1;
        if !valid_shape || !scoped_borrow_body_is_valid(&body, self.signatures) {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                arguments[1].span(),
                "non-escaping scoped FFI byte borrow body",
                "incompatible closure",
                span,
            ));
            return None;
        }
        let region = pop_foundation::BorrowRegionId::from_raw(self.next_borrow_region);
        self.next_borrow_region = self.next_borrow_region.saturating_add(1);
        Some(TypedExpression {
            type_id: body.results()[0],
            kind: TypedExpressionKind::FfiBytesWithPin {
                bytes: Box::new(bytes),
                body,
                body_type,
                region,
            },
            span,
        })
    }

    fn ffi_builtin_type(&mut self, name: &str, arguments: Vec<TypeId>) -> Option<TypeId> {
        let definition = self.resolver.schema().type_by_source_name(name)?.id();
        self.resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition,
                arguments,
            })
            .ok()
    }

    fn ffi_buffer_payload(&self, type_id: TypeId) -> Option<TypeId> {
        match self.resolver.arena().get(type_id) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if *definition == crate::FFI_BUFFER_TYPE_ID && arguments.len() == 1 => {
                Some(arguments[0])
            }
            _ => None,
        }
    }

    pub(crate) fn check_ffi_pointer_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        explicit_element: Option<(TypeId, Option<pop_foundation::SymbolId>)>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let [_, owner, operation] = path else {
            return None;
        };
        let expected_arity = usize::from(operation != "none");
        if arguments.len() != expected_arity {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                path.join("."),
                expected_arity,
                arguments.len(),
            ));
            return None;
        }
        if operation == "none" {
            let (element, record) = explicit_element?;
            let layout_record = match self.ffi_buffer_element(element, record)? {
                FfiBufferElementKind::Scalar => None,
                FfiBufferElementKind::LayoutRecord(record) => Some(record),
            };
            let read_only = owner == "OptionalReadOnlyPointer";
            let result = self.ffi_builtin_type(
                if read_only {
                    "Ffi.OptionalReadOnlyPointer"
                } else {
                    "Ffi.OptionalPointer"
                },
                vec![element],
            )?;
            return Some(TypedExpression {
                kind: TypedExpressionKind::FfiPointerNone {
                    element,
                    layout_record,
                    read_only,
                },
                type_id: result,
                span,
            });
        }

        let pointer = self.check_expression(&arguments[0])?;
        let (expected, expected_name, result_name, kind) =
            match (owner.as_str(), operation.as_str()) {
                ("OptionalPointer", "fromPointer") => (
                    crate::FFI_POINTER_TYPE_ID,
                    "Ffi.Pointer",
                    "Ffi.OptionalPointer",
                    FfiPointerOperationKind::ToOptional,
                ),
                ("OptionalReadOnlyPointer", "fromPointer") => (
                    crate::FFI_READ_ONLY_POINTER_TYPE_ID,
                    "Ffi.ReadOnlyPointer",
                    "Ffi.OptionalReadOnlyPointer",
                    FfiPointerOperationKind::ToOptional,
                ),
                ("Pointer", "readOnly") => (
                    crate::FFI_POINTER_TYPE_ID,
                    "Ffi.Pointer",
                    "Ffi.ReadOnlyPointer",
                    FfiPointerOperationKind::ReadOnly,
                ),
                ("OptionalPointer", "isPresent") => (
                    crate::FFI_OPTIONAL_POINTER_TYPE_ID,
                    "Ffi.OptionalPointer",
                    "Boolean",
                    FfiPointerOperationKind::IsPresent,
                ),
                ("OptionalPointer", "require") => (
                    crate::FFI_OPTIONAL_POINTER_TYPE_ID,
                    "Ffi.OptionalPointer",
                    "Ffi.Pointer",
                    FfiPointerOperationKind::Require,
                ),
                ("OptionalReadOnlyPointer", "isPresent") => (
                    crate::FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID,
                    "Ffi.OptionalReadOnlyPointer",
                    "Boolean",
                    FfiPointerOperationKind::IsPresent,
                ),
                ("OptionalReadOnlyPointer", "require") => (
                    crate::FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID,
                    "Ffi.OptionalReadOnlyPointer",
                    "Ffi.ReadOnlyPointer",
                    FfiPointerOperationKind::Require,
                ),
                _ => return None,
            };
        let Some(element) = self.ffi_exact_builtin_payload(pointer.type_id(), expected) else {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                pointer.span(),
                format!("{expected_name}<T>"),
                self.type_name(pointer.type_id()),
                span,
            ));
            return None;
        };
        let (type_id, result_definition) = if matches!(kind, FfiPointerOperationKind::Require) {
            let success = self.ffi_builtin_type(result_name, vec![element])?;
            let error = self.ffi_builtin_type("Ffi.NullPointerError", Vec::new())?;
            (
                self.resolver.result_type(success, error)?,
                Some(self.resolver.result_definition()?),
            )
        } else if result_name == "Boolean" {
            (self.resolver.arena().source_type("Boolean")?, None)
        } else {
            (self.ffi_builtin_type(result_name, vec![element])?, None)
        };
        let pointer = Box::new(pointer);
        let kind = match kind {
            FfiPointerOperationKind::ToOptional => {
                TypedExpressionKind::FfiPointerToOptional { pointer }
            }
            FfiPointerOperationKind::ReadOnly => {
                TypedExpressionKind::FfiPointerReadOnly { pointer }
            }
            FfiPointerOperationKind::IsPresent => {
                TypedExpressionKind::FfiPointerIsPresent { pointer }
            }
            FfiPointerOperationKind::Require => TypedExpressionKind::FfiPointerRequire {
                pointer,
                result: result_definition.expect("require has one exact Result definition"),
                success: ResultCaseId::from_raw(0),
                failure: ResultCaseId::from_raw(1),
            },
        };
        Some(TypedExpression {
            kind,
            type_id,
            span,
        })
    }

    fn ffi_exact_builtin_payload(
        &self,
        type_id: TypeId,
        expected: BuiltinTypeId,
    ) -> Option<TypeId> {
        match self.resolver.arena().get(type_id) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if *definition == expected && arguments.len() == 1 => Some(arguments[0]),
            _ => None,
        }
    }

    pub(crate) fn check_ffi_unsafe_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        explicit_element: Option<(TypeId, Option<pop_foundation::SymbolId>)>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let [_, _, operation] = path else {
            return None;
        };
        let expected_arity = match operation.as_str() {
            "load" | "address" | "pointerFromAddress" => 1,
            "store" | "advance" | "advanceReadOnly" => 2,
            "copy" => 3,
            _ => return None,
        };
        if arguments.len() != expected_arity {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                path.join("."),
                expected_arity,
                arguments.len(),
            ));
            return None;
        }
        let size = self.ffi_builtin_type("Ffi.C.Size", Vec::new())?;
        let pointer_difference = self.ffi_builtin_type("Ffi.C.PointerDifference", Vec::new())?;
        if operation == "pointerFromAddress" {
            let (element, record) = explicit_element?;
            let Some(layout) = self.ffi_element_layout(element, record) else {
                self.diagnostics.push(type_diagnostics::type_mismatch(
                    span,
                    "FFI ABI storage type",
                    self.type_name(element),
                    span,
                ));
                return None;
            };
            let layout_record = layout.layout_record();
            let address = self.check_ffi_exact_expression(&arguments[0], size, span)?;
            let type_id = self.ffi_builtin_type("Ffi.OptionalPointer", vec![element])?;
            return Some(TypedExpression {
                kind: TypedExpressionKind::FfiUnsafePointerFromAddress {
                    address: Box::new(address),
                    element,
                    layout_record,
                },
                type_id,
                span,
            });
        }

        let first = self.check_expression(&arguments[0])?;
        let expected_pointer = match operation.as_str() {
            "store" | "advance" => crate::FFI_POINTER_TYPE_ID,
            "load" | "advanceReadOnly" | "address" | "copy" => crate::FFI_READ_ONLY_POINTER_TYPE_ID,
            _ => return None,
        };
        let Some(element) = self.ffi_exact_builtin_payload(first.type_id(), expected_pointer)
        else {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                first.span(),
                if expected_pointer == crate::FFI_POINTER_TYPE_ID {
                    "Ffi.Pointer<T>"
                } else {
                    "Ffi.ReadOnlyPointer<T>"
                },
                self.type_name(first.type_id()),
                span,
            ));
            return None;
        };
        let Some(layout) = self.ffi_element_layout(element, None) else {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                first.span(),
                "FFI ABI storage type",
                self.type_name(element),
                span,
            ));
            return None;
        };
        let layout_record = layout.layout_record();
        let kind = match operation.as_str() {
            "load" => TypedExpressionKind::FfiUnsafeLoad {
                pointer: Box::new(first),
                element,
                layout_record,
            },
            "store" => TypedExpressionKind::FfiUnsafeStore {
                pointer: Box::new(first),
                value: Box::new(self.check_ffi_exact_expression(&arguments[1], element, span)?),
                element,
                layout_record,
            },
            "advance" | "advanceReadOnly" => TypedExpressionKind::FfiUnsafeAdvance {
                pointer: Box::new(first),
                elements: Box::new(self.check_ffi_exact_expression(
                    &arguments[1],
                    pointer_difference,
                    span,
                )?),
                element,
                layout_record,
                read_only: operation == "advanceReadOnly",
            },
            "copy" => {
                let destination = self.check_expression(&arguments[1])?;
                if self.ffi_exact_builtin_payload(destination.type_id(), crate::FFI_POINTER_TYPE_ID)
                    != Some(element)
                {
                    self.diagnostics.push(type_diagnostics::type_mismatch(
                        destination.span(),
                        format!("Ffi.Pointer<{}>", self.type_name(element)),
                        self.type_name(destination.type_id()),
                        span,
                    ));
                    return None;
                }
                TypedExpressionKind::FfiUnsafeCopy {
                    source: Box::new(first),
                    destination: Box::new(destination),
                    count: Box::new(self.check_ffi_exact_expression(&arguments[2], size, span)?),
                    element,
                    layout_record,
                }
            }
            "address" => TypedExpressionKind::FfiUnsafeAddress {
                pointer: Box::new(first),
                element,
                layout_record,
            },
            _ => return None,
        };
        let type_id = match operation.as_str() {
            "load" => element,
            "store" | "copy" => self.resolver.arena().source_type("nil")?,
            "advance" => self.ffi_builtin_type("Ffi.Pointer", vec![element])?,
            "advanceReadOnly" => self.ffi_builtin_type("Ffi.ReadOnlyPointer", vec![element])?,
            "address" => size,
            _ => return None,
        };
        Some(TypedExpression {
            kind,
            type_id,
            span,
        })
    }

    fn ffi_element_layout(
        &self,
        element: TypeId,
        record: Option<pop_foundation::SymbolId>,
    ) -> Option<FfiBufferElementKind> {
        let record = record.or_else(|| {
            self.resolver
                .record_definition_for_type(element)
                .map(|definition| definition.symbol())
        });
        self.ffi_buffer_element(element, record)
    }

    fn check_ffi_exact_expression(
        &mut self,
        expression: &ExpressionSyntax,
        expected: TypeId,
        call_span: SourceSpan,
    ) -> Option<TypedExpression> {
        let typed = self.check_expression(expression)?;
        if typed.type_id() == expected {
            return Some(typed);
        }
        self.diagnostics.push(type_diagnostics::type_mismatch(
            typed.span(),
            self.type_name(expected),
            self.type_name(typed.type_id()),
            call_span,
        ));
        None
    }

    fn ffi_buffer_element(
        &self,
        type_id: TypeId,
        record: Option<pop_foundation::SymbolId>,
    ) -> Option<FfiBufferElementKind> {
        match self.resolver.arena().get(type_id) {
            Some(SemanticType::Primitive(
                PrimitiveType::Integer(_) | PrimitiveType::Float32 | PrimitiveType::Float64,
            )) => Some(FfiBufferElementKind::Scalar),
            Some(SemanticType::Builtin { definition, .. }) => {
                (crate::is_ffi_integer_abi_builtin_type(*definition)
                    || crate::is_ffi_pointer_type_constructor(*definition)
                    || crate::is_ffi_function_type_constructor(*definition)
                    || *definition == crate::FFI_HANDLE_TYPE_ID)
                    .then_some(FfiBufferElementKind::Scalar)
            }
            Some(SemanticType::Record(_)) => record
                .and_then(|symbol| self.resolver.record_definition(symbol))
                .filter(|definition| {
                    definition.type_id() == type_id
                        && definition.has_ffi_c_layout()
                        && self.resolver.ffi_c_layout_is_valid(definition.symbol())
                })
                .map(|definition| FfiBufferElementKind::LayoutRecord(definition.symbol())),
            _ => None,
        }
    }

    fn check_exact_source_overload(
        &mut self,
        name: &str,
        symbols: &[pop_foundation::SymbolId],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        let candidates = symbols
            .iter()
            .filter_map(|symbol| {
                self.signatures
                    .get(symbol)
                    .cloned()
                    .map(|signature| (*symbol, signature))
            })
            .collect::<Vec<_>>();
        if candidates.len() != symbols.len()
            || candidates
                .iter()
                .any(|(_, signature)| !signature.type_parameters().is_empty())
        {
            return None;
        }
        let mut typed_arguments = Vec::with_capacity(arguments.len());
        for argument in arguments {
            typed_arguments.push(self.check_expression(argument)?);
        }
        let argument_types = typed_arguments
            .iter()
            .map(TypedExpression::type_id)
            .collect::<Vec<_>>();
        let matching = candidates
            .iter()
            .filter(|(_, signature)| {
                signature.parameters().len() == argument_types.len()
                    && signature
                        .parameters()
                        .iter()
                        .filter_map(|parameter| parameter.parameter_type().type_id())
                        .eq(argument_types.iter().copied())
            })
            .collect::<Vec<_>>();
        let [(symbol, signature)] = matching.as_slice() else {
            self.diagnostics
                .push(type_diagnostics::no_matching_overload(
                    span,
                    name,
                    symbols.iter().filter_map(|symbol| {
                        self.resolver
                            .database()
                            .index()
                            .declaration(*symbol)
                            .map(pop_resolve::Declaration::span)
                    }),
                ));
            return None;
        };
        let results = signature
            .results()
            .iter()
            .map(crate::ResolvedType::type_id)
            .collect::<Option<Vec<_>>>()?;
        let results = self.call_result_types(signature.is_async(), results)?;
        let dispatch = self
            .resolver
            .database()
            .index()
            .declaration(*symbol)
            .and_then(pop_resolve::Declaration::reference_identity)
            .map_or(
                TypedCallDispatch::Direct { function: *symbol },
                |function| TypedCallDispatch::Referenced { function },
            );
        Some(CheckedCall {
            call: TypedCall {
                dispatch,
                is_async: signature.is_async(),
                type_arguments: Vec::new(),
                arguments: typed_arguments,
                span,
            },
            results,
        })
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

        let mut inferred_type_arguments = Vec::with_capacity(signature.type_parameters().len());
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
            inferred_type_arguments.push(argument);
        }
        let substitution_map: BTreeMap<_, _> = signature
            .type_parameters()
            .iter()
            .zip(&inferred_type_arguments)
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
                type_arguments: inferred_type_arguments,
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

    fn task_builtin_type(&mut self, name: &str, arguments: Vec<TypeId>) -> Option<TypeId> {
        let definition = self.resolver.schema().type_by_source_name(name)?.id();
        self.resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition,
                arguments,
            })
            .ok()
    }

    fn task_argument(
        &mut self,
        argument: &ExpressionSyntax,
        expected: TypeId,
        operation: &str,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let typed = self
            .check_expression_expected(argument, Some(ExpectedExpressionType::plain(expected)))?;
        if typed.type_id() != expected {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                argument.span(),
                self.type_name(expected),
                self.type_name(typed.type_id()),
                span,
            ));
            let _ = operation;
            return None;
        }
        Some(typed)
    }

    fn check_task_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let operation = path.get(1)?.as_str();
        let expected_arity = match operation {
            "cancellationSource" => 0,
            "cancelToken" | "cancel" => 1,
            "group" | "start" => 2,
            _ => return None,
        };
        if arguments.len() != expected_arity {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                format!("Task.{operation}"),
                expected_arity,
                arguments.len(),
            ));
            return None;
        }
        let source_type = self.task_builtin_type("Task.CancelSource", Vec::new())?;
        let group_type = self.task_builtin_type("Task.Group", Vec::new())?;
        let cancel_type = self.task_builtin_type("CancelToken", Vec::new())?;
        match operation {
            "cancellationSource" => Some(TypedExpression {
                kind: TypedExpressionKind::TaskCancellationSource,
                type_id: source_type,
                span,
            }),
            "cancelToken" | "cancel" => {
                let source = self.task_argument(&arguments[0], source_type, operation, span)?;
                Some(TypedExpression {
                    kind: if operation == "cancelToken" {
                        TypedExpressionKind::TaskCancelToken {
                            source: Box::new(source),
                        }
                    } else {
                        TypedExpressionKind::TaskCancel {
                            source: Box::new(source),
                        }
                    },
                    type_id: if operation == "cancelToken" {
                        cancel_type
                    } else {
                        self.resolver.arena().source_type("nil")?
                    },
                    span,
                })
            }
            "start" => {
                let group = self.task_argument(&arguments[0], group_type, operation, span)?;
                let task = self.check_expression(&arguments[1])?;
                let task_definition = self.resolver.schema().type_by_source_name("Task")?.id();
                if !matches!(
                    self.resolver.arena().get(task.type_id()),
                    Some(SemanticType::Builtin { definition, arguments })
                        if *definition == task_definition && arguments.len() == 1
                ) {
                    self.diagnostics.push(type_diagnostics::type_mismatch(
                        arguments[1].span(),
                        "Task<T>",
                        self.type_name(task.type_id()),
                        span,
                    ));
                    return None;
                }
                let task_type = task.type_id();
                Some(TypedExpression {
                    kind: TypedExpressionKind::TaskStart {
                        group: Box::new(group),
                        task: Box::new(task),
                    },
                    type_id: task_type,
                    span,
                })
            }
            "group" => {
                let cancel = self.task_argument(&arguments[0], cancel_type, operation, span)?;
                let body = self.check_expression(&arguments[1])?;
                let Some(SemanticType::Function {
                    is_async: true,
                    parameters,
                    results,
                    ..
                }) = self.resolver.arena().get(body.type_id()).cloned()
                else {
                    self.diagnostics.push(type_diagnostics::type_mismatch(
                        arguments[1].span(),
                        "async function(Task.Group): T",
                        self.type_name(body.type_id()),
                        span,
                    ));
                    return None;
                };
                if parameters.as_slice() != [group_type] {
                    self.diagnostics.push(type_diagnostics::type_mismatch(
                        arguments[1].span(),
                        "async function(Task.Group): T",
                        self.type_name(body.type_id()),
                        span,
                    ));
                    return None;
                }
                let completion = match results.as_slice() {
                    [result] => *result,
                    _ => self
                        .resolver
                        .arena_mut()
                        .intern(SemanticType::Tuple(results))
                        .ok()?,
                };
                let task_type = self.task_builtin_type("Task", vec![completion])?;
                Some(TypedExpression {
                    kind: TypedExpressionKind::TaskGroup {
                        cancel: Box::new(cancel),
                        body: Box::new(body),
                    },
                    type_id: task_type,
                    span,
                })
            }
            _ => None,
        }
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
        if !self
            .resolver
            .database()
            .resolve(self.module, name, SymbolSpace::Value, span)
            .symbols()
            .is_empty()
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

fn scoped_borrow_body_is_valid(
    body: &TypedClosure,
    signatures: &BTreeMap<pop_foundation::SymbolId, crate::ResolvedFunctionSignature>,
) -> bool {
    let Some(pointer) = body
        .parameters()
        .first()
        .map(TypedClosureParameter::parameter)
    else {
        return false;
    };
    body.body()
        .statements()
        .iter()
        .all(|statement| match statement.kind() {
            TypedStatementKind::Return { values } => values
                .iter()
                .all(|value| scoped_borrow_expression_is_valid(value, pointer, false, signatures)),
            TypedStatementKind::Expression(expression) => {
                scoped_borrow_expression_is_valid(expression, pointer, false, signatures)
            }
            _ => false,
        })
}

fn scoped_borrow_expression_is_valid(
    expression: &TypedExpression,
    pointer: pop_foundation::ValueParameterId,
    pointer_allowed: bool,
    signatures: &BTreeMap<pop_foundation::SymbolId, crate::ResolvedFunctionSignature>,
) -> bool {
    match expression.kind() {
        TypedExpressionKind::Parameter(parameter) if *parameter == pointer => pointer_allowed,
        TypedExpressionKind::FfiPointerIsPresent { pointer: operand } => {
            scoped_borrow_expression_is_valid(operand, pointer, true, signatures)
        }
        TypedExpressionKind::FfiPointerRequire { pointer: operand, .. } => {
            pointer_allowed
                && scoped_borrow_expression_is_valid(operand, pointer, true, signatures)
        }
        TypedExpressionKind::ResultPropagate { result, .. } => {
            scoped_borrow_expression_is_valid(
                result,
                pointer,
                pointer_allowed,
                signatures,
            )
        }
        TypedExpressionKind::FfiPointerToOptional { pointer: operand }
        | TypedExpressionKind::FfiPointerReadOnly { pointer: operand } => {
            scoped_borrow_expression_is_valid(operand, pointer, false, signatures)
        }
        TypedExpressionKind::DirectCall {
            function,
            is_async,
            arguments,
            ..
        } => {
            if *is_async
                || signatures
                    .get(function)
                    .is_some_and(|signature| signature.effects().contains(crate::Effect::Suspends))
            {
                return false;
            }
            let foreign = signatures.get(function).is_some_and(|signature| {
                signature.effects().contains(crate::Effect::ForeignFunction)
            });
            arguments.iter().all(|argument| {
                scoped_borrow_expression_is_valid(argument, pointer, foreign, signatures)
            })
        }
        TypedExpressionKind::ReferencedCall {
            function,
            is_async,
            arguments,
            ..
        } => {
            if *is_async
                || signatures.get(&function.symbol()).is_some_and(|signature| {
                    signature.effects().contains(crate::Effect::Suspends)
                })
            {
                return false;
            }
            let foreign = signatures.get(&function.symbol()).is_some_and(|signature| {
                signature.effects().contains(crate::Effect::ForeignFunction)
            });
            arguments.iter().all(|argument| {
                scoped_borrow_expression_is_valid(argument, pointer, foreign, signatures)
            })
        }
        TypedExpressionKind::StandardCall { arguments, .. } => arguments
            .iter()
            .all(|argument| scoped_borrow_expression_is_valid(argument, pointer, false, signatures)),
        TypedExpressionKind::IndirectCall {
            callee,
            is_async,
            arguments,
        } => {
            !is_async
                && scoped_borrow_expression_is_valid(callee, pointer, false, signatures)
                && arguments
                    .iter()
                    .all(|argument| scoped_borrow_expression_is_valid(argument, pointer, false, signatures))
        }
        TypedExpressionKind::Closure(closure) => !closure.captures().iter().any(|capture| {
            matches!(capture.source(), CaptureSource::Parameter(parameter) if parameter == pointer)
        }),
        TypedExpressionKind::Array(elements) | TypedExpressionKind::Tuple(elements) => elements
            .iter()
            .all(|element| scoped_borrow_expression_is_valid(element, pointer, false, signatures)),
        TypedExpressionKind::Table(entries) => entries.iter().all(|entry| {
            scoped_borrow_expression_is_valid(entry.key(), pointer, false, signatures)
                && scoped_borrow_expression_is_valid(entry.value(), pointer, false, signatures)
        }),
        TypedExpressionKind::Record { fields, .. }
        | TypedExpressionKind::ClassConstruct { fields, .. } => fields.iter().all(|field| {
            scoped_borrow_expression_is_valid(field.value(), pointer, false, signatures)
        }),
        TypedExpressionKind::RecordUpdate { base, fields, .. } => {
            scoped_borrow_expression_is_valid(base, pointer, false, signatures)
                && fields.iter().all(|field| {
                    scoped_borrow_expression_is_valid(field.value(), pointer, false, signatures)
                })
        }
        TypedExpressionKind::UnionCase { arguments, .. }
        | TypedExpressionKind::ResultCase { arguments, .. }
        | TypedExpressionKind::IterationCase { arguments, .. }
        | TypedExpressionKind::ErrorCase { arguments, .. } => arguments.iter().all(|argument| {
            scoped_borrow_expression_is_valid(argument, pointer, false, signatures)
        }),
        TypedExpressionKind::FfiBufferWithPointer { .. }
        | TypedExpressionKind::FfiBytesWithPin { .. }
        | TypedExpressionKind::FfiUnsafeLoad { .. }
        | TypedExpressionKind::FfiUnsafeStore { .. }
        | TypedExpressionKind::FfiUnsafeAdvance { .. }
        | TypedExpressionKind::FfiUnsafeCopy { .. }
        | TypedExpressionKind::FfiUnsafeAddress { .. }
        | TypedExpressionKind::FfiUnsafePointerFromAddress { .. }
        | TypedExpressionKind::TaskGroup { .. }
        | TypedExpressionKind::TaskStart { .. }
        | TypedExpressionKind::TaskCancellationSource
        | TypedExpressionKind::TaskCancelToken { .. }
        | TypedExpressionKind::TaskCancel { .. }
        | TypedExpressionKind::Await { .. } => false,
        _ => true,
    }
}

fn is_ffi_pointer_operation(path: &[String]) -> bool {
    matches!(path,
    [ffi, owner, operation]
        if ffi == "Ffi"
            && matches!(
                (owner.as_str(), operation.as_str()),
                (
                    "OptionalPointer" | "OptionalReadOnlyPointer",
                    "fromPointer" | "isPresent" | "require"
                ) | ("Pointer", "readOnly")
            ))
}

fn is_ffi_unsafe_operation(path: &[String]) -> bool {
    matches!(path,
        [ffi, unsafe_namespace, operation]
            if ffi == "Ffi"
                && unsafe_namespace == "Unsafe"
                && matches!(operation.as_str(),
                    "load" | "store" | "advance" | "advanceReadOnly" | "copy" | "address"))
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
