//! Deterministic, budgeted interpreter for verified compile-time HIR.
//!
//! Evaluation is host/target independent and capability-limited. Every call,
//! allocation, dependency, and failure keeps typed provenance; ambient state
//! and backend handles are structurally unavailable.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    AttributeId, FileId, FunctionId, LocalId, ModuleId, SourceSpan, TextRange, TextSize, TypeId,
};
use pop_query::{BudgetError, BudgetTracker};
use pop_types::{
    AttributeQuerySubject, AttributeQueryValue, FloatKind, NumericError, SemanticType,
};

use crate::evaluation::*;
use crate::model::*;
use crate::program::{CompileTimeProgram, resolved_attribute_value, value_matches_type};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ActiveCall {
    function: FunctionId,
    arguments: Vec<CompileTimeValue>,
    frame: CompileTimeCallFrame,
}

pub struct CompileTimeInterpreter<'program> {
    pub(crate) program: &'program CompileTimeProgram,
    pub(crate) eligible: &'program BTreeSet<FunctionId>,
    pub(crate) budget: CompileTimeBudget,
    pub(crate) tracker: BudgetTracker,
    pub(crate) dependencies: BTreeSet<CompileTimeDependency>,
    pub(crate) active_calls: Vec<ActiveCall>,
    pub(crate) evaluation_key: Option<CompileTimeEvaluationKey>,
    pub(crate) origin: SourceSpan,
    pub(crate) maximum_live_values: u64,
    pub(crate) temporary_live_values: u64,
    pub(crate) output_bytes: u64,
    pub(crate) diagnostics: u64,
}

impl<'program> CompileTimeInterpreter<'program> {
    #[must_use]
    pub fn new<B: Into<CompileTimeBudget>>(
        program: &'program CompileTimeProgram,
        eligible: &'program BTreeSet<FunctionId>,
        budget: B,
    ) -> Self {
        let budget = budget.into();
        Self {
            program,
            eligible,
            budget,
            tracker: BudgetTracker::new(budget.query()),
            dependencies: BTreeSet::new(),
            active_calls: Vec::new(),
            evaluation_key: None,
            origin: empty_source_span(),
            maximum_live_values: 0,
            temporary_live_values: 0,
            output_bytes: 0,
            diagnostics: 0,
        }
    }

    /// Records structured diagnostics already produced by the restricted
    /// compile-time effect/query layer that shares this evaluation envelope.
    /// The count is checked before execution and published in usage data.
    #[must_use]
    pub const fn with_recorded_diagnostics(mut self, diagnostics: u64) -> Self {
        self.diagnostics = diagnostics;
        self
    }

    /// Evaluates one explicitly eligible compile-time function.
    ///
    /// # Errors
    ///
    /// Returns a deterministic semantic, eligibility, type, or budget error.
    pub fn evaluate(
        self,
        function: FunctionId,
        arguments: &[CompileTimeValue],
    ) -> Result<EvaluationResult, EvaluationError> {
        self.evaluate_detailed(function, arguments)
            .map_err(|failure| failure.legacy_error())
    }

    /// Evaluates with deterministic dependency, usage, call-chain, and cycle
    /// information suitable for structured compile-time diagnostics.
    ///
    /// # Errors
    ///
    /// Returns a provenance-carrying failure without collapsing an active
    /// evaluation-key cycle into a generic call-depth exhaustion.
    pub fn evaluate_detailed(
        self,
        function: FunctionId,
        arguments: &[CompileTimeValue],
    ) -> Result<EvaluationResult, EvaluationFailure> {
        let origin = self
            .program
            .function(function)
            .map_or_else(empty_source_span, |definition| definition.body().span());
        self.evaluate_detailed_from(function, arguments, origin)
    }

