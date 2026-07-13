//! Driver integration for restricted compile-time analysis and evaluation.
//!
//! This module coordinates typed lowering, eligibility, dependency closure,
//! deterministic evaluation, defaults, and structured provenance. Execution
//! remains owned by `pop-compile-time`; the driver only sequences queries.

use std::collections::{BTreeMap, BTreeSet};

use pop_compile_time::{
    CompileTimeBudget, CompileTimeExpression, CompileTimeExpressionKind, CompileTimeFunction,
    CompileTimeInterpreter, CompileTimeLoweringError, CompileTimeProgram, CompileTimeValue,
    EvaluationError, EvaluationFailure, EvaluationFailureKind, ProgramError,
    lower_compile_time_expression, lower_compile_time_function,
};
use pop_diagnostics::compile_time as compile_time_diagnostics;
use pop_foundation::{
    Diagnostic, DiagnosticOrigin, DiagnosticOriginKind, FunctionId, ModuleId, SourceSpan, SymbolId,
    TypeId,
};
use pop_hir::{HirDeclaration, HirDeclarationKind};
use pop_query::{BudgetError, QueryBudget};
use pop_syntax::ExpressionSyntax;
use pop_types::{
    AttributeConstant, AttributeQueryIndex, BodyChecker, FieldDefault, PendingConstantExpression,
    ResolvedFunctionSignature, SignatureResolver, TypeArena, TypedBody,
};

use crate::api::{FrontEndCompileTimeEvaluation, FrontEndConstant};
use crate::work::{CompileTimeContext, ConstantWork, FunctionWork};

