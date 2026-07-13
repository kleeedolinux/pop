//! Deterministic evaluation budgets, results, usage, and failure provenance.
//!
//! These are data contracts shared by the interpreter, driver, and query
//! cache. Keeping them separate prevents execution machinery from leaking
//! into callers that only inspect a result or diagnostic chain.

use pop_foundation::{FunctionId, SourceSpan};
use pop_query::{BudgetError, QueryBudget};

use crate::model::{CompileTimeDependency, CompileTimeValue};

/// Deterministic resource envelope for one compile-time evaluation.
///
/// The generic query budget owns fuel, cumulative allocation, and call depth.
/// Compile-time evaluation additionally bounds the recursive live-value shape,
/// published output, and structured diagnostics as required by ADR 0023.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompileTimeBudget {
    pub(crate) query: QueryBudget,
    pub(crate) maximum_live_values: u64,
    pub(crate) maximum_output_bytes: u64,
    pub(crate) maximum_diagnostics: u64,
}

pub const DEFAULT_MAXIMUM_LIVE_VALUES: u64 = 65_536;
pub const DEFAULT_MAXIMUM_OUTPUT_BYTES: u64 = 1_048_576;
pub const DEFAULT_MAXIMUM_DIAGNOSTICS: u64 = 128;

impl CompileTimeBudget {
    #[must_use]
    pub const fn new(
        query: QueryBudget,
        maximum_live_values: u64,
        maximum_output_bytes: u64,
        maximum_diagnostics: u64,
    ) -> Self {
        Self {
            query,
            maximum_live_values,
            maximum_output_bytes,
            maximum_diagnostics,
        }
    }

    #[must_use]
    pub const fn query(self) -> QueryBudget {
        self.query
    }

    #[must_use]
    pub const fn maximum_live_values(self) -> u64 {
        self.maximum_live_values
    }

    #[must_use]
    pub const fn maximum_output_bytes(self) -> u64 {
        self.maximum_output_bytes
    }

    #[must_use]
    pub const fn maximum_diagnostics(self) -> u64 {
        self.maximum_diagnostics
    }
}

impl From<QueryBudget> for CompileTimeBudget {
    fn from(query: QueryBudget) -> Self {
        Self::new(
            query,
            DEFAULT_MAXIMUM_LIVE_VALUES,
            DEFAULT_MAXIMUM_OUTPUT_BYTES,
            DEFAULT_MAXIMUM_DIAGNOSTICS,
        )
    }
}

/// Canonical evaluation identity used by dependency tracking and cycle checks.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompileTimeEvaluationKey {
    pub(crate) function: FunctionId,
    pub(crate) arguments: Vec<CompileTimeValue>,
}

impl CompileTimeEvaluationKey {
    #[must_use]
    pub fn new(function: FunctionId, arguments: Vec<CompileTimeValue>) -> Self {
        Self {
            function,
            arguments,
        }
    }

    #[must_use]
    pub const fn function(&self) -> FunctionId {
        self.function
    }