    /// Evaluates one compile-time request while retaining the requesting UDA,
    /// constant, or other source origin independently of nested call sites.
    ///
    /// # Errors
    ///
    /// Returns a failure carrying the canonical root key, origin, dependency
    /// set, resource envelope, and compile-time call chain.
    pub fn evaluate_detailed_from(
        mut self,
        function: FunctionId,
        arguments: &[CompileTimeValue],
        origin: SourceSpan,
    ) -> Result<EvaluationResult, EvaluationFailure> {
        let evaluation_key = CompileTimeEvaluationKey::new(function, arguments.to_vec());
        self.evaluation_key = Some(evaluation_key.clone());
        self.origin = origin;
        self.dependencies.insert(CompileTimeDependency::Compiler {
            compiler_version: env!("CARGO_PKG_VERSION"),
            compile_time_ir_version: COMPILE_TIME_IR_VERSION,
        });
        let call_site = self
            .program
            .function(function)
            .map_or(origin, |definition| definition.body().span());
        if self.diagnostics > self.budget.maximum_diagnostics() {
            return Err(self.failure_with_chain(
                EvaluationFailureKind::Error(EvaluationError::Budget(BudgetError::DiagnosticLimit)),
                origin,
                vec![CompileTimeCallFrame {
                    function,
                    call_site,
                }],
            ));
        }
        let value = self.evaluate_call(function, arguments, call_site)?;
        self.output_bytes = value_size(&value);
        if self.output_bytes > self.budget.maximum_output_bytes() {
            return Err(self.failure_with_chain(
                EvaluationFailureKind::Error(EvaluationError::Budget(BudgetError::OutputSizeLimit)),
                origin,
                vec![CompileTimeCallFrame {
                    function,
                    call_site,
                }],
            ));
        }
        let dependencies: Vec<_> = self.dependencies.iter().cloned().collect();
        let function_dependencies = dependencies
            .iter()
            .filter_map(|dependency| match dependency {
                CompileTimeDependency::Function(function) => Some(*function),
                _ => None,
            })
            .collect();
        Ok(EvaluationResult {
            value,
            evaluation_key,
            origin,
            function_dependencies,
            dependencies,
            budget: self.budget,
            usage: self.usage(),
        })
    }