pub(crate) fn check_compile_time_function_bodies(
    functions: &[FunctionWork],
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> BTreeMap<SymbolId, TypedBody> {
    let mut bodies = BTreeMap::new();
    for function in functions {
        let typed = BodyChecker::new(function.module, resolver, signatures)
            .check(&function.signature, &function.body);
        if function.is_compile_time {
            diagnostics.extend(typed.diagnostics().iter().cloned());
        }
        if let Some(body) = typed.body() {
            bodies.insert(function.signature.symbol(), body.clone());
        }
    }
    bodies
}

pub(crate) fn build_compile_time_context(
    functions: &[FunctionWork],
    bodies: &BTreeMap<SymbolId, TypedBody>,
    types: &TypeArena,
    diagnostics: &mut Vec<Diagnostic>,
) -> CompileTimeContext {
    let mut lowered = BTreeMap::new();
    let mut eligible = BTreeSet::new();
    let mut names = BTreeMap::new();
    for function in functions {
        let id = FunctionId::from_raw(function.signature.symbol().raw());
        names.insert(id, function.signature.name().to_owned());
        let Some(body) = bodies.get(&function.signature.symbol()) else {
            continue;
        };
        match lower_compile_time_function(&function.signature, body, types) {
            Ok(definition) => {
                if function.is_compile_time {
                    eligible.insert(id);
                }
                lowered.insert(id, definition);
            }
            Err(error) if function.is_compile_time => {
                diagnostics.push(compile_time_diagnostics::forbidden_effect(
                    compile_time_lowering_span(error, function.span),
                    function.signature.name(),
                    format!("{error:?}"),
                    [],
                ));
            }
            Err(_) => {}
        }
    }
    for function in &eligible {
        let Some(definition) = lowered.get(function) else {
            continue;
        };
        for (called, span) in direct_calls(definition.body()) {
            if !eligible.contains(&called) {
                diagnostics.push(compile_time_diagnostics::function_not_eligible(
                    span,
                    compile_time_function_name(&names, called),
                    [],
                ));
            }
        }
    }
    CompileTimeContext {
        functions: lowered,
        eligible,
        names,
    }
}

fn compile_time_lowering_span(error: CompileTimeLoweringError, fallback: SourceSpan) -> SourceSpan {
    match error {
        CompileTimeLoweringError::MissingCanonicalType { span }
        | CompileTimeLoweringError::UnsupportedReturnArity { span, .. }
        | CompileTimeLoweringError::UnsupportedConstruct { span, .. }
        | CompileTimeLoweringError::UnknownParameter { span, .. }
        | CompileTimeLoweringError::TypeMismatch { span, .. }
        | CompileTimeLoweringError::InvalidOperatorTypes { span } => span,
        CompileTimeLoweringError::BodyDoesNotProduceSingleResult { span } => {
            span.unwrap_or(fallback)
        }
        CompileTimeLoweringError::UnsupportedResultArity { .. } => fallback,
    }
}

fn direct_calls(expression: &CompileTimeExpression) -> Vec<(FunctionId, SourceSpan)> {
    let mut calls = Vec::new();
    collect_direct_calls(expression, &mut calls);
    calls
}

fn collect_direct_calls(
    expression: &CompileTimeExpression,
    calls: &mut Vec<(FunctionId, SourceSpan)>,
) {
    match expression.kind() {
        CompileTimeExpressionKind::Constant(_)
        | CompileTimeExpressionKind::Parameter(_)
        | CompileTimeExpressionKind::Local(_) => {}
        CompileTimeExpressionKind::Let {
            initializer, body, ..
        }
        | CompileTimeExpressionKind::LetTuple {
            initializer, body, ..
        } => {
            collect_direct_calls(initializer, calls);
            collect_direct_calls(body, calls);
        }
        CompileTimeExpressionKind::Unary { operand, .. } => collect_direct_calls(operand, calls),
        CompileTimeExpressionKind::NumericConvert { value, .. } => {
            collect_direct_calls(value, calls);
        }
        CompileTimeExpressionKind::Binary { left, right, .. } => {
            collect_direct_calls(left, calls);
            collect_direct_calls(right, calls);
        }
        CompileTimeExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            collect_direct_calls(condition, calls);
            collect_direct_calls(when_true, calls);
            collect_direct_calls(when_false, calls);
        }
        CompileTimeExpressionKind::Tuple(elements) => {
            for element in elements {
                collect_direct_calls(element, calls);
            }
        }
        CompileTimeExpressionKind::TupleGet { tuple, .. } => {
            collect_direct_calls(tuple, calls);
        }
        CompileTimeExpressionKind::Call {
            function,
            arguments,
        } => {
            for argument in arguments {
                collect_direct_calls(argument, calls);
            }
            calls.push((*function, expression.span()));
        }
        CompileTimeExpressionKind::AttributeQuery { .. } => {}
    }
}

pub(crate) fn evaluate_declaration_defaults(
    declarations: &[HirDeclaration],
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for declaration in declarations {
        match declaration.kind() {
            HirDeclarationKind::Attribute(_) => evaluate_attribute_defaults(
                declaration,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
                diagnostics,
            ),
            HirDeclarationKind::Record(_) => evaluate_record_defaults(
                declaration,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
                diagnostics,
            ),
            HirDeclarationKind::Class(_) => evaluate_class_defaults(
                declaration,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
                diagnostics,
            ),
            HirDeclarationKind::Union(_) | HirDeclarationKind::Interface(_) => {}
        }
    }
}