    #[must_use]
    pub fn arguments(&self) -> &[CompileTimeValue] {
        &self.arguments
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationResult {
    pub(crate) value: CompileTimeValue,
    pub(crate) evaluation_key: CompileTimeEvaluationKey,
    pub(crate) origin: SourceSpan,
    pub(crate) function_dependencies: Vec<FunctionId>,
    pub(crate) dependencies: Vec<CompileTimeDependency>,
    pub(crate) budget: CompileTimeBudget,
    pub(crate) usage: EvaluationUsage,
}

impl EvaluationResult {
    #[must_use]
    pub const fn value(&self) -> &CompileTimeValue {
        &self.value
    }

    #[must_use]
    pub const fn evaluation_key(&self) -> &CompileTimeEvaluationKey {
        &self.evaluation_key
    }

    #[must_use]
    pub const fn origin(&self) -> SourceSpan {
        self.origin
    }

    #[must_use]
    pub fn function_dependencies(&self) -> &[FunctionId] {
        &self.function_dependencies
    }

    #[must_use]
    pub fn dependencies(&self) -> &[CompileTimeDependency] {
        &self.dependencies
    }

    #[must_use]
    pub const fn budget(&self) -> &CompileTimeBudget {
        &self.budget
    }

    #[must_use]
    pub const fn usage(&self) -> &EvaluationUsage {
        &self.usage
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EvaluationUsage {
    pub(crate) instructions: u64,
    pub(crate) allocated_bytes: u64,
    pub(crate) maximum_call_depth: u32,
    pub(crate) maximum_live_values: u64,
    pub(crate) output_bytes: u64,
    pub(crate) diagnostics: u64,
}

impl EvaluationUsage {
    #[must_use]
    pub const fn instructions(self) -> u64 {
        self.instructions
    }

    #[must_use]
    pub const fn allocated_bytes(self) -> u64 {
        self.allocated_bytes
    }

    #[must_use]
    pub const fn maximum_call_depth(self) -> u32 {
        self.maximum_call_depth
    }

    #[must_use]
    pub const fn maximum_live_values(self) -> u64 {
        self.maximum_live_values
    }

    #[must_use]
    pub const fn output_bytes(self) -> u64 {
        self.output_bytes
    }

    #[must_use]
    pub const fn diagnostics(self) -> u64 {
        self.diagnostics
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvaluationError {
    UnknownFunction(FunctionId),
    IneligibleFunction(FunctionId),
    WrongArity {
        function: FunctionId,
        expected: usize,
        found: usize,
    },
    TypeMismatch,
    IntegerOverflow,
    DivisionByZero,
    Budget(BudgetError),
}

impl From<BudgetError> for EvaluationError {
    fn from(error: BudgetError) -> Self {
        Self::Budget(error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EvaluationFailureKind {
    Error(EvaluationError),
    CallCycle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompileTimeCallFrame {
    pub(crate) function: FunctionId,
    pub(crate) call_site: SourceSpan,
}

impl CompileTimeCallFrame {
    #[must_use]
    pub const fn function(self) -> FunctionId {
        self.function
    }

    #[must_use]
    pub const fn call_site(self) -> SourceSpan {
        self.call_site
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EvaluationFailure {
    pub(crate) kind: EvaluationFailureKind,
    pub(crate) location: SourceSpan,
    pub(crate) context: Box<EvaluationFailureContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct EvaluationFailureContext {
    pub(crate) evaluation_key: CompileTimeEvaluationKey,
    pub(crate) origin: SourceSpan,
    pub(crate) call_chain: Vec<CompileTimeCallFrame>,
    pub(crate) dependencies: Vec<CompileTimeDependency>,
    pub(crate) budget: CompileTimeBudget,
    pub(crate) usage: EvaluationUsage,
}

impl EvaluationFailure {
    #[must_use]
    pub const fn kind(&self) -> EvaluationFailureKind {
        self.kind
    }

    #[must_use]
    pub const fn location(&self) -> SourceSpan {
        self.location
    }

    #[must_use]
    pub const fn evaluation_key(&self) -> &CompileTimeEvaluationKey {
        &self.context.evaluation_key
    }

    #[must_use]
    pub const fn origin(&self) -> SourceSpan {
        self.context.origin
    }

    #[must_use]
    pub fn call_chain(&self) -> &[CompileTimeCallFrame] {
        &self.context.call_chain
    }

    #[must_use]
    pub fn dependencies(&self) -> &[CompileTimeDependency] {
        &self.context.dependencies
    }

    #[must_use]
    pub const fn budget(&self) -> &CompileTimeBudget {
        &self.context.budget
    }

    #[must_use]
    pub const fn usage(&self) -> &EvaluationUsage {
        &self.context.usage
    }

    pub(crate) const fn legacy_error(&self) -> EvaluationError {
        match self.kind {
            EvaluationFailureKind::Error(error) => error,
            // The source-integrated driver will adopt `evaluate_detailed` when
            // it owns POP4006 provenance. Preserve the existing exhaustive
            // EvaluationError API until that coordinated change.
            EvaluationFailureKind::CallCycle => {
                EvaluationError::Budget(BudgetError::CallDepthLimit)
            }
        }
    }
}