    fn evaluate_call(
        &mut self,
        function: FunctionId,
        arguments: &[CompileTimeValue],
        call_site: SourceSpan,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        let (definition, frame) = self.prepare_call(function, arguments, call_site)?;
        if let Err(error) = self.tracker.enter_call() {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::Budget(error)),
                call_site,
            ));
        }
        self.active_calls.push(ActiveCall {
            function,
            arguments: arguments.to_vec(),
            frame,
        });
        let value = self.evaluate_expression(definition.body(), arguments, &mut BTreeMap::new());
        self.active_calls.pop();
        let exit = self.tracker.exit_call();
        let value = value?;
        if let Err(error) = exit {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::Budget(error)),
                call_site,
            ));
        }
        if value_matches_type(
            &value,
            definition.result(),
            self.program.types(),
            self.program.metadata(),
        ) {
            Ok(value)
        } else {
            Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                definition.body().span(),
            ))
        }
    }

    fn prepare_call(
        &mut self,
        function: FunctionId,
        arguments: &[CompileTimeValue],
        call_site: SourceSpan,
    ) -> Result<(CompileTimeFunction, CompileTimeCallFrame), EvaluationFailure> {
        if !self.eligible.contains(&function) {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::IneligibleFunction(function)),
                call_site,
            ));
        }
        let Some(definition) = self.program.function(function).cloned() else {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::UnknownFunction(function)),
                call_site,
            ));
        };
        if definition.parameters().len() != arguments.len() {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::WrongArity {
                    function,
                    expected: definition.parameters().len(),
                    found: arguments.len(),
                }),
                call_site,
            ));
        }
        if arguments
            .iter()
            .zip(definition.parameters())
            .any(|(argument, type_id)| {
                !value_matches_type(
                    argument,
                    *type_id,
                    self.program.types(),
                    self.program.metadata(),
                )
            })
        {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                call_site,
            ));
        }
        self.dependencies
            .insert(CompileTimeDependency::Function(function));
        self.dependencies
            .insert(CompileTimeDependency::CanonicalArguments {
                function,
                arguments: arguments.to_vec(),
            });
        for type_id in definition
            .parameters()
            .iter()
            .copied()
            .chain(std::iter::once(definition.result()))
        {
            self.record_type_dependency(type_id);
        }
        for argument in arguments {
            self.record_value_dependencies(argument);
        }
        let frame = CompileTimeCallFrame {
            function,
            call_site,
        };
        if let Some(position) = self.active_calls.iter().position(|active| {
            active.function == function && active.arguments.as_slice() == arguments
        }) {
            let mut call_chain: Vec<_> = self.active_calls[position..]
                .iter()
                .map(|active| active.frame)
                .collect();
            call_chain.push(frame);
            return Err(self.failure_with_chain(
                EvaluationFailureKind::CallCycle,
                call_site,
                call_chain,
            ));
        }
        Ok((definition, frame))
    }

    fn evaluate_expression(
        &mut self,
        expression: &CompileTimeExpression,
        parameters: &[CompileTimeValue],
        locals: &mut BTreeMap<LocalId, CompileTimeValue>,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        if let Err(error) = self.tracker.consume_instructions(1) {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::Budget(error)),
                expression.span(),
            ));
        }
        self.record_type_dependency(expression.type_id());
        let value = match expression.kind() {
            CompileTimeExpressionKind::Constant(value) => {
                self.evaluate_constant(value, expression.span())
            }
            CompileTimeExpressionKind::Parameter(index) => {
                self.evaluate_parameter(*index, parameters, expression.span())
            }
            CompileTimeExpressionKind::Local(local) => {
                locals.get(local).cloned().ok_or_else(|| {
                    self.failure(
                        EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                        expression.span(),
                    )
                })
            }
            CompileTimeExpressionKind::Let {
                local,
                initializer,
                body,
                ..
            } => self.evaluate_let(*local, initializer, body, parameters, locals),
            CompileTimeExpressionKind::LetTuple {
                locals: bindings,
                initializer,
                body,
            } => self.evaluate_let_tuple(bindings, initializer, body, parameters, locals),
            CompileTimeExpressionKind::Unary { operator, operand } => {
                let operand = self.evaluate_expression(operand, parameters, locals)?;
                evaluate_unary(*operator, operand).map_err(|error| {
                    self.failure(EvaluationFailureKind::Error(error), expression.span())
                })
            }
            CompileTimeExpressionKind::Binary {
                operator,
                left,
                right,
            } => self.evaluate_binary_expression(
                *operator,
                left,
                right,
                parameters,
                locals,
                expression.span(),
            ),
            CompileTimeExpressionKind::NumericConvert { conversion, value } => {
                let value = self.evaluate_expression(value, parameters, locals)?;
                evaluate_numeric_conversion(*conversion, value).map_err(|error| {
                    self.failure(EvaluationFailureKind::Error(error), expression.span())
                })
            }
            CompileTimeExpressionKind::Conditional {
                condition,
                when_true,
                when_false,
            } => match self.evaluate_expression(condition, parameters, locals)? {
                CompileTimeValue::Boolean(true) => {
                    self.evaluate_expression(when_true, parameters, locals)
                }
                CompileTimeValue::Boolean(false) => {
                    self.evaluate_expression(when_false, parameters, locals)
                }
                _ => Err(self.failure(
                    EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                    expression.span(),
                )),
            },
            CompileTimeExpressionKind::Tuple(elements) => {
                self.evaluate_tuple(elements, parameters, locals, expression.span())
            }
            CompileTimeExpressionKind::TupleGet { tuple, index } => {
                match self.evaluate_expression(tuple, parameters, locals)? {
                    CompileTimeValue::Tuple(values) => {
                        values.get(*index as usize).cloned().ok_or_else(|| {
                            self.failure(
                                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                                expression.span(),
                            )
                        })
                    }
                    _ => Err(self.failure(
                        EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                        expression.span(),
                    )),
                }
            }
            CompileTimeExpressionKind::Call {
                function,
                arguments,
            } => {
                let mut values = Vec::with_capacity(arguments.len());
                let mut held_values = 0_u64;
                for argument in arguments {
                    let value = self.evaluate_expression_with_temporaries(
                        argument,
                        parameters,
                        locals,
                        held_values,
                    )?;
                    held_values = held_values.saturating_add(value_count(&value));
                    values.push(value);
                }
                self.evaluate_call(*function, &values, expression.span())
            }
            CompileTimeExpressionKind::AttributeQuery {
                module,
                attribute,
                subject,
                has_only,
            } => self.evaluate_attribute_query(
                *module,
                *attribute,
                *subject,
                *has_only,
                expression.span(),
            ),
        }?;
        self.observe_live_value(&value, locals, expression.span())?;
        Ok(value)
    }

    fn evaluate_attribute_query(
        &mut self,
        module: ModuleId,
        attribute: AttributeId,
        subject: AttributeQuerySubject,
        has_only: bool,
        span: SourceSpan,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        self.dependencies
            .insert(CompileTimeDependency::Attribute(attribute));
        match subject {
            AttributeQuerySubject::Symbol(symbol) => {
                self.dependencies
                    .insert(CompileTimeDependency::Symbol(symbol));
            }
            AttributeQuerySubject::Type(type_id) => self.record_type_dependency(type_id),
        }
        let Some(queries) = self.program.attribute_queries() else {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                span,
            ));
        };
        if has_only {
            return queries
                .has_attribute(module, subject, attribute)
                .map(CompileTimeValue::Boolean)
                .map_err(|_| {
                    self.failure(
                        EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                        span,
                    )
                });
        }
        let value = queries.attribute(module, subject, attribute).map_err(|_| {
            self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                span,
            )
        })?;
        Ok(match value {
            AttributeQueryValue::Optional(None) => CompileTimeValue::Nil,
            AttributeQueryValue::Optional(Some(value)) => resolved_attribute_value(value),
            AttributeQueryValue::ImmutableSequence(values) => {
                CompileTimeValue::Array(values.iter().map(resolved_attribute_value).collect())
            }
        })
    }

    fn evaluate_constant(
        &mut self,
        value: &CompileTimeValue,
        span: SourceSpan,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        self.record_value_dependencies(value);
        if let Err(error) = self.tracker.allocate(value_size(value)) {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::Budget(error)),
                span,
            ));
        }
        Ok(value.clone())
    }

    fn evaluate_parameter(
        &self,
        index: u32,
        parameters: &[CompileTimeValue],
        span: SourceSpan,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        let Ok(index) = usize::try_from(index) else {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                span,
            ));
        };
        parameters.get(index).cloned().ok_or_else(|| {
            self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                span,
            )
        })
    }

    fn evaluate_let(
        &mut self,
        local: LocalId,
        initializer: &CompileTimeExpression,
        body: &CompileTimeExpression,
        parameters: &[CompileTimeValue],
        locals: &mut BTreeMap<LocalId, CompileTimeValue>,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        let value = self.evaluate_expression(initializer, parameters, locals)?;
        let previous = locals.insert(local, value);
        let result = self.evaluate_expression(body, parameters, locals);
        if let Some(previous) = previous {
            locals.insert(local, previous);
        } else {
            locals.remove(&local);
        }
        result
    }

    fn evaluate_let_tuple(
        &mut self,
        bindings: &[(LocalId, TypeId)],
        initializer: &CompileTimeExpression,
        body: &CompileTimeExpression,
        parameters: &[CompileTimeValue],
        locals: &mut BTreeMap<LocalId, CompileTimeValue>,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        let CompileTimeValue::Tuple(values) =
            self.evaluate_expression(initializer, parameters, locals)?
        else {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                initializer.span(),
            ));
        };
        if bindings.len() != values.len() {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                initializer.span(),
            ));
        }
        let mut previous = Vec::with_capacity(bindings.len());
        for ((local, _), value) in bindings.iter().zip(values) {
            previous.push((*local, locals.insert(*local, value)));
        }
        let result = self.evaluate_expression(body, parameters, locals);
        for (local, value) in previous.into_iter().rev() {
            if let Some(value) = value {
                locals.insert(local, value);
            } else {
                locals.remove(&local);
            }
        }
        result
    }

    fn evaluate_tuple(
        &mut self,
        elements: &[CompileTimeExpression],
        parameters: &[CompileTimeValue],
        locals: &mut BTreeMap<LocalId, CompileTimeValue>,
        span: SourceSpan,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        let mut values = Vec::with_capacity(elements.len());
        let mut held_values = 0_u64;
        for element in elements {
            let value = self.evaluate_expression_with_temporaries(
                element,
                parameters,
                locals,
                held_values,
            )?;
            held_values = held_values.saturating_add(value_count(&value));
            values.push(value);
        }
        let bytes = u64::try_from(values.len())
            .unwrap_or(u64::MAX)
            .saturating_mul(8);
        if let Err(error) = self.tracker.allocate(bytes) {
            return Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::Budget(error)),
                span,
            ));
        }
        Ok(CompileTimeValue::Tuple(values))
    }

    fn evaluate_binary_expression(
        &mut self,
        operator: CompileTimeBinaryOperator,
        left: &CompileTimeExpression,
        right: &CompileTimeExpression,
        parameters: &[CompileTimeValue],
        locals: &mut BTreeMap<LocalId, CompileTimeValue>,
        span: SourceSpan,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        let left = self.evaluate_expression(left, parameters, locals)?;
        match (operator, left) {
            (CompileTimeBinaryOperator::And, CompileTimeValue::Boolean(false)) => {
                Ok(CompileTimeValue::Boolean(false))
            }
            (CompileTimeBinaryOperator::Or, CompileTimeValue::Boolean(true)) => {
                Ok(CompileTimeValue::Boolean(true))
            }
            (
                CompileTimeBinaryOperator::And | CompileTimeBinaryOperator::Or,
                CompileTimeValue::Boolean(left),
            ) => {
                let right =
                    self.evaluate_expression_with_temporaries(right, parameters, locals, 1)?;
                evaluate_boolean_binary(operator, CompileTimeValue::Boolean(left), right)
                    .map_err(|error| self.failure(EvaluationFailureKind::Error(error), span))
            }
            (CompileTimeBinaryOperator::And | CompileTimeBinaryOperator::Or, _) => Err(self
                .failure(
                    EvaluationFailureKind::Error(EvaluationError::TypeMismatch),
                    span,
                )),
            (_, left) => {
                let right = self.evaluate_expression_with_temporaries(
                    right,
                    parameters,
                    locals,
                    value_count(&left),
                )?;
                evaluate_binary(operator, left, right)
                    .map_err(|error| self.failure(EvaluationFailureKind::Error(error), span))
            }
        }
    }

    fn evaluate_expression_with_temporaries(
        &mut self,
        expression: &CompileTimeExpression,
        parameters: &[CompileTimeValue],
        locals: &mut BTreeMap<LocalId, CompileTimeValue>,
        additional_live_values: u64,
    ) -> Result<CompileTimeValue, EvaluationFailure> {
        let previous = self.temporary_live_values;
        self.temporary_live_values = previous.saturating_add(additional_live_values);
        let result = self.evaluate_expression(expression, parameters, locals);
        self.temporary_live_values = previous;
        result
    }

    fn observe_live_value(
        &mut self,
        value: &CompileTimeValue,
        locals: &BTreeMap<LocalId, CompileTimeValue>,
        span: SourceSpan,
    ) -> Result<(), EvaluationFailure> {
        let active_arguments = self
            .active_calls
            .iter()
            .flat_map(|call| &call.arguments)
            .map(value_count)
            .fold(0_u64, u64::saturating_add);
        let local_values = locals
            .values()
            .map(value_count)
            .fold(0_u64, u64::saturating_add);
        let live = active_arguments
            .saturating_add(local_values)
            .saturating_add(self.temporary_live_values)
            .saturating_add(value_count(value));
        self.maximum_live_values = self.maximum_live_values.max(live);
        if live > self.budget.maximum_live_values() {
            Err(self.failure(
                EvaluationFailureKind::Error(EvaluationError::Budget(BudgetError::LiveValueLimit)),
                span,
            ))
        } else {
            Ok(())
        }
    }

    fn record_type_dependency(&mut self, type_id: TypeId) {
        if !self
            .dependencies
            .insert(CompileTimeDependency::Type(type_id))
        {
            return;
        }
        let Some(semantic) = self.program.types().get(type_id).cloned() else {
            return;
        };
        match semantic {
            SemanticType::Tuple(elements) | SemanticType::Union(elements) => {
                for element in elements {
                    self.record_type_dependency(element);
                }
            }
            SemanticType::Function {
                parameters,
                results,
                ..
            } => {
                for type_id in parameters.into_iter().chain(results) {
                    self.record_type_dependency(type_id);
                }
            }
            SemanticType::Record(fields) => {
                for (_, field_type) in fields {
                    self.record_type_dependency(field_type);
                }
            }
            SemanticType::TaggedUnion { definition, .. } => {
                self.dependencies
                    .insert(CompileTimeDependency::Symbol(definition));
            }
            SemanticType::Enum { definition } => {
                self.dependencies
                    .insert(CompileTimeDependency::Symbol(definition));
            }
            SemanticType::Attribute {
                attribute,
                parameters,
            } => {
                self.dependencies
                    .insert(CompileTimeDependency::Attribute(attribute));
                for parameter in parameters {
                    self.record_type_dependency(parameter);
                }
            }
            SemanticType::Array(element) | SemanticType::Optional(element) => {
                self.record_type_dependency(element);
            }
            SemanticType::Table { key, value } => {
                self.record_type_dependency(key);
                self.record_type_dependency(value);
            }
            SemanticType::Class { arguments, .. }
            | SemanticType::Interface { arguments, .. }
            | SemanticType::Builtin { arguments, .. } => {
                for argument in arguments {
                    self.record_type_dependency(argument);
                }
            }
            SemanticType::Primitive(_)
            | SemanticType::TypeParameter(_)
            | SemanticType::Opaque(_)
            | SemanticType::Error => {}
        }
    }

    fn record_value_dependencies(&mut self, value: &CompileTimeValue) {
        match value {
            CompileTimeValue::Tuple(values) | CompileTimeValue::Array(values) => {
                for value in values {
                    self.record_value_dependencies(value);
                }
            }
            CompileTimeValue::Record(fields) => {
                for (field, value) in fields {
                    self.dependencies
                        .insert(CompileTimeDependency::Field(*field));
                    self.record_value_dependencies(value);
                }
            }
            CompileTimeValue::Attribute {
                attribute,
                arguments,
            } => {
                self.dependencies
                    .insert(CompileTimeDependency::Attribute(*attribute));
                for argument in arguments {
                    self.record_value_dependencies(argument);
                }
            }
            CompileTimeValue::Union {
                union,
                case,
                arguments,
            } => {
                self.dependencies
                    .insert(CompileTimeDependency::Symbol(*union));
                self.dependencies.insert(CompileTimeDependency::UnionCase {
                    union: *union,
                    case: *case,
                });
                for argument in arguments {
                    self.record_value_dependencies(argument);
                }
            }
            CompileTimeValue::TypeReference(type_id) => {
                self.record_type_dependency(*type_id);
            }
            CompileTimeValue::SymbolReference(symbol) => {
                self.dependencies
                    .insert(CompileTimeDependency::Symbol(*symbol));
            }
            CompileTimeValue::Nil
            | CompileTimeValue::Boolean(_)
            | CompileTimeValue::Integer(_)
            | CompileTimeValue::Float(_)
            | CompileTimeValue::String(_) => {}
        }
    }

    fn failure(&self, kind: EvaluationFailureKind, location: SourceSpan) -> EvaluationFailure {
        self.failure_with_chain(
            kind,
            location,
            self.active_calls
                .iter()
                .map(|active| active.frame)
                .collect(),
        )
    }

    fn failure_with_chain(
        &self,
        kind: EvaluationFailureKind,
        location: SourceSpan,
        call_chain: Vec<CompileTimeCallFrame>,
    ) -> EvaluationFailure {
        EvaluationFailure {
            kind,
            location,
            context: Box::new(EvaluationFailureContext {
                evaluation_key: self.evaluation_key.clone().unwrap_or_else(|| {
                    CompileTimeEvaluationKey::new(FunctionId::from_raw(u32::MAX), Vec::new())
                }),
                origin: self.origin,
                call_chain,
                dependencies: self.dependencies.iter().cloned().collect(),
                budget: self.budget,
                usage: self.usage(),
            }),
        }
    }

    fn usage(&self) -> EvaluationUsage {
        EvaluationUsage {
            instructions: self.tracker.instructions(),
            allocated_bytes: self.tracker.allocation_bytes(),
            maximum_call_depth: self.tracker.maximum_call_depth(),
            maximum_live_values: self.maximum_live_values,
            output_bytes: self.output_bytes,
            diagnostics: self.diagnostics,
        }
    }
}