pub(crate) fn evaluate_source_constants(
    constants: &[ConstantWork],
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    attribute_queries: &AttributeQueryIndex,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<FrontEndConstant> {
    let mut evaluated = Vec::new();
    for constant in constants {
        let expected = if let Some(annotation) = constant.syntax.annotation() {
            let (resolved, type_diagnostics) =
                resolver.resolve_standalone_type(constant.module, annotation);
            diagnostics.extend(type_diagnostics);
            resolved
        } else {
            None
        };
        if constant.syntax.annotation().is_some() && expected.is_none() {
            continue;
        }
        let typed = BodyChecker::new(constant.module, resolver, signatures)
            .check_constant_expression(constant.syntax.initializer(), expected);
        diagnostics.extend(typed.diagnostics().iter().cloned());
        let Some(expression) = typed.expression() else {
            continue;
        };
        let type_id = expression.type_id();
        let lowered = match lower_compile_time_expression(expression, resolver.arena()) {
            Ok(lowered) => lowered,
            Err(error) => {
                diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                    compile_time_lowering_span(error, constant.syntax.span()),
                    "constant initializer",
                ));
                continue;
            }
        };
        match evaluate_compile_time_expression(
            lowered,
            compile_time,
            resolver.arena(),
            Some(attribute_queries),
            compile_time_evaluations,
        ) {
            Ok(value) => evaluated.push(FrontEndConstant {
                symbol: constant.symbol,
                name: constant.syntax.name().to_owned(),
                type_id,
                value,
            }),
            Err(errors) => diagnostics.extend(errors),
        }
    }
    evaluated
}

fn evaluate_attribute_defaults(
    declaration: &HirDeclaration,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pending: Vec<_> = resolver
        .attribute_definition(declaration.symbol())
        .into_iter()
        .flat_map(pop_types::AttributeDefinition::parameters)
        .filter_map(|parameter| {
            parameter.pending_default().map(|expression| {
                (
                    parameter.parameter(),
                    expression.clone(),
                    parameter.parameter_type(),
                )
            })
        })
        .collect();
    for (parameter, expression, expected) in pending {
        let evaluated = evaluate_required_expression(
            declaration.module(),
            expression.expression(),
            expected,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
        )
        .and_then(|value| {
            compile_time_attribute_constant(value).ok_or_else(|| {
                vec![compile_time_diagnostics::ineligible_constant_expression(
                    expression.expression().span(),
                    "attribute default",
                )]
            })
        });
        match evaluated {
            Ok(value) => {
                if resolver
                    .install_attribute_parameter_default(declaration.symbol(), parameter, value)
                    .is_err()
                {
                    diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                        expression.expression().span(),
                        "attribute default",
                    ));
                }
            }
            Err(errors) => diagnostics.extend(errors),
        }
    }
}

