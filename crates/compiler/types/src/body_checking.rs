use std::collections::{BTreeMap, BTreeSet};

use pop_diagnostics::{resolution as resolution_diagnostics, types as type_diagnostics};
use pop_foundation::{
    BindingId, CaptureId, Diagnostic, FieldId, LocalId, ModuleId, NestedFunctionId,
    NominalInterfaceId, ResultCaseId, SourceSpan, SymbolId, TypeId, ValueParameterId,
};
use pop_resolve::SymbolSpace;
use pop_syntax::{
    BinaryOperator as SyntaxBinaryOperator, ExpressionSyntax, ExpressionSyntaxKind,
    FunctionBodySyntax, StringSegmentSyntaxKind, UnaryOperator as SyntaxUnaryOperator,
};

use crate::capture_analysis::finalize_capture_modes;
use crate::typed_body::*;
use crate::{
    AttributeConstant, AttributeQuerySubject, FloatKind, FloatValue, IntegerKind, IntegerValue,
    PrimitiveType, ResolvedFunctionSignature, SemanticType, SignatureResolver,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeConstant {
    type_id: TypeId,
    value: AttributeConstant,
}

impl RuntimeConstant {
    #[must_use]
    pub const fn new(type_id: TypeId, value: AttributeConstant) -> Self {
        Self { type_id, value }
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub const fn value(&self) -> &AttributeConstant {
        &self.value
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Binding {
    pub(crate) id: BindingId,
    pub(crate) kind: BindingKind,
    pub(crate) type_id: TypeId,
    pub(crate) function_depth: u32,
}

#[derive(Clone, Copy)]
pub(crate) enum BindingKind {
    Local(LocalId),
    LoopLocal(LocalId),
    ImmutableLocal(LocalId),
    Parameter(ValueParameterId),
}

#[derive(Clone, Copy, Eq, PartialEq)]
pub(crate) enum ActiveCollectionIteration {
    Local(LocalId),
    Parameter(ValueParameterId),
    Capture(CaptureId),
}

impl BindingKind {
    pub(crate) const fn capture_source(self) -> CaptureSource {
        match self {
            Self::Local(local) | Self::LoopLocal(local) | Self::ImmutableLocal(local) => {
                CaptureSource::Local(local)
            }
            Self::Parameter(parameter) => CaptureSource::Parameter(parameter),
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct PendingCapture {
    pub(crate) capture: CaptureId,
    pub(crate) binding: BindingId,
    pub(crate) source: CaptureSource,
    pub(crate) type_id: TypeId,
}

pub(crate) struct ActiveFunction {
    pub(crate) function: NestedFunctionId,
    pub(crate) depth: u32,
    pub(crate) next_capture: u32,
    pub(crate) captures: BTreeMap<BindingId, PendingCapture>,
}

pub(crate) enum UnionCaseLookup {
    NotUnion,
    Missing,
    Found(crate::UnionDefinition, crate::UnionCaseDefinition),
}

pub(crate) enum ErrorCaseLookup {
    NotError,
    Missing,
    Found(crate::ErrorDefinition, crate::ErrorCaseDefinition),
}

pub(crate) enum BoundPathLookup {
    NotBound,
    Error,
    Found(TypedExpression),
}

pub(crate) struct CheckedCall {
    pub(crate) call: TypedCall,
    pub(crate) results: Vec<TypeId>,
}

pub(crate) struct ResolvedClosureShape {
    pub(crate) parameters: Vec<(String, TypeId, SourceSpan)>,
    pub(crate) results: Vec<(TypeId, SourceSpan)>,
    pub(crate) function_type: TypeId,
}

pub(crate) enum CheckedInvocation {
    Call(CheckedCall),
    Value(TypedExpression),
}

#[derive(Clone, Copy)]
pub(crate) enum NumericTarget {
    Integer(IntegerKind),
    Float(FloatKind),
}

#[derive(Clone, Copy)]
pub(crate) struct ExpectedExpressionType {
    pub(crate) type_id: TypeId,
    pub(crate) declaration: Option<SymbolId>,
}

impl ExpectedExpressionType {
    pub(crate) const fn plain(type_id: TypeId) -> Self {
        Self {
            type_id,
            declaration: None,
        }
    }

    pub(crate) fn resolved(resolved: &crate::ResolvedType) -> Option<Self> {
        let declaration = match resolved.kind() {
            crate::ResolvedTypeKind::Declaration { symbol, .. } => Some(*symbol),
            _ => None,
        };
        Some(Self {
            type_id: resolved.type_id()?,
            declaration,
        })
    }
}

pub struct BodyChecker<'resolver, 'index> {
    pub(crate) module: ModuleId,
    pub(crate) resolver: &'resolver mut SignatureResolver<'index>,
    pub(crate) signatures: &'resolver BTreeMap<SymbolId, ResolvedFunctionSignature>,
    pub(crate) constants: Option<&'resolver BTreeMap<SymbolId, RuntimeConstant>>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) scopes: Vec<BTreeMap<String, Binding>>,
    pub(crate) next_local: u32,
    pub(crate) next_binding: u32,
    pub(crate) next_nested_function: u32,
    pub(crate) function_depth: u32,
    pub(crate) active_functions: Vec<ActiveFunction>,
    pub(crate) written_bindings: BTreeSet<BindingId>,
    pub(crate) signature_stack: Vec<ResolvedFunctionSignature>,
    pub(crate) loop_depth: u32,
    pub(crate) flow_narrowings: Vec<BTreeMap<BindingId, TypeId>>,
    pub(crate) active_collection_iterations: Vec<ActiveCollectionIteration>,
}

impl<'resolver, 'index> BodyChecker<'resolver, 'index> {
    pub(crate) fn call_result_types(
        &mut self,
        is_async: bool,
        results: Vec<TypeId>,
    ) -> Option<Vec<TypeId>> {
        if !is_async {
            return Some(results);
        }
        let completion = match results.as_slice() {
            [completion] => *completion,
            _ => self
                .resolver
                .arena_mut()
                .intern(SemanticType::Tuple(results))
                .ok()?,
        };
        let task = self.resolver.schema().type_by_source_name("Task")?.id();
        let task_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: task,
                arguments: vec![completion],
            })
            .ok()?;
        Some(vec![task_type])
    }

    fn check_await(
        &mut self,
        operand: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if !self
            .signature_stack
            .last()
            .is_some_and(ResolvedFunctionSignature::is_async)
        {
            self.diagnostics
                .push(type_diagnostics::await_outside_async(span));
            return None;
        }
        let task = self.check_expression(operand)?;
        let task_definition = self.resolver.schema().type_by_source_name("Task")?.id();
        let completion = match self.resolver.arena().get(task.type_id()) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if *definition == task_definition && arguments.len() == 1 => arguments[0],
            _ => {
                self.diagnostics.push(type_diagnostics::await_non_task(
                    operand.span(),
                    self.type_name(task.type_id()),
                ));
                return None;
            }
        };
        self.invalidate_flow_narrowings();
        Some(TypedExpression {
            kind: TypedExpressionKind::Await {
                task: Box::new(task),
            },
            type_id: completion,
            span,
        })
    }

    #[must_use]
    pub fn new(
        module: ModuleId,
        resolver: &'resolver mut SignatureResolver<'index>,
        signatures: &'resolver BTreeMap<SymbolId, ResolvedFunctionSignature>,
    ) -> Self {
        Self {
            module,
            resolver,
            signatures,
            constants: None,
            diagnostics: Vec::new(),
            scopes: vec![BTreeMap::new()],
            next_local: 0,
            next_binding: 0,
            next_nested_function: 0,
            function_depth: 0,
            active_functions: Vec::new(),
            written_bindings: BTreeSet::new(),
            signature_stack: Vec::new(),
            loop_depth: 0,
            flow_narrowings: Vec::new(),
            active_collection_iterations: Vec::new(),
        }
    }

    #[must_use]
    pub const fn with_runtime_constants(
        mut self,
        constants: &'resolver BTreeMap<SymbolId, RuntimeConstant>,
    ) -> Self {
        self.constants = Some(constants);
        self
    }

    #[must_use]
    pub fn check(
        mut self,
        signature: &ResolvedFunctionSignature,
        body: &FunctionBodySyntax,
    ) -> TypedBodyResult {
        self.signature_stack.push(signature.clone());
        for (index, parameter) in signature.parameters().iter().enumerate() {
            if let Some(type_id) = parameter.parameter_type().type_id() {
                let raw = u32::try_from(index).unwrap_or(u32::MAX);
                let binding = BindingId::from_raw(self.next_binding);
                self.next_binding = self.next_binding.saturating_add(1);
                self.scopes[0].insert(
                    parameter.name().to_owned(),
                    Binding {
                        id: binding,
                        kind: BindingKind::Parameter(ValueParameterId::from_raw(raw)),
                        type_id,
                        function_depth: 0,
                    },
                );
            }
        }
        let mut statements = Vec::new();
        for statement in body.statements() {
            if let Some(typed) = self.check_statement(signature, statement) {
                statements.push(typed);
            }
        }
        if self.diagnostics.is_empty()
            && !signature.results().is_empty()
            && !statements_definitely_return(&statements)
        {
            let file = signature.results()[0].span().file();
            self.diagnostics
                .push(type_diagnostics::not_all_paths_return(SourceSpan::new(
                    file,
                    body.range(),
                )));
        }
        self.diagnostics.sort_by_key(|diagnostic| {
            let span = diagnostic.primary_span();
            (
                span.file(),
                span.range().start(),
                diagnostic.code().as_str(),
            )
        });
        let mut typed = self
            .diagnostics
            .is_empty()
            .then_some(TypedBody { statements });
        if let Some(body) = &mut typed {
            finalize_capture_modes(body, &self.written_bindings);
        }
        TypedBodyResult {
            body: typed,
            diagnostics: self.diagnostics,
        }
    }

    /// Type-checks one expression required to produce an exact compile-time
    /// value type. This uses the ordinary source checker and resolved callable
    /// signatures; it does not grant compile-time eligibility by itself.
    #[must_use]
    pub fn check_required_expression(
        mut self,
        expression: &ExpressionSyntax,
        expected: TypeId,
    ) -> TypedExpressionResult {
        let typed = self
            .check_expression_expected(expression, Some(ExpectedExpressionType::plain(expected)));
        if let Some(typed) = &typed {
            self.require_same_type(expected, typed.type_id(), typed.span(), expression.span());
        }
        TypedExpressionResult {
            expression: self.diagnostics.is_empty().then_some(typed).flatten(),
            diagnostics: self.diagnostics,
        }
    }

    /// Type-checks a namespace constant initializer, inferring its type when
    /// no explicit annotation was supplied.
    #[must_use]
    pub fn check_constant_expression(
        mut self,
        expression: &ExpressionSyntax,
        expected: Option<TypeId>,
    ) -> TypedExpressionResult {
        let typed =
            self.check_expression_expected(expression, expected.map(ExpectedExpressionType::plain));
        if let (Some(expected), Some(typed)) = (expected, &typed) {
            self.require_same_type(expected, typed.type_id(), typed.span(), expression.span());
        }
        TypedExpressionResult {
            expression: self.diagnostics.is_empty().then_some(typed).flatten(),
            diagnostics: self.diagnostics,
        }
    }

    pub(crate) fn check_condition(
        &mut self,
        condition: &ExpressionSyntax,
    ) -> Option<TypedExpression> {
        let typed = self.check_expression(condition)?;
        let boolean = self.resolver.arena().source_type("Boolean")?;
        self.require_same_type(boolean, typed.type_id(), typed.span(), condition.span());
        Some(typed)
    }

    pub(crate) fn check_expression(
        &mut self,
        expression: &ExpressionSyntax,
    ) -> Option<TypedExpression> {
        self.check_expression_expected(expression, None)
    }

    pub(crate) fn check_expression_expected(
        &mut self,
        expression: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
    ) -> Option<TypedExpression> {
        let typed = self.check_expression_uncoerced(expression, expected)?;
        let Some(expected) = expected else {
            return Some(typed);
        };
        if typed.type_id() == expected.type_id {
            return Some(typed);
        }
        if self
            .resolver
            .is_class_to_interface_upcast(typed.type_id(), expected.type_id)
        {
            let SemanticType::Interface { interface, .. } =
                self.resolver.arena().get(expected.type_id)?
            else {
                return None;
            };
            return Some(TypedExpression {
                kind: TypedExpressionKind::InterfaceUpcast {
                    value: Box::new(typed),
                    interface: NominalInterfaceId::User(*interface),
                },
                type_id: expected.type_id,
                span: expression.span(),
            });
        }
        if self
            .resolver
            .is_class_to_builtin_interface_upcast(typed.type_id(), expected.type_id)
        {
            let SemanticType::Builtin { definition, .. } =
                self.resolver.arena().get(expected.type_id)?
            else {
                return None;
            };
            return Some(TypedExpression {
                kind: TypedExpressionKind::InterfaceUpcast {
                    value: Box::new(typed),
                    interface: NominalInterfaceId::Builtin(*definition),
                },
                type_id: expected.type_id,
                span: expression.span(),
            });
        }
        Some(typed)
    }

    pub(crate) fn check_expression_uncoerced(
        &mut self,
        expression: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
    ) -> Option<TypedExpression> {
        let span = expression.span();
        match expression.kind() {
            ExpressionSyntaxKind::Integer(value) => self.numeric_literal_expression(
                value,
                expected.map(|expected| expected.type_id),
                false,
                span,
            ),
            ExpressionSyntaxKind::Float(value) => self.float_literal_expression(
                value,
                expected.map(|expected| expected.type_id),
                span,
            ),
            ExpressionSyntaxKind::String(value) => self.primitive_expression(
                TypedExpressionKind::String(value.clone()),
                "String",
                span,
            ),
            ExpressionSyntaxKind::InterpolatedString(segments) => {
                self.check_interpolated_string(segments, span)
            }
            ExpressionSyntaxKind::Boolean(value) => {
                self.primitive_expression(TypedExpressionKind::Boolean(*value), "Boolean", span)
            }
            ExpressionSyntaxKind::Nil => {
                self.primitive_expression(TypedExpressionKind::Nil, "nil", span)
            }
            ExpressionSyntaxKind::Function(function) => {
                let signature = self.signature_stack.last()?.clone();
                self.check_closure_expression(&signature, function)
            }
            ExpressionSyntaxKind::Await { operand } => self.check_await(operand, span),
            ExpressionSyntaxKind::Name(path) => self.check_name(path, expected, span),
            ExpressionSyntaxKind::Index { base, index } => self.check_array_get(base, index, span),
            ExpressionSyntaxKind::Construct { type_name, fields } => self.check_class_construct(
                type_name,
                fields,
                expected.map(|expected| expected.type_id),
                span,
            ),
            ExpressionSyntaxKind::MethodCall {
                receiver,
                method,
                arguments,
            } => {
                let result = self.check_receiver_method_call(receiver, method, arguments, span);
                self.invalidate_flow_narrowings();
                result
            }
            ExpressionSyntaxKind::Array(elements) => {
                self.check_array_literal(elements, expected.map(|expected| expected.type_id), span)
            }
            ExpressionSyntaxKind::Aggregate { fields } => {
                self.check_aggregate_literal(fields, expected, span)
            }
            ExpressionSyntaxKind::With { base, fields } => {
                self.check_record_update(base, fields, span)
            }
            ExpressionSyntaxKind::Tuple(elements) => {
                let elements: Option<Vec<_>> = elements
                    .iter()
                    .map(|element| self.check_expression(element))
                    .collect();
                let elements = elements?;
                let type_id = self
                    .resolver
                    .arena_mut()
                    .intern(SemanticType::Tuple(
                        elements.iter().map(TypedExpression::type_id).collect(),
                    ))
                    .ok()?;
                Some(TypedExpression {
                    kind: TypedExpressionKind::Tuple(elements),
                    type_id,
                    span,
                })
            }
            ExpressionSyntaxKind::Unary { operator, operand } => {
                self.check_unary(*operator, operand, expected, span)
            }
            ExpressionSyntaxKind::OptionalPropagate { operand } => {
                self.check_optional_propagate(operand, span)
            }
            ExpressionSyntaxKind::ResultPropagate { .. } => {
                let ExpressionSyntaxKind::ResultPropagate { operand } = expression.kind() else {
                    unreachable!()
                };
                self.check_result_propagate(operand, span)
            }
            ExpressionSyntaxKind::Binary {
                operator,
                left,
                right,
            } => self.check_binary(*operator, left, right, expected, span),
            ExpressionSyntaxKind::Conditional {
                condition,
                when_true,
                when_false,
            } => {
                let condition = self.check_condition(condition)?;
                let when_true = self.check_expression_expected(when_true, expected)?;
                let branch_expected =
                    expected.unwrap_or_else(|| ExpectedExpressionType::plain(when_true.type_id()));
                let when_false =
                    self.check_expression_expected(when_false, Some(branch_expected))?;
                self.require_same_type(
                    when_true.type_id(),
                    when_false.type_id(),
                    when_false.span(),
                    span,
                );
                Some(TypedExpression {
                    type_id: when_true.type_id(),
                    kind: TypedExpressionKind::Conditional {
                        condition: Box::new(condition),
                        when_true: Box::new(when_true),
                        when_false: Box::new(when_false),
                    },
                    span,
                })
            }
            ExpressionSyntaxKind::Call { callee, arguments } => {
                let result = self.check_call(callee, arguments, expected, span);
                self.invalidate_flow_narrowings();
                result
            }
            ExpressionSyntaxKind::GenericCall {
                callee,
                type_arguments,
                arguments,
            } => {
                let result = self.check_generic_call(callee, type_arguments, arguments, span);
                self.invalidate_flow_narrowings();
                result
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    pub(crate) fn check_generic_call(
        &mut self,
        callee: &ExpressionSyntax,
        type_arguments: &[pop_syntax::TypeSyntax],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let ExpressionSyntaxKind::Name(path) = callee.kind() else {
            self.diagnostics.push(resolution_diagnostics::unknown_name(
                callee.span(),
                "generic call target",
            ));
            return None;
        };
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
            if type_arguments.len() != 1 {
                self.diagnostics.push(type_diagnostics::wrong_type_arity(
                    span,
                    "Ffi.Handle operation",
                    1,
                    type_arguments.len(),
                ));
                return None;
            }
            let enclosing = self.signature_stack.last().cloned()?;
            let (resolved, diagnostics) =
                self.resolver
                    .resolve_annotation(self.module, &type_arguments[0], &enclosing);
            self.diagnostics.extend(diagnostics);
            return self.check_ffi_handle_invocation(
                path,
                arguments,
                Some(resolved?.type_id()?),
                span,
            );
        }
        if matches!(path.as_slice(), [array, create] if array == "Array" && create == "create") {
            return self.check_array_create(type_arguments, arguments, span);
        }
        if matches!(path.as_slice(), [list, operation]
            if list == "List" && matches!(operation.as_str(), "create" | "withCapacity"))
        {
            return self.check_list_create(path, type_arguments, arguments, span);
        }
        if matches!(path.as_slice(), [result, case] if result == "Result" && matches!(case.as_str(), "Ok" | "Error"))
        {
            if type_arguments.len() != 2 {
                self.diagnostics.push(type_diagnostics::wrong_type_arity(
                    span,
                    "Result",
                    2,
                    type_arguments.len(),
                ));
                return None;
            }
            if arguments.len() != 1 {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    span,
                    "Result case construction",
                    1,
                    arguments.len(),
                ));
                return None;
            }
            let enclosing = self.signature_stack.last().cloned()?;
            let mut resolved = Vec::with_capacity(2);
            for argument in type_arguments {
                let (argument, diagnostics) =
                    self.resolver
                        .resolve_annotation(self.module, argument, &enclosing);
                self.diagnostics.extend(diagnostics);
                resolved.push(argument?.type_id()?);
            }
            let ok = path.last().is_some_and(|case| case == "Ok");
            let payload_type = if ok { resolved[0] } else { resolved[1] };
            let payload = self.check_expression_expected(
                &arguments[0],
                Some(ExpectedExpressionType::plain(payload_type)),
            )?;
            self.require_same_type(payload_type, payload.type_id(), payload.span(), span);
            let result_type = self.resolver.result_type(resolved[0], resolved[1])?;
            return Some(TypedExpression {
                kind: TypedExpressionKind::ResultCase {
                    result: self.resolver.result_definition()?,
                    case: ResultCaseId::from_raw(u32::from(!ok)),
                    arguments: vec![payload],
                },
                type_id: result_type,
                span,
            });
        }
        if path.len() >= 2 {
            let type_name = path[..path.len() - 1].join(".");
            let resolution = self.resolver.database().resolve(
                self.module,
                &type_name,
                SymbolSpace::Type,
                callee.span(),
            );
            if let Some(definition_symbol) = resolution.symbol()
                && let Some(expected_arity) =
                    self.resolver.union_type_parameter_count(definition_symbol)
            {
                if expected_arity != type_arguments.len() {
                    self.diagnostics.push(type_diagnostics::wrong_type_arity(
                        span,
                        &type_name,
                        u16::try_from(expected_arity).unwrap_or(u16::MAX),
                        type_arguments.len(),
                    ));
                    return None;
                }
                let enclosing = self.signature_stack.last().cloned()?;
                let mut resolved_arguments = Vec::with_capacity(type_arguments.len());
                for argument in type_arguments {
                    let (resolved, diagnostics) =
                        self.resolver
                            .resolve_annotation(self.module, argument, &enclosing);
                    self.diagnostics.extend(diagnostics);
                    resolved_arguments.push(resolved?.type_id()?);
                }
                let definition = self
                    .resolver
                    .instantiate_union(definition_symbol, &resolved_arguments)?;
                let case_name = path.last()?;
                let Some(case) = definition
                    .cases()
                    .iter()
                    .find(|case| case.name() == case_name)
                    .cloned()
                else {
                    self.diagnostics
                        .push(resolution_diagnostics::unknown_name(span, path.join(".")));
                    return None;
                };
                return self.check_union_case_call(&definition, &case, arguments, span);
            }
            if let Some(definition_symbol) = resolution.symbol()
                && let Some(expected_arity) =
                    self.resolver.error_type_parameter_count(definition_symbol)
            {
                if expected_arity != type_arguments.len() {
                    self.diagnostics.push(type_diagnostics::wrong_type_arity(
                        span,
                        &type_name,
                        u16::try_from(expected_arity).unwrap_or(u16::MAX),
                        type_arguments.len(),
                    ));
                    return None;
                }
                let enclosing = self.signature_stack.last().cloned()?;
                let mut resolved_arguments = Vec::with_capacity(type_arguments.len());
                for argument in type_arguments {
                    let (resolved, diagnostics) =
                        self.resolver
                            .resolve_annotation(self.module, argument, &enclosing);
                    self.diagnostics.extend(diagnostics);
                    resolved_arguments.push(resolved?.type_id()?);
                }
                let definition = self
                    .resolver
                    .instantiate_error(definition_symbol, &resolved_arguments)?;
                let case_name = path.last()?;
                let Some(case) = definition
                    .cases()
                    .iter()
                    .find(|case| case.name() == case_name)
                    .cloned()
                else {
                    self.diagnostics
                        .push(resolution_diagnostics::unknown_name(span, path.join(".")));
                    return None;
                };
                return self.check_error_case_call(&definition, &case, arguments, span);
            }
        }
        let name = path.join(".");
        let resolution =
            self.resolver
                .database()
                .resolve(self.module, &name, SymbolSpace::Value, callee.span());
        if let Some(symbol) = resolution.symbol()
            && let Some(signature) = self.signatures.get(&symbol).cloned()
        {
            if signature.type_parameters().len() != type_arguments.len() {
                self.diagnostics.push(type_diagnostics::wrong_type_arity(
                    span,
                    &name,
                    u16::try_from(signature.type_parameters().len()).unwrap_or(u16::MAX),
                    type_arguments.len(),
                ));
                return None;
            }
            if signature.parameters().len() != arguments.len() {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    span,
                    &name,
                    signature.parameters().len(),
                    arguments.len(),
                ));
                return None;
            }
            let enclosing = self.signature_stack.last().cloned()?;
            let mut resolved_arguments = Vec::with_capacity(type_arguments.len());
            for argument in type_arguments {
                let (resolved, diagnostics) =
                    self.resolver
                        .resolve_annotation(self.module, argument, &enclosing);
                self.diagnostics.extend(diagnostics);
                resolved_arguments.push(resolved?.type_id()?);
            }
            let substitutions: BTreeMap<_, _> = signature
                .type_parameters()
                .iter()
                .zip(&resolved_arguments)
                .map(|(parameter, argument)| (parameter.parameter(), *argument))
                .collect();
            let mut bound_substitutions = substitutions.clone();
            for (parameter, argument) in signature.type_parameters().iter().zip(&resolved_arguments)
            {
                if let Some(bound) = parameter.bound()
                    && !self.infer_type_pattern(bound, *argument, &mut bound_substitutions)
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
            for parameter in signature.type_parameters() {
                if let Some(bound) = parameter.bound() {
                    self.materialize_generic_bound_types(bound, &substitutions)?;
                }
            }
            let parameter_types = signature
                .parameters()
                .iter()
                .map(|parameter| {
                    self.resolver.substitute_type_parameters(
                        parameter.parameter_type().type_id()?,
                        &substitutions,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            let result_types = signature
                .results()
                .iter()
                .map(|result| {
                    self.resolver
                        .substitute_type_parameters(result.type_id()?, &substitutions)
                })
                .collect::<Option<Vec<_>>>()?;
            let result_types = self.call_result_types(signature.is_async(), result_types)?;
            let mut checked_arguments = Vec::with_capacity(arguments.len());
            for (argument, parameter_type) in arguments.iter().zip(parameter_types) {
                let typed = self.check_expression_expected(
                    argument,
                    Some(ExpectedExpressionType::plain(parameter_type)),
                )?;
                self.require_same_type(
                    parameter_type,
                    typed.type_id(),
                    typed.span(),
                    argument.span(),
                );
                checked_arguments.push(typed);
            }
            let dispatch = self
                .resolver
                .database()
                .index()
                .declaration(symbol)
                .and_then(pop_resolve::Declaration::reference_identity)
                .map_or(TypedCallDispatch::Direct { function: symbol }, |function| {
                    TypedCallDispatch::Referenced { function }
                });
            return self.checked_call_expression(CheckedCall {
                call: TypedCall {
                    dispatch,
                    is_async: signature.is_async(),
                    type_arguments: resolved_arguments,
                    arguments: checked_arguments,
                    span,
                },
                results: result_types,
            });
        }
        let [query] = path.as_slice() else {
            self.diagnostics.push(resolution_diagnostics::unknown_name(
                callee.span(),
                path.join("."),
            ));
            return None;
        };
        if !matches!(query.as_str(), "attribute" | "hasAttribute")
            || type_arguments.len() != 1
            || arguments.len() != 1
        {
            self.diagnostics
                .push(resolution_diagnostics::unknown_name(callee.span(), query));
            return None;
        }
        let pop_syntax::TypeSyntaxKind::Named {
            path: attribute_path,
            arguments: attribute_arguments,
        } = type_arguments[0].kind()
        else {
            self.diagnostics.push(resolution_diagnostics::unknown_name(
                type_arguments[0].span(),
                "attribute type",
            ));
            return None;
        };
        if !attribute_arguments.is_empty() {
            self.diagnostics.push(type_diagnostics::wrong_type_arity(
                type_arguments[0].span(),
                attribute_path.join("."),
                0,
                attribute_arguments.len(),
            ));
            return None;
        }
        let attribute_symbol = self.resolver.database().resolve(
            self.module,
            &attribute_path.join("."),
            SymbolSpace::Type,
            type_arguments[0].span(),
        );
        self.diagnostics
            .extend(attribute_symbol.diagnostics().iter().cloned());
        let definition = attribute_symbol
            .symbol()
            .and_then(|symbol| self.resolver.attribute_definition(symbol))?
            .clone();
        let ExpressionSyntaxKind::Name(subject_path) = arguments[0].kind() else {
            self.diagnostics.push(resolution_diagnostics::unknown_name(
                arguments[0].span(),
                "resolved attribute query subject",
            ));
            return None;
        };
        let subject_name = subject_path.join(".");
        let type_resolution = self.resolver.database().resolve(
            self.module,
            &subject_name,
            SymbolSpace::Type,
            arguments[0].span(),
        );
        let subject = if let Some(symbol) = type_resolution.symbol() {
            let type_id = self.resolver.declaration_type(symbol)?;
            AttributeQuerySubject::Type(type_id)
        } else {
            let value_resolution = self.resolver.database().resolve(
                self.module,
                &subject_name,
                SymbolSpace::Value,
                arguments[0].span(),
            );
            self.diagnostics
                .extend(value_resolution.diagnostics().iter().cloned());
            AttributeQuerySubject::Symbol(value_resolution.symbol()?)
        };
        let boolean = self.resolver.arena().source_type("Boolean")?;
        if query == "hasAttribute" {
            return Some(TypedExpression {
                kind: TypedExpressionKind::HasAttributeQuery {
                    module: self.module,
                    attribute: definition.attribute(),
                    subject,
                },
                type_id: boolean,
                span,
            });
        }
        let type_id = if definition.usage().is_repeatable() {
            self.resolver
                .arena_mut()
                .intern(SemanticType::Array(definition.type_id()))
                .ok()?
        } else {
            self.resolver
                .arena_mut()
                .optional(definition.type_id())
                .ok()?
        };
        Some(TypedExpression {
            kind: TypedExpressionKind::AttributeQuery {
                module: self.module,
                attribute: definition.attribute(),
                subject,
            },
            type_id,
            span,
        })
    }

    pub(crate) fn numeric_literal_expression(
        &mut self,
        value: &str,
        expected: Option<TypeId>,
        negative: bool,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let type_id = expected
            .filter(|type_id| self.is_numeric(*type_id))
            .or_else(|| self.resolver.arena().source_type("Int"))?;
        let spelling = if negative {
            format!("-{value}")
        } else {
            value.to_owned()
        };
        let kind = match self.numeric_target(type_id)? {
            NumericTarget::Integer(kind) => {
                IntegerValue::parse_decimal(&spelling, kind).map(TypedExpressionKind::Integer)
            }
            NumericTarget::Float(kind) => {
                FloatValue::parse_decimal(&spelling, kind).map(TypedExpressionKind::Float)
            }
        };
        if let Ok(kind) = kind {
            Some(TypedExpression {
                kind,
                type_id,
                span,
            })
        } else {
            self.diagnostics
                .push(type_diagnostics::numeric_literal_out_of_range(
                    span,
                    spelling,
                    self.type_name(type_id),
                ));
            None
        }
    }

    pub(crate) fn float_literal_expression(
        &mut self,
        value: &str,
        expected: Option<TypeId>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let type_id = expected
            .filter(|type_id| {
                matches!(self.numeric_target(*type_id), Some(NumericTarget::Float(_)))
            })
            .or_else(|| self.resolver.arena().source_type("Float"))?;
        let NumericTarget::Float(kind) = self.numeric_target(type_id)? else {
            return None;
        };
        match FloatValue::parse_decimal(value, kind) {
            Ok(value) => Some(TypedExpression {
                kind: TypedExpressionKind::Float(value),
                type_id,
                span,
            }),
            Err(_) => {
                self.diagnostics
                    .push(type_diagnostics::numeric_literal_out_of_range(
                        span,
                        value,
                        self.type_name(type_id),
                    ));
                None
            }
        }
    }

    pub(crate) fn primitive_expression(
        &self,
        kind: TypedExpressionKind,
        name: &str,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        Some(TypedExpression {
            kind,
            type_id: self.resolver.arena().source_type(name)?,
            span,
        })
    }

    fn check_interpolated_string(
        &mut self,
        segments: &[pop_syntax::StringSegmentSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let string = self.resolver.arena().source_type("String")?;
        let mut composed: Option<TypedExpression> = None;
        for segment in segments {
            let value = match segment.kind() {
                StringSegmentSyntaxKind::Text(value) => TypedExpression {
                    kind: TypedExpressionKind::String(value.clone()),
                    type_id: string,
                    span: segment.span(),
                },
                StringSegmentSyntaxKind::Expression(expression) => {
                    let value = self.check_expression(expression)?;
                    if value.type_id() == string {
                        value
                    } else {
                        let kind = match self.resolver.arena().get(value.type_id()) {
                            Some(SemanticType::Primitive(PrimitiveType::Boolean)) => {
                                StringFormatKind::Boolean
                            }
                            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
                                StringFormatKind::Integer(*kind)
                            }
                            Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
                                StringFormatKind::Float(FloatKind::Float32)
                            }
                            Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
                                StringFormatKind::Float(FloatKind::Float64)
                            }
                            _ => {
                                self.diagnostics.push(type_diagnostics::invalid_operator(
                                    expression.span(),
                                    "string interpolation",
                                    self.type_name(value.type_id()),
                                ));
                                return None;
                            }
                        };
                        TypedExpression {
                            kind: TypedExpressionKind::StringFormat {
                                kind,
                                value: Box::new(value),
                            },
                            type_id: string,
                            span: expression.span(),
                        }
                    }
                }
            };
            composed = Some(if let Some(left) = composed {
                TypedExpression {
                    kind: TypedExpressionKind::StringConcat {
                        left: Box::new(left),
                        right: Box::new(value),
                    },
                    type_id: string,
                    span,
                }
            } else {
                value
            });
        }
        Some(composed.unwrap_or(TypedExpression {
            kind: TypedExpressionKind::String(String::new()),
            type_id: string,
            span,
        }))
    }

    pub(crate) fn check_name(
        &mut self,
        path: &[String],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        match self.check_bound_path(path, span) {
            BoundPathLookup::Found(bound) => return Some(bound),
            BoundPathLookup::Error => return None,
            BoundPathLookup::NotBound => {}
        }
        if path.len() >= 2 {
            if matches!(path, [iteration, end] if iteration == "Iteration" && end == "End") {
                let Some(expected) = expected else {
                    self.diagnostics
                        .push(type_diagnostics::generic_inference_failure(
                            span,
                            "T",
                            "Iteration.End requires an expected Iteration<T> type",
                        ));
                    return None;
                };
                let protocol = self.resolver.schema().iteration_protocol()?;
                if !matches!(
                    self.resolver.arena().get(expected.type_id),
                    Some(SemanticType::Builtin { definition, arguments })
                        if *definition == protocol.iteration() && arguments.len() == 1
                ) {
                    self.diagnostics.push(type_diagnostics::type_mismatch(
                        span,
                        "Iteration<T>",
                        self.type_name(expected.type_id),
                        span,
                    ));
                    return None;
                }
                return Some(TypedExpression {
                    kind: TypedExpressionKind::IterationCase {
                        iteration: protocol.iteration(),
                        case: protocol.end_case(),
                        arguments: Vec::new(),
                    },
                    type_id: expected.type_id,
                    span,
                });
            }
            let type_name = path[..path.len() - 1].join(".");
            let resolution =
                self.resolver
                    .database()
                    .resolve(self.module, &type_name, SymbolSpace::Type, span);
            if let Some(symbol) = resolution.symbol()
                && let Some(definition) = self.resolver.enum_definition(symbol).cloned()
            {
                let case_name = &path[path.len() - 1];
                let Some(case) = definition
                    .cases()
                    .iter()
                    .find(|case| case.name() == case_name)
                else {
                    self.diagnostics
                        .push(resolution_diagnostics::unknown_name(span, path.join(".")));
                    return None;
                };
                return Some(TypedExpression {
                    kind: TypedExpressionKind::EnumCase {
                        definition: definition.symbol(),
                        case: case.case(),
                        discriminant: case.discriminant(),
                    },
                    type_id: definition.type_id(),
                    span,
                });
            }
        }
        match self.lookup_union_case(path, span) {
            UnionCaseLookup::Found(definition, case) => {
                if !case.parameters().is_empty() {
                    self.diagnostics.push(type_diagnostics::wrong_value_arity(
                        span,
                        "union case",
                        case.parameters().len(),
                        0,
                    ));
                    return None;
                }
                return Some(TypedExpression {
                    kind: TypedExpressionKind::UnionCase {
                        union: definition.symbol(),
                        case: case.case(),
                        arguments: Vec::new(),
                    },
                    type_id: definition.type_id(),
                    span,
                });
            }
            UnionCaseLookup::Missing => return None,
            UnionCaseLookup::NotUnion => {}
        }
        match self.lookup_error_case(path, span) {
            ErrorCaseLookup::Found(definition, case) => {
                if !case.parameters().is_empty() {
                    self.diagnostics.push(type_diagnostics::wrong_value_arity(
                        span,
                        "error case",
                        case.parameters().len(),
                        0,
                    ));
                    return None;
                }
                return Some(TypedExpression {
                    kind: TypedExpressionKind::ErrorCase {
                        error: definition.error(),
                        case: case.case(),
                        arguments: Vec::new(),
                    },
                    type_id: definition.type_id(),
                    span,
                });
            }
            ErrorCaseLookup::Missing => return None,
            ErrorCaseLookup::NotError => {}
        }
        let name = path.join(".");
        let resolution =
            self.resolver
                .database()
                .resolve(self.module, &name, SymbolSpace::Value, span);
        if !resolution.diagnostics().is_empty() {
            self.diagnostics
                .extend(resolution.diagnostics().iter().cloned());
            return None;
        }
        if resolution.symbols().len() > 1 {
            self.diagnostics
                .push(resolution_diagnostics::ambiguous_name(
                    span,
                    &name,
                    resolution.symbols().iter().filter_map(|symbol| {
                        self.resolver
                            .database()
                            .index()
                            .declaration(*symbol)
                            .map(pop_resolve::Declaration::span)
                    }),
                ));
            return None;
        }
        let symbol = resolution.symbol()?;
        if let Some(constant) = self.constants.and_then(|constants| constants.get(&symbol)) {
            return self.runtime_constant_expression(constant, span);
        }
        let signature = self.signatures.get(&symbol)?;
        let parameters: Option<Vec<_>> = signature
            .parameters()
            .iter()
            .map(|parameter| parameter.parameter_type().type_id())
            .collect();
        let results: Option<Vec<_>> = signature
            .results()
            .iter()
            .map(crate::ResolvedType::type_id)
            .collect();
        let type_id = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Function {
                is_async: signature.is_async(),
                parameters: parameters?,
                results: results?,
                effects: crate::EffectSummary::empty(),
            })
            .ok()?;
        Some(TypedExpression {
            kind: TypedExpressionKind::Function(symbol),
            type_id,
            span,
        })
    }

    fn runtime_constant_expression(
        &self,
        constant: &RuntimeConstant,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        self.runtime_constant_value(constant.value(), constant.type_id(), span)
    }

    fn runtime_constant_value(
        &self,
        value: &AttributeConstant,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let kind = match value {
            AttributeConstant::Nil => TypedExpressionKind::Nil,
            AttributeConstant::Boolean(value) => TypedExpressionKind::Boolean(*value),
            AttributeConstant::Integer(value) => TypedExpressionKind::Integer(*value),
            AttributeConstant::Float(value) => TypedExpressionKind::Float(*value),
            AttributeConstant::String(value) => TypedExpressionKind::String(value.clone()),
            AttributeConstant::Tuple(values) => {
                let Some(SemanticType::Tuple(element_types)) = self.resolver.arena().get(type_id)
                else {
                    return None;
                };
                if values.len() != element_types.len() {
                    return None;
                }
                let elements = values
                    .iter()
                    .zip(element_types)
                    .map(|(value, type_id)| self.runtime_constant_value(value, *type_id, span))
                    .collect::<Option<Vec<_>>>()?;
                TypedExpressionKind::Tuple(elements)
            }
        };
        Some(TypedExpression {
            kind,
            type_id,
            span,
        })
    }

    pub(crate) fn check_bound_path(
        &mut self,
        path: &[String],
        span: SourceSpan,
    ) -> BoundPathLookup {
        let Some(name) = path.first() else {
            return BoundPathLookup::NotBound;
        };
        let Some(binding) = self.binding_by_name(name) else {
            return BoundPathLookup::NotBound;
        };
        let Some(kind) = self.binding_reference_kind(binding) else {
            return BoundPathLookup::Error;
        };
        let mut expression = TypedExpression {
            kind,
            type_id: binding.type_id,
            span,
        };
        let effective_type = self.effective_binding_type(binding);
        if effective_type != binding.type_id {
            expression = TypedExpression {
                kind: TypedExpressionKind::OptionalNarrow {
                    optional: Box::new(expression),
                },
                type_id: effective_type,
                span,
            };
        }
        for field_name in &path[1..] {
            if let Some(definition) = self
                .resolver
                .record_definition_for_type(expression.type_id())
                .cloned()
            {
                let Some(field) = definition
                    .fields()
                    .iter()
                    .find(|field| field.name() == field_name)
                else {
                    self.diagnostics
                        .push(type_diagnostics::unknown_record_field(span, field_name));
                    return BoundPathLookup::Error;
                };
                expression =
                    typed_field_access(expression, field.field(), field.field_type(), span);
                continue;
            }
            if let Some(definition) = self
                .resolver
                .class_definition_for_type(expression.type_id())
                .cloned()
            {
                let Some(field) = definition
                    .fields()
                    .iter()
                    .find(|field| field.name() == field_name)
                else {
                    self.diagnostics
                        .push(type_diagnostics::unknown_record_field(span, field_name));
                    return BoundPathLookup::Error;
                };
                if !self.can_access_class_member(&definition, field.visibility()) {
                    self.diagnostics
                        .push(resolution_diagnostics::inaccessible_name(
                            span,
                            field.name(),
                            field.span(),
                        ));
                    return BoundPathLookup::Error;
                }
                expression =
                    typed_field_access(expression, field.field(), field.field_type(), span);
                continue;
            }
            {
                self.diagnostics
                    .push(type_diagnostics::unknown_record_field(span, field_name));
                return BoundPathLookup::Error;
            }
        }
        BoundPathLookup::Found(expression)
    }

    pub(crate) fn binding_by_name(&self, name: &str) -> Option<Binding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .copied()
    }

    pub(crate) fn effective_binding_type(&self, binding: Binding) -> TypeId {
        if self.function_depth == binding.function_depth
            && let Some(narrowed) = self
                .flow_narrowings
                .iter()
                .rev()
                .find_map(|facts| facts.get(&binding.id))
        {
            return *narrowed;
        }
        binding.type_id
    }

    pub(crate) fn optional_inner(&mut self, type_id: TypeId) -> Option<TypeId> {
        let nil = self.resolver.arena().source_type("nil")?;
        let SemanticType::Union(members) = self.resolver.arena().get(type_id)?.clone() else {
            return None;
        };
        if !members.contains(&nil) {
            return None;
        }
        let present = members
            .into_iter()
            .filter(|member| *member != nil)
            .collect::<Vec<_>>();
        match present.as_slice() {
            [inner] => Some(*inner),
            [] => None,
            _ => self.resolver.arena_mut().union(present).ok(),
        }
    }

    pub(crate) fn invalidate_flow_binding(&mut self, binding: BindingId) {
        for facts in &mut self.flow_narrowings {
            facts.remove(&binding);
        }
    }

    pub(crate) fn invalidate_flow_narrowings(&mut self) {
        for facts in &mut self.flow_narrowings {
            facts.clear();
        }
    }

    fn check_optional_propagate(
        &mut self,
        operand: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let optional = self.check_expression(operand)?;
        let Some(inner_type) = self.optional_inner(optional.type_id()) else {
            self.invalid_operator(span, "postfix ?", &[optional.type_id()]);
            return None;
        };
        let Some(signature) = self.signature_stack.last() else {
            self.invalid_operator(span, "postfix ?", &[optional.type_id()]);
            return None;
        };
        if signature.results().len() != 1 {
            self.invalid_operator(span, "postfix ?", &[optional.type_id()]);
            return None;
        }
        let enclosing_result = signature.results()[0].type_id()?;
        if self.optional_inner(enclosing_result).is_none() {
            self.invalid_operator(span, "postfix ?", &[optional.type_id(), enclosing_result]);
            return None;
        }
        Some(TypedExpression {
            kind: TypedExpressionKind::OptionalPropagate {
                optional: Box::new(optional),
                enclosing_result,
            },
            type_id: inner_type,
            span,
        })
    }

    fn check_result_propagate(
        &mut self,
        operand: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let result = self.check_expression(operand)?;
        let Some((success_type, error_type)) = self.resolver.result_parts(result.type_id()) else {
            self.diagnostics
                .push(type_diagnostics::invalid_result_propagation(
                    span,
                    self.type_name(result.type_id()),
                    "non-Result function",
                ));
            return None;
        };
        let Some(signature) = self.signature_stack.last() else {
            self.diagnostics
                .push(type_diagnostics::invalid_result_propagation(
                    span,
                    self.type_name(result.type_id()),
                    "no enclosing function",
                ));
            return None;
        };
        if signature.results().len() != 1 {
            self.diagnostics
                .push(type_diagnostics::invalid_result_propagation(
                    span,
                    self.type_name(result.type_id()),
                    "enclosing function does not return one Result",
                ));
            return None;
        }
        let enclosing_result = signature.results()[0].type_id()?;
        let Some((_, enclosing_error)) = self.resolver.result_parts(enclosing_result) else {
            self.diagnostics
                .push(type_diagnostics::invalid_result_propagation(
                    span,
                    self.type_name(result.type_id()),
                    self.type_name(enclosing_result),
                ));
            return None;
        };
        if error_type != enclosing_error {
            self.diagnostics
                .push(type_diagnostics::invalid_result_propagation(
                    span,
                    self.type_name(result.type_id()),
                    self.type_name(enclosing_result),
                ));
            return None;
        }
        Some(TypedExpression {
            kind: TypedExpressionKind::ResultPropagate {
                result: Box::new(result),
                result_definition: self.resolver.result_definition()?,
                success_type,
                error_type,
                enclosing_result,
            },
            type_id: success_type,
            span,
        })
    }

    pub(crate) fn binding_reference_kind(
        &mut self,
        binding: Binding,
    ) -> Option<TypedExpressionKind> {
        if self.function_depth > binding.function_depth {
            return self
                .record_capture(binding)
                .map(TypedExpressionKind::Capture);
        }
        Some(match binding.kind {
            BindingKind::Local(local)
            | BindingKind::LoopLocal(local)
            | BindingKind::ImmutableLocal(local) => TypedExpressionKind::Local(local),
            BindingKind::Parameter(parameter) => TypedExpressionKind::Parameter(parameter),
        })
    }

    pub(crate) fn record_capture(&mut self, binding: Binding) -> Option<CaptureId> {
        let mut source = binding.kind.capture_source();
        let mut current = None;
        for function in &mut self.active_functions {
            if function.depth <= binding.function_depth {
                continue;
            }
            let pending = if let Some(existing) = function.captures.get(&binding.id).copied() {
                existing
            } else {
                let capture = CaptureId::from_raw(function.next_capture);
                function.next_capture = function.next_capture.saturating_add(1);
                let pending = PendingCapture {
                    capture,
                    binding: binding.id,
                    source,
                    type_id: binding.type_id,
                };
                function.captures.insert(binding.id, pending);
                pending
            };
            source = CaptureSource::Capture(pending.capture);
            current = Some(pending.capture);
        }
        current
    }
}

fn typed_field_access(
    base: TypedExpression,
    field: FieldId,
    field_type: TypeId,
    span: SourceSpan,
) -> TypedExpression {
    TypedExpression {
        kind: TypedExpressionKind::Field {
            base: Box::new(base),
            field,
        },
        type_id: field_type,
        span,
    }
}

pub(crate) fn typed_field_default(
    default: &crate::FieldDefault,
    type_id: TypeId,
    span: SourceSpan,
) -> TypedExpression {
    let kind = match default {
        crate::FieldDefault::Nil => TypedExpressionKind::Nil,
        crate::FieldDefault::Boolean(value) => TypedExpressionKind::Boolean(*value),
        crate::FieldDefault::Integer(value) => TypedExpressionKind::Integer(*value),
        crate::FieldDefault::Float(value) => TypedExpressionKind::Float(*value),
        crate::FieldDefault::String(value) => TypedExpressionKind::String(value.clone()),
    };
    TypedExpression {
        kind,
        type_id,
        span,
    }
}

pub(crate) fn statements_definitely_return(statements: &[TypedStatement]) -> bool {
    statements.iter().any(|statement| match statement.kind() {
        TypedStatementKind::Return { .. } => true,
        TypedStatementKind::If {
            then_body,
            else_body,
            ..
        }
        | TypedStatementKind::OptionalIf {
            then_body,
            else_body,
            ..
        } => {
            !else_body.is_empty()
                && statements_definitely_return(then_body)
                && statements_definitely_return(else_body)
        }
        TypedStatementKind::Match { arms, .. } => {
            !arms.is_empty()
                && arms
                    .iter()
                    .all(|arm| statements_definitely_return(arm.body()))
        }
        TypedStatementKind::ErrorMatch { arms, .. } => {
            !arms.is_empty()
                && arms
                    .iter()
                    .all(|arm| statements_definitely_return(arm.body()))
        }
        TypedStatementKind::ResultMatch { arms, .. } => {
            !arms.is_empty()
                && arms
                    .iter()
                    .all(|arm| statements_definitely_return(arm.body()))
        }
        TypedStatementKind::Local { .. }
        | TypedStatementKind::MultipleLocal { .. }
        | TypedStatementKind::LocalSet { .. }
        | TypedStatementKind::ParameterSet { .. }
        | TypedStatementKind::CaptureSet { .. }
        | TypedStatementKind::While { .. }
        | TypedStatementKind::OptionalWhile { .. }
        | TypedStatementKind::NumericFor { .. }
        | TypedStatementKind::GeneralizedFor { .. }
        | TypedStatementKind::Defer { .. }
        | TypedStatementKind::AsyncDefer { .. }
        | TypedStatementKind::Break
        | TypedStatementKind::Continue
        | TypedStatementKind::FieldSet { .. }
        | TypedStatementKind::CompoundFieldSet { .. }
        | TypedStatementKind::ArraySet { .. }
        | TypedStatementKind::ListSet { .. }
        | TypedStatementKind::TableSet { .. }
        | TypedStatementKind::CompoundArraySet { .. }
        | TypedStatementKind::MultipleAssignment { .. }
        | TypedStatementKind::Call(_)
        | TypedStatementKind::Expression(_) => false,
        TypedStatementKind::RepeatUntil { body, .. } => statements_definitely_return(body),
    })
}

pub(crate) fn missing_match_arms(
    union_name: &str,
    cases: &[&crate::UnionCaseDefinition],
) -> String {
    let mut replacement = String::new();
    for case in cases {
        replacement.push_str("when ");
        replacement.push_str(union_name);
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

pub(crate) const fn typed_unary(operator: SyntaxUnaryOperator) -> TypedUnaryOperator {
    match operator {
        SyntaxUnaryOperator::Not => TypedUnaryOperator::Not,
        SyntaxUnaryOperator::Negate => TypedUnaryOperator::Negate,
    }
}

pub(crate) fn typed_binary(operator: SyntaxBinaryOperator) -> TypedBinaryOperator {
    match operator {
        SyntaxBinaryOperator::Or => TypedBinaryOperator::Or,
        SyntaxBinaryOperator::OptionalDefault => {
            unreachable!("optional default has a distinct typed expression")
        }
        SyntaxBinaryOperator::And => TypedBinaryOperator::And,
        SyntaxBinaryOperator::Equal => TypedBinaryOperator::Equal,
        SyntaxBinaryOperator::NotEqual => TypedBinaryOperator::NotEqual,
        SyntaxBinaryOperator::LessThan => TypedBinaryOperator::LessThan,
        SyntaxBinaryOperator::LessThanOrEqual => TypedBinaryOperator::LessThanOrEqual,
        SyntaxBinaryOperator::GreaterThan => TypedBinaryOperator::GreaterThan,
        SyntaxBinaryOperator::GreaterThanOrEqual => TypedBinaryOperator::GreaterThanOrEqual,
        SyntaxBinaryOperator::Concat => unreachable!(),
        SyntaxBinaryOperator::Add => TypedBinaryOperator::Add,
        SyntaxBinaryOperator::Subtract => TypedBinaryOperator::Subtract,
        SyntaxBinaryOperator::Multiply => TypedBinaryOperator::Multiply,
        SyntaxBinaryOperator::Divide => TypedBinaryOperator::Divide,
        SyntaxBinaryOperator::Remainder => TypedBinaryOperator::Remainder,
    }
}

pub(crate) const fn unary_text(operator: SyntaxUnaryOperator) -> &'static str {
    match operator {
        SyntaxUnaryOperator::Not => "not",
        SyntaxUnaryOperator::Negate => "unary -",
    }
}

pub(crate) const fn binary_text(operator: SyntaxBinaryOperator) -> &'static str {
    match operator {
        SyntaxBinaryOperator::Or => "or",
        SyntaxBinaryOperator::OptionalDefault => "??",
        SyntaxBinaryOperator::And => "and",
        SyntaxBinaryOperator::Equal => "==",
        SyntaxBinaryOperator::NotEqual => "~=",
        SyntaxBinaryOperator::LessThan => "<",
        SyntaxBinaryOperator::LessThanOrEqual => "<=",
        SyntaxBinaryOperator::GreaterThan => ">",
        SyntaxBinaryOperator::GreaterThanOrEqual => ">=",
        SyntaxBinaryOperator::Concat => "..",
        SyntaxBinaryOperator::Add => "+",
        SyntaxBinaryOperator::Subtract => "-",
        SyntaxBinaryOperator::Multiply => "*",
        SyntaxBinaryOperator::Divide => "/",
        SyntaxBinaryOperator::Remainder => "%",
    }
}

pub(crate) const fn primitive_name(primitive: PrimitiveType) -> &'static str {
    match primitive {
        PrimitiveType::Nil => "nil",
        PrimitiveType::Boolean => "Boolean",
        PrimitiveType::Integer(IntegerKind::Int8) => "Int8",
        PrimitiveType::Integer(IntegerKind::Int16) => "Int16",
        PrimitiveType::Integer(IntegerKind::Int32) => "Int32",
        PrimitiveType::Integer(IntegerKind::Int64) => "Int64",
        PrimitiveType::Integer(IntegerKind::UInt8) => "UInt8",
        PrimitiveType::Integer(IntegerKind::UInt16) => "UInt16",
        PrimitiveType::Integer(IntegerKind::UInt32) => "UInt32",
        PrimitiveType::Integer(IntegerKind::UInt64) => "UInt64",
        PrimitiveType::Float32 => "Float32",
        PrimitiveType::Float64 => "Float64",
        PrimitiveType::String => "String",
        PrimitiveType::Never => "Never",
    }
}