fn empty_source_span() -> SourceSpan {
    SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)))
}

fn evaluate_binary(
    operator: CompileTimeBinaryOperator,
    left: CompileTimeValue,
    right: CompileTimeValue,
) -> Result<CompileTimeValue, EvaluationError> {
    match operator {
        CompileTimeBinaryOperator::CheckedAdd
        | CompileTimeBinaryOperator::CheckedSubtract
        | CompileTimeBinaryOperator::CheckedMultiply
        | CompileTimeBinaryOperator::CheckedDivide
        | CompileTimeBinaryOperator::CheckedRemainder => {
            evaluate_integer_binary(operator, left, right)
        }
        CompileTimeBinaryOperator::FloatAdd
        | CompileTimeBinaryOperator::FloatSubtract
        | CompileTimeBinaryOperator::FloatMultiply
        | CompileTimeBinaryOperator::FloatDivide => evaluate_float_binary(operator, left, right),
        CompileTimeBinaryOperator::Equal => Ok(CompileTimeValue::Boolean(left == right)),
        CompileTimeBinaryOperator::NotEqual => Ok(CompileTimeValue::Boolean(left != right)),
        CompileTimeBinaryOperator::LessThan
        | CompileTimeBinaryOperator::LessThanOrEqual
        | CompileTimeBinaryOperator::GreaterThan
        | CompileTimeBinaryOperator::GreaterThanOrEqual => evaluate_ordering(operator, left, right),
        CompileTimeBinaryOperator::And | CompileTimeBinaryOperator::Or => {
            evaluate_boolean_binary(operator, left, right)
        }
    }
}