fn evaluate_record_defaults(
    declaration: &HirDeclaration,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pending: Vec<_> = resolver
        .record_definition(declaration.symbol())
        .into_iter()
        .flat_map(pop_types::RecordDefinition::fields)
        .filter_map(|field| {
            field
                .pending_default()
                .map(|expression| (field.field(), expression.clone(), field.field_type()))
        })
        .collect();
    for (field, expression, expected) in pending {
        install_field_default(
            declaration,
            field,
            &expression,
            expected,
            false,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
    }
}

fn evaluate_class_defaults(
    declaration: &HirDeclaration,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let pending: Vec<_> = resolver
        .class_definition(declaration.symbol())
        .into_iter()
        .flat_map(pop_types::ClassDefinition::fields)
        .filter_map(|field| {
            field
                .pending_default()
                .map(|expression| (field.field(), expression.clone(), field.field_type()))
        })
        .collect();
    for (field, expression, expected) in pending {
        install_field_default(
            declaration,
            field,
            &expression,
            expected,
            true,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn install_field_default(
    declaration: &HirDeclaration,
    field: pop_foundation::FieldId,
    pending: &PendingConstantExpression,
    expected: TypeId,
    class_field: bool,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let evaluated = evaluate_required_expression(
        declaration.module(),
        pending.expression(),
        expected,
        signatures,
        compile_time,
        resolver,
        compile_time_evaluations,
    )
    .and_then(|value| {
        compile_time_field_default(value).ok_or_else(|| {
            vec![compile_time_diagnostics::ineligible_constant_expression(
                pending.expression().span(),
                "field default",
            )]
        })
    });
    match evaluated {
        Ok(value) => {
            let installed = if class_field {
                resolver.install_class_field_default(declaration.symbol(), field, value)
            } else {
                resolver.install_record_field_default(declaration.symbol(), field, value)
            };
            if installed.is_err() {
                diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                    pending.expression().span(),
                    "field default",
                ));
            }
        }
        Err(errors) => diagnostics.extend(errors),
    }
}

pub(crate) fn evaluate_required_expression(
    module: ModuleId,
    expression: &ExpressionSyntax,
    expected: TypeId,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
) -> Result<CompileTimeValue, Vec<Diagnostic>> {
    let typed = BodyChecker::new(module, resolver, signatures)
        .check_required_expression(expression, expected);
    if !typed.diagnostics().is_empty() {
        return Err(typed.diagnostics().to_vec());
    }
    let Some(typed) = typed.expression() else {
        return Err(vec![
            compile_time_diagnostics::ineligible_constant_expression(
                expression.span(),
                "required constant",
            ),
        ]);
    };
    let lowered = lower_compile_time_expression(typed, resolver.arena()).map_err(|error| {
        vec![compile_time_diagnostics::ineligible_constant_expression(
            compile_time_lowering_span(error, expression.span()),
            "required constant",
        )]
    })?;
    evaluate_compile_time_expression(
        lowered,
        compile_time,
        resolver.arena(),
        None,
        compile_time_evaluations,
    )
}

pub(crate) fn evaluate_compile_time_expression(
    expression: CompileTimeExpression,
    context: &CompileTimeContext,
    types: &TypeArena,
    attribute_queries: Option<&AttributeQueryIndex>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
) -> Result<CompileTimeValue, Vec<Diagnostic>> {
    let mut selected = BTreeMap::new();
    collect_reachable_compile_time_functions(&expression, context, &mut selected)?;
    let wrapper = unused_function_id(context);
    let result_type = expression.type_id();
    let span = expression.span();
    let wrapper_function = CompileTimeFunction::new(wrapper, Vec::new(), result_type, expression);
    let mut definitions: Vec<_> = selected.into_values().collect();
    definitions.push(wrapper_function);
    let mut program = CompileTimeProgram::new(definitions, types)
        .map_err(|error| vec![program_diagnostic(error, span, context)])?;
    if let Some(attribute_queries) = attribute_queries {
        program = program.with_attribute_queries(attribute_queries.clone());
    }
    let mut eligible = context.eligible.clone();
    eligible.insert(wrapper);
    match CompileTimeInterpreter::new(&program, &eligible, default_compile_time_budget())
        .evaluate_detailed_from(wrapper, &[], span)
    {
        Ok(result) => {
            let value = result.value().clone();
            compile_time_evaluations.push(FrontEndCompileTimeEvaluation::Result(result));
            Ok(value)
        }
        Err(failure) => {
            let diagnostic = evaluation_diagnostic(&failure, context);
            compile_time_evaluations.push(FrontEndCompileTimeEvaluation::Failure(failure));
            Err(vec![diagnostic])
        }
    }
}

pub(crate) fn evaluate_compile_time_function(
    function: FunctionId,
    arguments: &[CompileTimeValue],
    origin: SourceSpan,
    context: &CompileTimeContext,
    types: &TypeArena,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
) -> Result<CompileTimeValue, Vec<Diagnostic>> {
    if !context.eligible.contains(&function) {
        return Err(vec![compile_time_diagnostics::function_not_eligible(
            origin,
            compile_time_function_name(&context.names, function),
            [],
        )]);
    }
    let Some(root) = context.functions.get(&function).cloned() else {
        return Err(vec![compile_time_diagnostics::function_not_eligible(
            origin,
            compile_time_function_name(&context.names, function),
            [],
        )]);
    };
    let mut selected = BTreeMap::from([(function, root.clone())]);
    collect_reachable_compile_time_functions(root.body(), context, &mut selected)?;
    let program = CompileTimeProgram::new(selected.into_values().collect(), types)
        .map_err(|error| vec![program_diagnostic(error, origin, context)])?;
    match CompileTimeInterpreter::new(&program, &context.eligible, default_compile_time_budget())
        .evaluate_detailed_from(function, arguments, origin)
    {
        Ok(result) => {
            let value = result.value().clone();
            compile_time_evaluations.push(FrontEndCompileTimeEvaluation::Result(result));
            Ok(value)
        }
        Err(failure) => {
            let diagnostic = evaluation_diagnostic(&failure, context);
            compile_time_evaluations.push(FrontEndCompileTimeEvaluation::Failure(failure));
            Err(vec![diagnostic])
        }
    }
}

fn collect_reachable_compile_time_functions(
    expression: &CompileTimeExpression,
    context: &CompileTimeContext,
    selected: &mut BTreeMap<FunctionId, CompileTimeFunction>,
) -> Result<(), Vec<Diagnostic>> {
    for (function, span) in direct_calls(expression) {
        if !context.eligible.contains(&function) {
            return Err(vec![compile_time_diagnostics::function_not_eligible(
                span,
                compile_time_function_name(&context.names, function),
                [],
            )]);
        }
        if selected.contains_key(&function) {
            continue;
        }
        let Some(definition) = context.functions.get(&function).cloned() else {
            return Err(vec![compile_time_diagnostics::function_not_eligible(
                span,
                compile_time_function_name(&context.names, function),
                [],
            )]);
        };
        selected.insert(function, definition.clone());
        collect_reachable_compile_time_functions(definition.body(), context, selected)?;
    }
    Ok(())
}

fn unused_function_id(context: &CompileTimeContext) -> FunctionId {
    let mut raw = context
        .functions
        .keys()
        .map(|function| function.raw())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    while context.functions.contains_key(&FunctionId::from_raw(raw)) {
        raw = raw.saturating_add(1);
    }
    FunctionId::from_raw(raw)
}

pub(crate) fn program_diagnostic(
    error: ProgramError,
    span: SourceSpan,
    context: &CompileTimeContext,
) -> Diagnostic {
    match error {
        ProgramError::UnknownFunction(function) => compile_time_diagnostics::function_not_eligible(
            span,
            compile_time_function_name(&context.names, function),
            [],
        ),
        _ => compile_time_diagnostics::ineligible_constant_expression(
            span,
            format!("invalid compile-time program: {error:?}"),
        ),
    }
}

fn evaluation_diagnostic(failure: &EvaluationFailure, context: &CompileTimeContext) -> Diagnostic {
    let origins: Vec<_> = failure
        .call_chain()
        .iter()
        .map(|frame| DiagnosticOrigin::new(frame.call_site(), DiagnosticOriginKind::CompileTime))
        .chain(std::iter::once(DiagnosticOrigin::new(
            failure.origin(),
            DiagnosticOriginKind::CompileTime,
        )))
        .collect();
    let span = failure.location();
    let EvaluationFailureKind::Error(error) = failure.kind() else {
        let cycle = failure
            .call_chain()
            .iter()
            .map(|frame| compile_time_function_name(&context.names, frame.function()))
            .collect::<Vec<_>>()
            .join(" -> ");
        return compile_time_diagnostics::cycle(span, cycle, origins);
    };
    let diagnostic = match error {
        EvaluationError::UnknownFunction(function)
        | EvaluationError::IneligibleFunction(function) => {
            compile_time_diagnostics::function_not_eligible(
                span,
                compile_time_function_name(&context.names, function),
                [],
            )
        }
        EvaluationError::IntegerOverflow => {
            compile_time_diagnostics::constant_integer_overflow(span, "required constant")
        }
        EvaluationError::DivisionByZero => {
            compile_time_diagnostics::constant_division_by_zero(span, "required constant")
        }
        EvaluationError::Budget(resource) => compile_time_diagnostics::resource_limit(
            span,
            budget_name(resource),
            budget_limit(resource, *failure.budget()),
            [],
        ),
        EvaluationError::WrongArity { .. } | EvaluationError::TypeMismatch => {
            compile_time_diagnostics::ineligible_constant_expression(span, "required constant")
        }
    };
    origins
        .into_iter()
        .fold(diagnostic, Diagnostic::with_origin)
}

const fn default_compile_time_budget() -> CompileTimeBudget {
    CompileTimeBudget::new(
        QueryBudget::new(100_000, 1_048_576, 128),
        65_536,
        1_048_576,
        128,
    )
}

const fn budget_name(error: BudgetError) -> &'static str {
    match error {
        BudgetError::InstructionLimit => "instructions",
        BudgetError::AllocationLimit => "allocation bytes",
        BudgetError::CallDepthLimit => "call depth",
        BudgetError::LiveValueLimit => "live values",
        BudgetError::OutputSizeLimit => "output bytes",
        BudgetError::DiagnosticLimit => "diagnostics",
        BudgetError::UnbalancedCallExit => "call stack",
    }
}

const fn budget_limit(error: BudgetError, budget: CompileTimeBudget) -> u64 {
    match error {
        BudgetError::InstructionLimit => budget.query().instruction_fuel(),
        BudgetError::AllocationLimit => budget.query().allocation_bytes(),
        BudgetError::CallDepthLimit => budget.query().maximum_call_depth() as u64,
        BudgetError::LiveValueLimit => budget.maximum_live_values(),
        BudgetError::OutputSizeLimit => budget.maximum_output_bytes(),
        BudgetError::DiagnosticLimit => budget.maximum_diagnostics(),
        BudgetError::UnbalancedCallExit => 0,
    }
}

pub(crate) fn compile_time_function_name(
    names: &BTreeMap<FunctionId, String>,
    function: FunctionId,
) -> String {
    names
        .get(&function)
        .cloned()
        .unwrap_or_else(|| format!("function#{}", function.raw()))
}

pub(crate) fn compile_time_attribute_constant(
    value: CompileTimeValue,
) -> Option<AttributeConstant> {
    match value {
        CompileTimeValue::Nil => Some(AttributeConstant::Nil),
        CompileTimeValue::Boolean(value) => Some(AttributeConstant::Boolean(value)),
        CompileTimeValue::Integer(value) => Some(AttributeConstant::Integer(value)),
        CompileTimeValue::Float(value) => Some(AttributeConstant::Float(value)),
        CompileTimeValue::String(value) => Some(AttributeConstant::String(value)),
        CompileTimeValue::Tuple(values) => values
            .into_iter()
            .map(compile_time_attribute_constant)
            .collect::<Option<Vec<_>>>()
            .map(AttributeConstant::Tuple),
        CompileTimeValue::Array(_)
        | CompileTimeValue::Attribute { .. }
        | CompileTimeValue::Record(_)
        | CompileTimeValue::Union { .. }
        | CompileTimeValue::TypeReference(_)
        | CompileTimeValue::SymbolReference(_) => None,
    }
}

fn compile_time_field_default(value: CompileTimeValue) -> Option<FieldDefault> {
    match value {
        CompileTimeValue::Nil => Some(FieldDefault::Nil),
        CompileTimeValue::Boolean(value) => Some(FieldDefault::Boolean(value)),
        CompileTimeValue::Integer(value) => Some(FieldDefault::Integer(value)),
        CompileTimeValue::Float(value) => Some(FieldDefault::Float(value)),
        CompileTimeValue::String(value) => Some(FieldDefault::String(value)),
        CompileTimeValue::Tuple(_)
        | CompileTimeValue::Array(_)
        | CompileTimeValue::Attribute { .. }
        | CompileTimeValue::Record(_)
        | CompileTimeValue::Union { .. }
        | CompileTimeValue::TypeReference(_)
        | CompileTimeValue::SymbolReference(_) => None,
    }
}