fn evaluate_integer_binary(
    operator: CompileTimeBinaryOperator,
    left: CompileTimeValue,
    right: CompileTimeValue,
) -> Result<CompileTimeValue, EvaluationError> {
    let (CompileTimeValue::Integer(left), CompileTimeValue::Integer(right)) = (left, right) else {
        return Err(EvaluationError::TypeMismatch);
    };
    let value = match operator {
        CompileTimeBinaryOperator::CheckedAdd => left.checked_add(right),
        CompileTimeBinaryOperator::CheckedSubtract => left.checked_subtract(right),
        CompileTimeBinaryOperator::CheckedMultiply => left.checked_multiply(right),
        CompileTimeBinaryOperator::CheckedDivide => left.checked_divide(right),
        CompileTimeBinaryOperator::CheckedRemainder => left.checked_remainder(right),
        _ => return Err(EvaluationError::TypeMismatch),
    };
    value
        .map(CompileTimeValue::Integer)
        .map_err(numeric_evaluation_error)
}

fn evaluate_float_binary(
    operator: CompileTimeBinaryOperator,
    left: CompileTimeValue,
    right: CompileTimeValue,
) -> Result<CompileTimeValue, EvaluationError> {
    let (CompileTimeValue::Float(left), CompileTimeValue::Float(right)) = (left, right) else {
        return Err(EvaluationError::TypeMismatch);
    };
    let value = match operator {
        CompileTimeBinaryOperator::FloatAdd => left.checked_add(right),
        CompileTimeBinaryOperator::FloatSubtract => left.checked_subtract(right),
        CompileTimeBinaryOperator::FloatMultiply => left.checked_multiply(right),
        CompileTimeBinaryOperator::FloatDivide => left.checked_divide(right),
        _ => return Err(EvaluationError::TypeMismatch),
    };
    value
        .map(CompileTimeValue::Float)
        .map_err(numeric_evaluation_error)
}

fn evaluate_ordering(
    operator: CompileTimeBinaryOperator,
    left: CompileTimeValue,
    right: CompileTimeValue,
) -> Result<CompileTimeValue, EvaluationError> {
    let ordering = match (left, right) {
        (CompileTimeValue::Integer(left), CompileTimeValue::Integer(right)) => {
            Some(left.compare(right).map_err(numeric_evaluation_error)?)
        }
        (CompileTimeValue::Float(left), CompileTimeValue::Float(right)) => left
            .partial_compare(right)
            .map_err(numeric_evaluation_error)?,
        _ => return Err(EvaluationError::TypeMismatch),
    };
    let value = matches!(
        (operator, ordering),
        (CompileTimeBinaryOperator::LessThan, Some(Ordering::Less))
            | (
                CompileTimeBinaryOperator::LessThanOrEqual,
                Some(Ordering::Less | Ordering::Equal)
            )
            | (
                CompileTimeBinaryOperator::GreaterThan,
                Some(Ordering::Greater)
            )
            | (
                CompileTimeBinaryOperator::GreaterThanOrEqual,
                Some(Ordering::Greater | Ordering::Equal)
            )
    );
    Ok(CompileTimeValue::Boolean(value))
}

fn evaluate_numeric_conversion(
    conversion: pop_types::NumericConversionKind,
    value: CompileTimeValue,
) -> Result<CompileTimeValue, EvaluationError> {
    use pop_types::NumericConversionKind;
    match (conversion, value) {
        (
            NumericConversionKind::IntegerToInteger { source, target },
            CompileTimeValue::Integer(value),
        ) if value.kind() == source => value
            .convert(target)
            .map(CompileTimeValue::Integer)
            .map_err(numeric_evaluation_error),
        (
            NumericConversionKind::IntegerToFloat { source, target },
            CompileTimeValue::Integer(value),
        ) if value.kind() == source => Ok(CompileTimeValue::Float(value.to_float(target))),
        (
            NumericConversionKind::FloatToInteger { source, target },
            CompileTimeValue::Float(value),
        ) if value.kind() == source => value
            .to_integer(target)
            .map(CompileTimeValue::Integer)
            .map_err(numeric_evaluation_error),
        (
            NumericConversionKind::FloatToFloat { source, target },
            CompileTimeValue::Float(value),
        ) if value.kind() == source => Ok(CompileTimeValue::Float(value.convert(target))),
        _ => Err(EvaluationError::TypeMismatch),
    }
}

fn evaluate_boolean_binary(
    operator: CompileTimeBinaryOperator,
    left: CompileTimeValue,
    right: CompileTimeValue,
) -> Result<CompileTimeValue, EvaluationError> {
    let (CompileTimeValue::Boolean(left), CompileTimeValue::Boolean(right)) = (left, right) else {
        return Err(EvaluationError::TypeMismatch);
    };
    let value = match operator {
        CompileTimeBinaryOperator::And => left && right,
        CompileTimeBinaryOperator::Or => left || right,
        _ => return Err(EvaluationError::TypeMismatch),
    };
    Ok(CompileTimeValue::Boolean(value))
}

fn evaluate_unary(
    operator: CompileTimeUnaryOperator,
    operand: CompileTimeValue,
) -> Result<CompileTimeValue, EvaluationError> {
    match (operator, operand) {
        (CompileTimeUnaryOperator::CheckedIntegerNegate, CompileTimeValue::Integer(value)) => value
            .checked_negate()
            .map(CompileTimeValue::Integer)
            .map_err(numeric_evaluation_error),
        (CompileTimeUnaryOperator::FloatNegate, CompileTimeValue::Float(value)) => {
            Ok(CompileTimeValue::Float(value.negate()))
        }
        (CompileTimeUnaryOperator::BooleanNot, CompileTimeValue::Boolean(value)) => {
            Ok(CompileTimeValue::Boolean(!value))
        }
        _ => Err(EvaluationError::TypeMismatch),
    }
}

const fn numeric_evaluation_error(error: NumericError) -> EvaluationError {
    match error {
        NumericError::Overflow | NumericError::OutOfRange => EvaluationError::IntegerOverflow,
        NumericError::DivisionByZero => EvaluationError::DivisionByZero,
        NumericError::InvalidLiteral | NumericError::KindMismatch => EvaluationError::TypeMismatch,
    }
}

fn value_size(value: &CompileTimeValue) -> u64 {
    match value {
        CompileTimeValue::Nil => 0,
        CompileTimeValue::Boolean(_) => 1,
        CompileTimeValue::Integer(value) => u64::from(value.kind().bit_width() / 8),
        CompileTimeValue::Float(value) => match value.kind() {
            FloatKind::Float32 => 4,
            FloatKind::Float64 => 8,
        },
        CompileTimeValue::TypeReference(_) | CompileTimeValue::SymbolReference(_) => 8,
        CompileTimeValue::String(value) => u64::try_from(value.len()).unwrap_or(u64::MAX),
        CompileTimeValue::Tuple(values) | CompileTimeValue::Array(values) => {
            values.iter().map(value_size).fold(
                u64::try_from(values.len())
                    .unwrap_or(u64::MAX)
                    .saturating_mul(8),
                u64::saturating_add,
            )
        }
        CompileTimeValue::Record(fields) => fields
            .iter()
            .map(|(_, value)| 4_u64.saturating_add(value_size(value)))
            .fold(0_u64, u64::saturating_add),
        CompileTimeValue::Attribute { arguments, .. }
        | CompileTimeValue::Union { arguments, .. } => arguments
            .iter()
            .map(value_size)
            .fold(8_u64, u64::saturating_add),
    }
}

fn value_count(value: &CompileTimeValue) -> u64 {
    match value {
        CompileTimeValue::Tuple(values) | CompileTimeValue::Array(values) => values
            .iter()
            .map(value_count)
            .fold(1_u64, u64::saturating_add),
        CompileTimeValue::Record(fields) => fields
            .iter()
            .map(|(_, value)| value_count(value))
            .fold(1_u64, u64::saturating_add),
        CompileTimeValue::Attribute { arguments, .. }
        | CompileTimeValue::Union { arguments, .. } => arguments
            .iter()
            .map(value_count)
            .fold(1_u64, u64::saturating_add),
        CompileTimeValue::Nil
        | CompileTimeValue::Boolean(_)
        | CompileTimeValue::Integer(_)
        | CompileTimeValue::Float(_)
        | CompileTimeValue::String(_)
        | CompileTimeValue::TypeReference(_)
        | CompileTimeValue::SymbolReference(_) => 1,
    }
}
