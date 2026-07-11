//! Restricted typed compile-time HIR, canonical values, and deterministic evaluation.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    AttributeId, FieldId, FileId, FunctionId, LocalId, ModuleId, SourceSpan, SymbolId, TextRange,
    TextSize, TypeId, UnionCaseId,
};
use pop_query::{BudgetError, BudgetTracker, QueryBudget};
use pop_types::{
    AttributeConstant, AttributeQueryIndex, AttributeQuerySubject, AttributeQueryValue, FloatKind,
    FloatValue, IntegerValue, NumericError, PrimitiveType, ResolvedAttribute,
    ResolvedFunctionSignature, SemanticType, TypeArena, TypedBinaryOperator, TypedBody,
    TypedCallDispatch, TypedExpression, TypedExpressionKind, TypedStatement, TypedStatementKind,
    TypedUnaryOperator,
};

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CompileTimeValue {
    Nil,
    Boolean(bool),
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
    Tuple(Vec<Self>),
    Array(Vec<Self>),
    Attribute {
        attribute: AttributeId,
        arguments: Vec<Self>,
    },
    Record(Vec<(FieldId, Self)>),
    Union {
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<Self>,
    },
    TypeReference(TypeId),
    SymbolReference(SymbolId),
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CompileTimeHandleKind {
    Type,
    Symbol,
}

/// Compile-time-only type facts that are deliberately absent from runtime HIR
/// type metadata.
///
/// The resolver/type-checker owns these identities. Keeping them explicit here
/// lets the compile-time verifier reject forged handles and malformed aggregate
/// values without making compiler handles valid runtime [`SemanticType`]s.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompileTimeTypeMetadata {
    handle_types: BTreeMap<TypeId, CompileTimeHandleKind>,
    types: BTreeSet<TypeId>,
    symbols: BTreeSet<SymbolId>,
    records: BTreeMap<TypeId, Vec<(FieldId, TypeId)>>,
    union_cases: BTreeMap<(SymbolId, UnionCaseId), Vec<TypeId>>,
}

impl CompileTimeTypeMetadata {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            handle_types: BTreeMap::new(),
            types: BTreeSet::new(),
            symbols: BTreeSet::new(),
            records: BTreeMap::new(),
            union_cases: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn with_handle_type(mut self, type_id: TypeId, kind: CompileTimeHandleKind) -> Self {
        self.handle_types.insert(type_id, kind);
        self
    }

    /// Registers a compiler-owned type identity that may be carried by an
    /// opaque compile-time type handle.
    #[must_use]
    pub fn with_type(mut self, type_id: TypeId) -> Self {
        self.types.insert(type_id);
        self
    }

    #[must_use]
    pub fn with_symbol(mut self, symbol: SymbolId) -> Self {
        self.symbols.insert(symbol);
        self
    }

    #[must_use]
    pub fn with_record(mut self, type_id: TypeId, mut fields: Vec<(FieldId, TypeId)>) -> Self {
        fields.sort_by_key(|(field, _)| *field);
        self.records.insert(type_id, fields);
        self
    }

    #[must_use]
    pub fn with_union_case(
        mut self,
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<TypeId>,
    ) -> Self {
        self.union_cases.insert((union, case), arguments);
        self
    }

    fn handle_kind(&self, type_id: TypeId) -> Option<CompileTimeHandleKind> {
        self.handle_types.get(&type_id).copied()
    }

    fn contains_symbol(&self, symbol: SymbolId) -> bool {
        self.symbols.contains(&symbol)
    }

    fn contains_type(&self, type_id: TypeId) -> bool {
        self.types.contains(&type_id)
    }

    fn record(&self, type_id: TypeId) -> Option<&[(FieldId, TypeId)]> {
        self.records.get(&type_id).map(Vec::as_slice)
    }

    fn union_case(&self, union: SymbolId, case: UnionCaseId) -> Option<&[TypeId]> {
        self.union_cases.get(&(union, case)).map(Vec::as_slice)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CompileTimeDependency {
    Compiler {
        compiler_version: &'static str,
        compile_time_ir_version: u32,
    },
    Function(FunctionId),
    CanonicalArguments {
        function: FunctionId,
        arguments: Vec<CompileTimeValue>,
    },
    Type(TypeId),
    Symbol(SymbolId),
    Attribute(AttributeId),
    Field(FieldId),
    UnionCase {
        union: SymbolId,
        case: UnionCaseId,
    },
}

pub const COMPILE_TIME_IR_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileTimeBinaryOperator {
    CheckedAdd,
    CheckedSubtract,
    CheckedMultiply,
    CheckedDivide,
    CheckedRemainder,
    FloatAdd,
    FloatSubtract,
    FloatMultiply,
    FloatDivide,
    Equal,
    NotEqual,
    LessThan,
    GreaterThan,
    And,
    Or,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileTimeUnaryOperator {
    CheckedIntegerNegate,
    FloatNegate,
    BooleanNot,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileTimeExpression {
    kind: CompileTimeExpressionKind,
    type_id: TypeId,
    span: SourceSpan,
}

impl CompileTimeExpression {
    #[must_use]
    pub fn constant(value: CompileTimeValue, type_id: TypeId, span: SourceSpan) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Constant(value),
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn parameter(index: u32, type_id: TypeId, span: SourceSpan) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Parameter(index),
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn local(local: LocalId, type_id: TypeId, span: SourceSpan) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Local(local),
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn let_binding(
        local: LocalId,
        local_type: TypeId,
        initializer: Self,
        body: Self,
        span: SourceSpan,
    ) -> Self {
        let type_id = body.type_id();
        Self {
            kind: CompileTimeExpressionKind::Let {
                local,
                local_type,
                initializer: Box::new(initializer),
                body: Box::new(body),
            },
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn binary(
        operator: CompileTimeBinaryOperator,
        left: Self,
        right: Self,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Binary {
                operator,
                left: Box::new(left),
                right: Box::new(right),
            },
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn unary(
        operator: CompileTimeUnaryOperator,
        operand: Self,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Unary {
                operator,
                operand: Box::new(operand),
            },
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn call(
        function: FunctionId,
        arguments: Vec<Self>,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Call {
                function,
                arguments,
            },
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn conditional(
        condition: Self,
        when_true: Self,
        when_false: Self,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Conditional {
                condition: Box::new(condition),
                when_true: Box::new(when_true),
                when_false: Box::new(when_false),
            },
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn tuple(elements: Vec<Self>, type_id: TypeId, span: SourceSpan) -> Self {
        Self {
            kind: CompileTimeExpressionKind::Tuple(elements),
            type_id,
            span,
        }
    }

    #[must_use]
    pub fn attribute_query(
        module: ModuleId,
        attribute: AttributeId,
        subject: AttributeQuerySubject,
        has_only: bool,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind: CompileTimeExpressionKind::AttributeQuery {
                module,
                attribute,
                subject,
                has_only,
            },
            type_id,
            span,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &CompileTimeExpressionKind {
        &self.kind
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompileTimeExpressionKind {
    Constant(CompileTimeValue),
    Parameter(u32),
    Local(LocalId),
    Let {
        local: LocalId,
        local_type: TypeId,
        initializer: Box<CompileTimeExpression>,
        body: Box<CompileTimeExpression>,
    },
    Unary {
        operator: CompileTimeUnaryOperator,
        operand: Box<CompileTimeExpression>,
    },
    Binary {
        operator: CompileTimeBinaryOperator,
        left: Box<CompileTimeExpression>,
        right: Box<CompileTimeExpression>,
    },
    Conditional {
        condition: Box<CompileTimeExpression>,
        when_true: Box<CompileTimeExpression>,
        when_false: Box<CompileTimeExpression>,
    },
    Tuple(Vec<CompileTimeExpression>),
    Call {
        function: FunctionId,
        arguments: Vec<CompileTimeExpression>,
    },
    AttributeQuery {
        module: ModuleId,
        attribute: AttributeId,
        subject: AttributeQuerySubject,
        has_only: bool,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompileTimeFunction {
    function: FunctionId,
    parameters: Vec<TypeId>,
    result: TypeId,
    body: CompileTimeExpression,
}

impl CompileTimeFunction {
    #[must_use]
    pub const fn new(
        function: FunctionId,
        parameters: Vec<TypeId>,
        result: TypeId,
        body: CompileTimeExpression,
    ) -> Self {
        Self {
            function,
            parameters,
            result,
            body,
        }
    }

    #[must_use]
    pub const fn function(&self) -> FunctionId {
        self.function
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }

    #[must_use]
    pub const fn result(&self) -> TypeId {
        self.result
    }

    #[must_use]
    pub const fn body(&self) -> &CompileTimeExpression {
        &self.body
    }
}

/// A typed construct intentionally outside the first restricted compile-time lowerer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnsupportedCompileTimeConstruct {
    LocalBinding,
    LocalReference,
    Closure,
    Capture,
    Loop,
    Match,
    Mutation,
    ResultlessCall,
    MethodCall,
    InterfaceDispatch,
    InterfaceConversion,
    AttributeQuery,
    IndirectCall,
    FunctionReference,
    FieldAccess,
    ArrayAccess,
    Record,
    ClassConstruction,
    RecordUpdate,
    Array,
    Table,
    UnionCase,
}

/// A deterministic failure to translate accepted typed syntax into restricted
/// compile-time HIR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompileTimeLoweringError {
    MissingCanonicalType {
        span: SourceSpan,
    },
    UnsupportedResultArity {
        found: usize,
    },
    UnsupportedReturnArity {
        found: usize,
        span: SourceSpan,
    },
    BodyDoesNotProduceSingleResult {
        span: Option<SourceSpan>,
    },
    UnsupportedConstruct {
        construct: UnsupportedCompileTimeConstruct,
        span: SourceSpan,
    },
    UnknownParameter {
        parameter: u32,
        span: SourceSpan,
    },
    TypeMismatch {
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidOperatorTypes {
        span: SourceSpan,
    },
}

/// Lowers one already-typed required-constant expression into restricted
/// compile-time HIR.
///
/// This entry point has no parameter environment. Parameter and local reads are
/// rejected, while direct calls remain explicit and are checked when the
/// resulting expressions are assembled into a [`CompileTimeProgram`].
///
/// # Errors
///
/// Returns a typed restriction or invariant error for constructs outside the
/// accepted deterministic subset.
pub fn lower_compile_time_expression(
    expression: &TypedExpression,
    types: &TypeArena,
) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
    TypedCompileTimeLowerer {
        types,
        parameter_types: &[],
    }
    .lower_expression(expression)
}

/// Lowers one accepted typed function body into restricted compile-time HIR.
///
/// The initial boundary supports exactly one result and a single deterministic
/// result expression, including nested `if` statements whose two branches each
/// produce exactly one value. State, loops, mutation, receiver dispatch, and
/// indirect calls are rejected explicitly.
///
/// # Errors
///
/// Returns a typed restriction, arity, canonical-type, or type-consistency
/// error. It never invents behavior for an unsupported body.
pub fn lower_compile_time_function(
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    types: &TypeArena,
) -> Result<CompileTimeFunction, CompileTimeLoweringError> {
    let parameter_types: Result<Vec<_>, _> = signature
        .parameters()
        .iter()
        .map(|parameter| {
            parameter.parameter_type().type_id().ok_or(
                CompileTimeLoweringError::MissingCanonicalType {
                    span: parameter.span(),
                },
            )
        })
        .collect();
    let parameter_types = parameter_types?;
    if signature.results().len() != 1 {
        return Err(CompileTimeLoweringError::UnsupportedResultArity {
            found: signature.results().len(),
        });
    }
    let result =
        signature.results()[0]
            .type_id()
            .ok_or(CompileTimeLoweringError::MissingCanonicalType {
                span: signature.results()[0].span(),
            })?;
    let lowerer = TypedCompileTimeLowerer {
        types,
        parameter_types: &parameter_types,
    };
    let body = lowerer.lower_statements(body.statements())?;
    if body.type_id() != result {
        return Err(CompileTimeLoweringError::TypeMismatch {
            expected: result,
            found: body.type_id(),
            span: body.span(),
        });
    }
    Ok(CompileTimeFunction::new(
        FunctionId::from_raw(signature.symbol().raw()),
        parameter_types,
        result,
        body,
    ))
}

struct TypedCompileTimeLowerer<'types> {
    types: &'types TypeArena,
    parameter_types: &'types [TypeId],
}

impl TypedCompileTimeLowerer<'_> {
    fn lower_statements(
        &self,
        statements: &[TypedStatement],
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let Some((first, rest)) = statements.split_first() else {
            return Err(CompileTimeLoweringError::BodyDoesNotProduceSingleResult { span: None });
        };
        if let TypedStatementKind::Local {
            local,
            local_type,
            initializer,
            ..
        } = first.kind()
        {
            let initializer = self.lower_expression(initializer)?;
            if initializer.type_id() != *local_type {
                return Err(CompileTimeLoweringError::TypeMismatch {
                    expected: *local_type,
                    found: initializer.type_id(),
                    span: initializer.span(),
                });
            }
            let body = self.lower_statements(rest)?;
            return Ok(CompileTimeExpression::let_binding(
                *local,
                *local_type,
                initializer,
                body,
                first.span(),
            ));
        }
        if rest.is_empty() {
            return self.lower_result_statement(first);
        }
        if let Some(error) = unsupported_statement_error(first) {
            return Err(error);
        }
        Err(CompileTimeLoweringError::BodyDoesNotProduceSingleResult {
            span: Some(first.span()),
        })
    }

    fn lower_result_statement(
        &self,
        statement: &TypedStatement,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        match statement.kind() {
            TypedStatementKind::Return { values } => {
                if values.len() != 1 {
                    return Err(CompileTimeLoweringError::UnsupportedReturnArity {
                        found: values.len(),
                        span: statement.span(),
                    });
                }
                self.lower_expression(&values[0])
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = self.lower_expression(condition)?;
                let when_true = self.lower_statements(then_body)?;
                let when_false = self.lower_statements(else_body)?;
                if when_true.type_id() != when_false.type_id() {
                    return Err(CompileTimeLoweringError::TypeMismatch {
                        expected: when_true.type_id(),
                        found: when_false.type_id(),
                        span: when_false.span(),
                    });
                }
                Ok(CompileTimeExpression::conditional(
                    condition,
                    when_true.clone(),
                    when_false,
                    when_true.type_id(),
                    statement.span(),
                ))
            }
            _ => Err(unsupported_statement_error(statement).unwrap_or(
                CompileTimeLoweringError::BodyDoesNotProduceSingleResult {
                    span: Some(statement.span()),
                },
            )),
        }
    }

    fn lower_expression(
        &self,
        expression: &TypedExpression,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let type_id = expression.type_id();
        let span = expression.span();
        match expression.kind() {
            TypedExpressionKind::Integer(_)
            | TypedExpressionKind::Float(_)
            | TypedExpressionKind::String(_)
            | TypedExpressionKind::Boolean(_)
            | TypedExpressionKind::Nil => Ok(lower_compile_time_literal(expression)),
            TypedExpressionKind::Parameter(parameter) => {
                self.lower_parameter(parameter.raw(), type_id, span)
            }
            TypedExpressionKind::Tuple(elements) => self.lower_tuple(elements, type_id, span),
            TypedExpressionKind::Unary { operator, operand } => {
                let lowered_operand = self.lower_expression(operand)?;
                let operator = self.lower_unary_operator(*operator, operand.type_id(), span)?;
                Ok(CompileTimeExpression::unary(
                    operator,
                    lowered_operand,
                    type_id,
                    span,
                ))
            }
            TypedExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                let operator = self.lower_binary_operator(*operator, left.type_id(), span)?;
                Ok(CompileTimeExpression::binary(
                    operator,
                    self.lower_expression(left)?,
                    self.lower_expression(right)?,
                    type_id,
                    span,
                ))
            }
            TypedExpressionKind::DirectCall {
                function,
                arguments,
            } => Ok(CompileTimeExpression::call(
                FunctionId::from_raw(function.raw()),
                arguments
                    .iter()
                    .map(|argument| self.lower_expression(argument))
                    .collect::<Result<Vec<_>, _>>()?,
                type_id,
                span,
            )),
            TypedExpressionKind::Local(local) => {
                Ok(CompileTimeExpression::local(*local, type_id, span))
            }
            TypedExpressionKind::AttributeQuery {
                module,
                attribute,
                subject,
            } => Ok(CompileTimeExpression::attribute_query(
                *module, *attribute, *subject, false, type_id, span,
            )),
            TypedExpressionKind::HasAttributeQuery {
                module,
                attribute,
                subject,
            } => Ok(CompileTimeExpression::attribute_query(
                *module, *attribute, *subject, true, type_id, span,
            )),
            unsupported => Err(unsupported_expression(
                expression,
                unsupported_compile_time_construct(unsupported),
            )),
        }
    }

    fn lower_parameter(
        &self,
        parameter: u32,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let Some(expected) = usize::try_from(parameter)
            .ok()
            .and_then(|index| self.parameter_types.get(index))
            .copied()
        else {
            return Err(CompileTimeLoweringError::UnknownParameter { parameter, span });
        };
        if expected != type_id {
            return Err(CompileTimeLoweringError::TypeMismatch {
                expected,
                found: type_id,
                span,
            });
        }
        Ok(CompileTimeExpression::parameter(parameter, type_id, span))
    }

    fn lower_tuple(
        &self,
        elements: &[TypedExpression],
        type_id: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeExpression, CompileTimeLoweringError> {
        let elements = elements
            .iter()
            .map(|element| self.lower_expression(element))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(CompileTimeExpression::tuple(elements, type_id, span))
    }

    fn lower_unary_operator(
        &self,
        operator: TypedUnaryOperator,
        operand: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeUnaryOperator, CompileTimeLoweringError> {
        match operator {
            TypedUnaryOperator::Not => Ok(CompileTimeUnaryOperator::BooleanNot),
            TypedUnaryOperator::Negate if is_integer_type(self.types, operand) => {
                Ok(CompileTimeUnaryOperator::CheckedIntegerNegate)
            }
            TypedUnaryOperator::Negate if is_float_type(self.types, operand) => {
                Ok(CompileTimeUnaryOperator::FloatNegate)
            }
            TypedUnaryOperator::Negate => {
                Err(CompileTimeLoweringError::InvalidOperatorTypes { span })
            }
        }
    }

    fn lower_binary_operator(
        &self,
        operator: TypedBinaryOperator,
        operand: TypeId,
        span: SourceSpan,
    ) -> Result<CompileTimeBinaryOperator, CompileTimeLoweringError> {
        let integer = is_integer_type(self.types, operand);
        let float = is_float_type(self.types, operand);
        match operator {
            TypedBinaryOperator::Add if integer => Ok(CompileTimeBinaryOperator::CheckedAdd),
            TypedBinaryOperator::Subtract if integer => {
                Ok(CompileTimeBinaryOperator::CheckedSubtract)
            }
            TypedBinaryOperator::Multiply if integer => {
                Ok(CompileTimeBinaryOperator::CheckedMultiply)
            }
            TypedBinaryOperator::Divide if integer => Ok(CompileTimeBinaryOperator::CheckedDivide),
            TypedBinaryOperator::Remainder if integer => {
                Ok(CompileTimeBinaryOperator::CheckedRemainder)
            }
            TypedBinaryOperator::Add if float => Ok(CompileTimeBinaryOperator::FloatAdd),
            TypedBinaryOperator::Subtract if float => Ok(CompileTimeBinaryOperator::FloatSubtract),
            TypedBinaryOperator::Multiply if float => Ok(CompileTimeBinaryOperator::FloatMultiply),
            TypedBinaryOperator::Divide if float => Ok(CompileTimeBinaryOperator::FloatDivide),
            TypedBinaryOperator::Equal => Ok(CompileTimeBinaryOperator::Equal),
            TypedBinaryOperator::NotEqual => Ok(CompileTimeBinaryOperator::NotEqual),
            TypedBinaryOperator::LessThan => Ok(CompileTimeBinaryOperator::LessThan),
            TypedBinaryOperator::GreaterThan => Ok(CompileTimeBinaryOperator::GreaterThan),
            TypedBinaryOperator::And => Ok(CompileTimeBinaryOperator::And),
            TypedBinaryOperator::Or => Ok(CompileTimeBinaryOperator::Or),
            TypedBinaryOperator::Add
            | TypedBinaryOperator::Subtract
            | TypedBinaryOperator::Multiply
            | TypedBinaryOperator::Divide
            | TypedBinaryOperator::Remainder => {
                Err(CompileTimeLoweringError::InvalidOperatorTypes { span })
            }
        }
    }
}

fn lower_compile_time_literal(expression: &TypedExpression) -> CompileTimeExpression {
    let value = match expression.kind() {
        TypedExpressionKind::Integer(value) => CompileTimeValue::Integer(*value),
        TypedExpressionKind::Float(value) => CompileTimeValue::Float(*value),
        TypedExpressionKind::String(value) => {
            CompileTimeValue::String(unquote_source_string(value))
        }
        TypedExpressionKind::Boolean(value) => CompileTimeValue::Boolean(*value),
        TypedExpressionKind::Nil => CompileTimeValue::Nil,
        _ => unreachable!("literal lowering receives only a typed literal"),
    };
    CompileTimeExpression::constant(value, expression.type_id(), expression.span())
}

fn unsupported_statement_error(statement: &TypedStatement) -> Option<CompileTimeLoweringError> {
    let construct = match statement.kind() {
        TypedStatementKind::While { .. } => UnsupportedCompileTimeConstruct::Loop,
        TypedStatementKind::LocalSet { .. }
        | TypedStatementKind::ParameterSet { .. }
        | TypedStatementKind::CaptureSet { .. }
        | TypedStatementKind::FieldSet { .. } => UnsupportedCompileTimeConstruct::Mutation,
        TypedStatementKind::Match { .. } => UnsupportedCompileTimeConstruct::Match,
        TypedStatementKind::Call(call) => match call.dispatch() {
            TypedCallDispatch::Standard { .. } | TypedCallDispatch::Direct { .. } => {
                UnsupportedCompileTimeConstruct::ResultlessCall
            }
            TypedCallDispatch::DirectMethod { .. } => UnsupportedCompileTimeConstruct::MethodCall,
            TypedCallDispatch::InterfaceMethod { .. } => {
                UnsupportedCompileTimeConstruct::InterfaceDispatch
            }
            TypedCallDispatch::Indirect { .. } => UnsupportedCompileTimeConstruct::IndirectCall,
        },
        TypedStatementKind::Local { .. }
        | TypedStatementKind::Return { .. }
        | TypedStatementKind::If { .. }
        | TypedStatementKind::Expression(_) => return None,
    };
    Some(CompileTimeLoweringError::UnsupportedConstruct {
        construct,
        span: statement.span(),
    })
}

const fn unsupported_expression(
    expression: &TypedExpression,
    construct: UnsupportedCompileTimeConstruct,
) -> CompileTimeLoweringError {
    CompileTimeLoweringError::UnsupportedConstruct {
        construct,
        span: expression.span(),
    }
}

fn unsupported_compile_time_construct(
    expression: &TypedExpressionKind,
) -> UnsupportedCompileTimeConstruct {
    match expression {
        TypedExpressionKind::Closure(_) => UnsupportedCompileTimeConstruct::Closure,
        TypedExpressionKind::Capture(_) => UnsupportedCompileTimeConstruct::Capture,
        TypedExpressionKind::Function(_) => UnsupportedCompileTimeConstruct::FunctionReference,
        TypedExpressionKind::Field { .. } => UnsupportedCompileTimeConstruct::FieldAccess,
        TypedExpressionKind::ArrayGet { .. } => UnsupportedCompileTimeConstruct::ArrayAccess,
        TypedExpressionKind::Record { .. } => UnsupportedCompileTimeConstruct::Record,
        TypedExpressionKind::ClassConstruct { .. } => {
            UnsupportedCompileTimeConstruct::ClassConstruction
        }
        TypedExpressionKind::RecordUpdate { .. } => UnsupportedCompileTimeConstruct::RecordUpdate,
        TypedExpressionKind::Array(_) => UnsupportedCompileTimeConstruct::Array,
        TypedExpressionKind::Table(_) => UnsupportedCompileTimeConstruct::Table,
        TypedExpressionKind::UnionCase { .. } => UnsupportedCompileTimeConstruct::UnionCase,
        TypedExpressionKind::DirectMethodCall { .. } => UnsupportedCompileTimeConstruct::MethodCall,
        TypedExpressionKind::InterfaceMethodCall { .. } => {
            UnsupportedCompileTimeConstruct::InterfaceDispatch
        }
        TypedExpressionKind::InterfaceUpcast { .. } => {
            UnsupportedCompileTimeConstruct::InterfaceConversion
        }
        TypedExpressionKind::IndirectCall { .. } => UnsupportedCompileTimeConstruct::IndirectCall,
        TypedExpressionKind::StandardCall { .. } => UnsupportedCompileTimeConstruct::ResultlessCall,
        TypedExpressionKind::Integer(_)
        | TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. }
        | TypedExpressionKind::Float(_)
        | TypedExpressionKind::String(_)
        | TypedExpressionKind::Boolean(_)
        | TypedExpressionKind::Nil
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::Parameter(_)
        | TypedExpressionKind::Tuple(_)
        | TypedExpressionKind::Unary { .. }
        | TypedExpressionKind::Binary { .. }
        | TypedExpressionKind::DirectCall { .. } => {
            unreachable!("supported expression is not routed to the unsupported lowerer")
        }
    }
}

fn unquote_source_string(value: &str) -> String {
    value
        .get(1..value.len().saturating_sub(1))
        .unwrap_or_default()
        .to_owned()
}

#[derive(Clone, Debug)]
pub struct CompileTimeProgram {
    functions: Vec<CompileTimeFunction>,
    types: TypeArena,
    metadata: CompileTimeTypeMetadata,
    attribute_queries: Option<AttributeQueryIndex>,
}

impl CompileTimeProgram {
    /// Builds a deterministic compile-time program.
    ///
    /// # Errors
    ///
    /// Rejects duplicate functions, unknown calls, and statically inconsistent
    /// parameter, branch, call-result, and function-result types.
    pub fn new(
        functions: Vec<CompileTimeFunction>,
        types: &TypeArena,
    ) -> Result<Self, ProgramError> {
        Self::new_with_metadata(functions, types, CompileTimeTypeMetadata::new())
    }

    /// Builds a deterministic compile-time program with compiler-owned handle
    /// and aggregate schemas.
    ///
    /// # Errors
    ///
    /// Applies the same verification as [`Self::new`] and rejects every value
    /// that does not exactly match the supplied compile-time-only metadata.
    pub fn new_with_metadata(
        mut functions: Vec<CompileTimeFunction>,
        types: &TypeArena,
        metadata: CompileTimeTypeMetadata,
    ) -> Result<Self, ProgramError> {
        functions.sort_by_key(CompileTimeFunction::function);
        if let Some(pair) = functions
            .windows(2)
            .find(|pair| pair[0].function() == pair[1].function())
        {
            return Err(ProgramError::DuplicateFunction(pair[0].function()));
        }
        let signatures: BTreeMap<_, _> = functions
            .iter()
            .map(|function| {
                (
                    function.function(),
                    (function.parameters().to_vec(), function.result()),
                )
            })
            .collect();
        for function in &functions {
            for type_id in function
                .parameters()
                .iter()
                .copied()
                .chain(std::iter::once(function.result()))
            {
                if !types.is_valid_compile_time_type(type_id) {
                    return Err(ProgramError::InvalidType(type_id));
                }
            }
            ExpressionVerifier {
                parameters: function.parameters(),
                signatures: &signatures,
                types,
                metadata: &metadata,
            }
            .verify(function.body(), &BTreeMap::new())?;
            if function.body().type_id() != function.result() {
                return Err(ProgramError::TypeMismatch {
                    expected: function.result(),
                    found: function.body().type_id(),
                });
            }
        }
        Ok(Self {
            functions,
            types: types.clone(),
            metadata,
            attribute_queries: None,
        })
    }

    #[must_use]
    pub fn with_attribute_queries(mut self, queries: AttributeQueryIndex) -> Self {
        self.attribute_queries = Some(queries);
        self
    }

    #[must_use]
    pub fn functions(&self) -> &[CompileTimeFunction] {
        &self.functions
    }

    #[must_use]
    pub const fn types(&self) -> &TypeArena {
        &self.types
    }

    #[must_use]
    pub const fn metadata(&self) -> &CompileTimeTypeMetadata {
        &self.metadata
    }

    fn function(&self, id: FunctionId) -> Option<&CompileTimeFunction> {
        self.functions
            .binary_search_by_key(&id, CompileTimeFunction::function)
            .ok()
            .map(|index| &self.functions[index])
    }

    const fn attribute_queries(&self) -> Option<&AttributeQueryIndex> {
        self.attribute_queries.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramError {
    DuplicateFunction(FunctionId),
    InvalidType(TypeId),
    UnknownFunction(FunctionId),
    UnknownParameter(u32),
    UnknownLocal(LocalId),
    DuplicateLocal(LocalId),
    WrongArity {
        function: FunctionId,
        expected: usize,
        found: usize,
    },
    TypeMismatch {
        expected: TypeId,
        found: TypeId,
    },
    ValueTypeMismatch {
        expected: TypeId,
    },
    InvalidUnaryOperator {
        operator: CompileTimeUnaryOperator,
        operand: TypeId,
        result: TypeId,
    },
    InvalidBinaryOperator {
        operator: CompileTimeBinaryOperator,
        left: TypeId,
        right: TypeId,
        result: TypeId,
    },
}

struct ExpressionVerifier<'program> {
    parameters: &'program [TypeId],
    signatures: &'program BTreeMap<FunctionId, (Vec<TypeId>, TypeId)>,
    types: &'program TypeArena,
    metadata: &'program CompileTimeTypeMetadata,
}

impl ExpressionVerifier<'_> {
    fn verify(
        &self,
        expression: &CompileTimeExpression,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        if !self.types.is_valid_hir_type(expression.type_id()) {
            return Err(ProgramError::InvalidType(expression.type_id()));
        }
        match expression.kind() {
            CompileTimeExpressionKind::Constant(value) => {
                if !value_matches_type(value, expression.type_id(), self.types, self.metadata) {
                    return Err(ProgramError::ValueTypeMismatch {
                        expected: expression.type_id(),
                    });
                }
            }
            CompileTimeExpressionKind::Parameter(index) => {
                let Some(parameter_type) = usize::try_from(*index)
                    .ok()
                    .and_then(|index| self.parameters.get(index))
                else {
                    return Err(ProgramError::UnknownParameter(*index));
                };
                require_type(*parameter_type, expression.type_id())?;
            }
            CompileTimeExpressionKind::Local(local) => {
                let Some(local_type) = locals.get(local) else {
                    return Err(ProgramError::UnknownLocal(*local));
                };
                require_type(*local_type, expression.type_id())?;
            }
            CompileTimeExpressionKind::Let {
                local,
                local_type,
                initializer,
                body,
            } => self.verify_let(expression, *local, *local_type, initializer, body, locals)?,
            CompileTimeExpressionKind::Unary { operator, operand } => {
                self.verify(operand, locals)?;
                if !valid_unary_operator(
                    *operator,
                    operand.type_id(),
                    expression.type_id(),
                    self.types,
                ) {
                    return Err(ProgramError::InvalidUnaryOperator {
                        operator: *operator,
                        operand: operand.type_id(),
                        result: expression.type_id(),
                    });
                }
            }
            CompileTimeExpressionKind::Binary {
                operator,
                left,
                right,
            } => self.verify_binary(*operator, left, right, expression.type_id(), locals)?,
            CompileTimeExpressionKind::Conditional {
                condition,
                when_true,
                when_false,
            } => {
                self.verify(condition, locals)?;
                self.verify(when_true, locals)?;
                self.verify(when_false, locals)?;
                require_type(boolean_type(self.types)?, condition.type_id())?;
                require_type(when_true.type_id(), when_false.type_id())?;
                require_type(expression.type_id(), when_true.type_id())?;
            }
            CompileTimeExpressionKind::Tuple(elements) => {
                self.verify_tuple(expression.type_id(), elements, locals)?;
            }
            CompileTimeExpressionKind::Call {
                function,
                arguments,
            } => self.verify_call(*function, arguments, expression.type_id(), locals)?,
            CompileTimeExpressionKind::AttributeQuery {
                attribute,
                has_only,
                ..
            } => {
                if *has_only {
                    require_type(boolean_type(self.types)?, expression.type_id())?;
                } else if !type_contains_attribute(self.types, expression.type_id(), *attribute) {
                    return Err(ProgramError::ValueTypeMismatch {
                        expected: expression.type_id(),
                    });
                }
            }
        }
        Ok(())
    }

    fn verify_let(
        &self,
        expression: &CompileTimeExpression,
        local: LocalId,
        local_type: TypeId,
        initializer: &CompileTimeExpression,
        body: &CompileTimeExpression,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        self.verify(initializer, locals)?;
        require_type(local_type, initializer.type_id())?;
        if locals.contains_key(&local) {
            return Err(ProgramError::DuplicateLocal(local));
        }
        let mut body_locals = locals.clone();
        body_locals.insert(local, local_type);
        self.verify(body, &body_locals)?;
        require_type(expression.type_id(), body.type_id())
    }

    fn verify_binary(
        &self,
        operator: CompileTimeBinaryOperator,
        left: &CompileTimeExpression,
        right: &CompileTimeExpression,
        result: TypeId,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        self.verify(left, locals)?;
        self.verify(right, locals)?;
        if valid_binary_operator(
            operator,
            left.type_id(),
            right.type_id(),
            result,
            self.types,
        ) {
            Ok(())
        } else {
            Err(ProgramError::InvalidBinaryOperator {
                operator,
                left: left.type_id(),
                right: right.type_id(),
                result,
            })
        }
    }

    fn verify_tuple(
        &self,
        result: TypeId,
        elements: &[CompileTimeExpression],
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        let Some(SemanticType::Tuple(element_types)) = self.types.get(result) else {
            return Err(ProgramError::ValueTypeMismatch { expected: result });
        };
        if element_types.len() != elements.len() {
            return Err(ProgramError::ValueTypeMismatch { expected: result });
        }
        for (element, expected) in elements.iter().zip(element_types) {
            self.verify(element, locals)?;
            require_type(*expected, element.type_id())?;
        }
        Ok(())
    }

    fn verify_call(
        &self,
        function: FunctionId,
        arguments: &[CompileTimeExpression],
        result_type: TypeId,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        let Some((parameter_types, result)) = self.signatures.get(&function) else {
            return Err(ProgramError::UnknownFunction(function));
        };
        if parameter_types.len() != arguments.len() {
            return Err(ProgramError::WrongArity {
                function,
                expected: parameter_types.len(),
                found: arguments.len(),
            });
        }
        for (argument, parameter_type) in arguments.iter().zip(parameter_types) {
            self.verify(argument, locals)?;
            require_type(*parameter_type, argument.type_id())?;
        }
        require_type(*result, result_type)
    }
}

fn value_matches_type(
    value: &CompileTimeValue,
    type_id: TypeId,
    types: &TypeArena,
    metadata: &CompileTimeTypeMetadata,
) -> bool {
    match (value, types.get(type_id)) {
        (
            CompileTimeValue::Nil,
            Some(SemanticType::Primitive(PrimitiveType::Nil) | SemanticType::Optional(_)),
        )
        | (CompileTimeValue::Boolean(_), Some(SemanticType::Primitive(PrimitiveType::Boolean)))
        | (CompileTimeValue::String(_), Some(SemanticType::Primitive(PrimitiveType::String))) => {
            true
        }
        (
            CompileTimeValue::Integer(value),
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))),
        ) => value.kind() == *kind,
        (CompileTimeValue::Float(value), Some(SemanticType::Primitive(PrimitiveType::Float32))) => {
            value.kind() == FloatKind::Float32
        }
        (CompileTimeValue::Float(value), Some(SemanticType::Primitive(PrimitiveType::Float64))) => {
            value.kind() == FloatKind::Float64
        }
        (CompileTimeValue::Tuple(values), Some(SemanticType::Tuple(element_types))) => {
            values.len() == element_types.len()
                && values
                    .iter()
                    .zip(element_types)
                    .all(|(value, type_id)| value_matches_type(value, *type_id, types, metadata))
        }
        (CompileTimeValue::Array(values), Some(SemanticType::Array(element_type))) => values
            .iter()
            .all(|value| value_matches_type(value, *element_type, types, metadata)),
        (
            CompileTimeValue::Attribute {
                attribute: value_attribute,
                arguments,
            },
            Some(SemanticType::Attribute {
                attribute,
                parameters,
            }),
        ) => {
            value_attribute == attribute
                && arguments.len() == parameters.len()
                && arguments.iter().zip(parameters).all(|(value, parameter)| {
                    value_matches_type(value, *parameter, types, metadata)
                })
        }
        (CompileTimeValue::Record(values), Some(SemanticType::Record(semantic_fields))) => {
            let Some(fields) = metadata.record(type_id) else {
                return false;
            };
            if values.len() != fields.len()
                || fields.windows(2).any(|pair| pair[0].0 == pair[1].0)
                || !record_metadata_matches_semantic_type(fields, semantic_fields)
            {
                return false;
            }
            values
                .iter()
                .zip(fields)
                .all(|((value_field, value), (field, field_type))| {
                    value_field == field && value_matches_type(value, *field_type, types, metadata)
                })
        }
        (
            CompileTimeValue::Union {
                union,
                case,
                arguments,
            },
            Some(SemanticType::TaggedUnion { definition }),
        ) => {
            let Some(argument_types) = metadata.union_case(*union, *case) else {
                return false;
            };
            union == definition
                && arguments.len() == argument_types.len()
                && arguments
                    .iter()
                    .zip(argument_types)
                    .all(|(argument, argument_type)| {
                        value_matches_type(argument, *argument_type, types, metadata)
                    })
        }
        (CompileTimeValue::TypeReference(referenced), Some(SemanticType::Opaque(_))) => {
            metadata.handle_kind(type_id) == Some(CompileTimeHandleKind::Type)
                && types.get(*referenced).is_some()
                && metadata.contains_type(*referenced)
        }
        (CompileTimeValue::SymbolReference(symbol), Some(SemanticType::Opaque(_))) => {
            metadata.handle_kind(type_id) == Some(CompileTimeHandleKind::Symbol)
                && metadata.contains_symbol(*symbol)
        }
        (value, Some(SemanticType::Optional(inner))) => {
            value_matches_type(value, *inner, types, metadata)
        }
        (_, Some(SemanticType::Union(members))) => members
            .iter()
            .any(|member| value_matches_type(value, *member, types, metadata)),
        _ => false,
    }
}

fn type_contains_attribute(types: &TypeArena, type_id: TypeId, attribute: AttributeId) -> bool {
    match types.get(type_id) {
        Some(SemanticType::Attribute {
            attribute: found, ..
        }) => *found == attribute,
        Some(SemanticType::Array(element) | SemanticType::Optional(element)) => {
            type_contains_attribute(types, *element, attribute)
        }
        Some(SemanticType::Union(members)) => members
            .iter()
            .any(|member| type_contains_attribute(types, *member, attribute)),
        _ => false,
    }
}

fn resolved_attribute_value(attribute: &ResolvedAttribute) -> CompileTimeValue {
    CompileTimeValue::Attribute {
        attribute: attribute.attribute(),
        arguments: attribute
            .arguments()
            .iter()
            .map(|argument| attribute_constant_value(argument.value()))
            .collect(),
    }
}

fn attribute_constant_value(value: &AttributeConstant) -> CompileTimeValue {
    match value {
        AttributeConstant::Nil => CompileTimeValue::Nil,
        AttributeConstant::Boolean(value) => CompileTimeValue::Boolean(*value),
        AttributeConstant::Integer(value) => CompileTimeValue::Integer(*value),
        AttributeConstant::Float(value) => CompileTimeValue::Float(*value),
        AttributeConstant::String(value) => CompileTimeValue::String(value.clone()),
        AttributeConstant::Tuple(values) => {
            CompileTimeValue::Tuple(values.iter().map(attribute_constant_value).collect())
        }
    }
}

fn record_metadata_matches_semantic_type(
    fields: &[(FieldId, TypeId)],
    semantic_fields: &[(String, TypeId)],
) -> bool {
    if fields.len() != semantic_fields.len() {
        return false;
    }
    let mut metadata_types: Vec<_> = fields.iter().map(|(_, type_id)| *type_id).collect();
    let mut declared_types: Vec<_> = semantic_fields
        .iter()
        .map(|(_, type_id)| *type_id)
        .collect();
    metadata_types.sort_unstable();
    declared_types.sort_unstable();
    metadata_types == declared_types
}

fn valid_unary_operator(
    operator: CompileTimeUnaryOperator,
    operand: TypeId,
    result: TypeId,
    types: &TypeArena,
) -> bool {
    operand == result
        && match operator {
            CompileTimeUnaryOperator::CheckedIntegerNegate => {
                matches!(
                    types.get(operand),
                    Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) if kind.is_signed()
                )
            }
            CompileTimeUnaryOperator::FloatNegate => is_float_type(types, operand),
            CompileTimeUnaryOperator::BooleanNot => boolean_type(types) == Ok(operand),
        }
}

fn valid_binary_operator(
    operator: CompileTimeBinaryOperator,
    left: TypeId,
    right: TypeId,
    result: TypeId,
    types: &TypeArena,
) -> bool {
    if left != right {
        return false;
    }
    match operator {
        CompileTimeBinaryOperator::CheckedAdd
        | CompileTimeBinaryOperator::CheckedSubtract
        | CompileTimeBinaryOperator::CheckedMultiply
        | CompileTimeBinaryOperator::CheckedDivide
        | CompileTimeBinaryOperator::CheckedRemainder => {
            left == result && is_integer_type(types, left)
        }
        CompileTimeBinaryOperator::FloatAdd
        | CompileTimeBinaryOperator::FloatSubtract
        | CompileTimeBinaryOperator::FloatMultiply
        | CompileTimeBinaryOperator::FloatDivide => left == result && is_float_type(types, left),
        CompileTimeBinaryOperator::LessThan | CompileTimeBinaryOperator::GreaterThan => {
            (is_integer_type(types, left) || is_float_type(types, left))
                && boolean_type(types) == Ok(result)
        }
        CompileTimeBinaryOperator::Equal | CompileTimeBinaryOperator::NotEqual => {
            boolean_type(types) == Ok(result) && supports_compile_time_equality(types, left)
        }
        CompileTimeBinaryOperator::And | CompileTimeBinaryOperator::Or => {
            boolean_type(types) == Ok(left) && left == result
        }
    }
}

fn supports_compile_time_equality(types: &TypeArena, type_id: TypeId) -> bool {
    match types.get(type_id) {
        Some(SemanticType::Primitive(
            PrimitiveType::Nil
            | PrimitiveType::Boolean
            | PrimitiveType::Integer(_)
            | PrimitiveType::String,
        )) => true,
        Some(SemanticType::Tuple(elements) | SemanticType::Union(elements)) => elements
            .iter()
            .all(|element| supports_compile_time_equality(types, *element)),
        Some(SemanticType::Record(fields)) => fields
            .iter()
            .all(|(_, field_type)| supports_compile_time_equality(types, *field_type)),
        _ => false,
    }
}

fn is_integer_type(types: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        types.get(type_id),
        Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
    )
}

fn is_float_type(types: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        types.get(type_id),
        Some(SemanticType::Primitive(
            PrimitiveType::Float32 | PrimitiveType::Float64
        ))
    )
}

fn boolean_type(types: &TypeArena) -> Result<TypeId, ProgramError> {
    types
        .source_type("Boolean")
        .ok_or(ProgramError::InvalidType(TypeId::from_raw(u32::MAX)))
}

fn require_type(expected: TypeId, found: TypeId) -> Result<(), ProgramError> {
    if expected == found {
        Ok(())
    } else {
        Err(ProgramError::TypeMismatch { expected, found })
    }
}

/// Deterministic resource envelope for one compile-time evaluation.
///
/// The generic query budget owns fuel, cumulative allocation, and call depth.
/// Compile-time evaluation additionally bounds the recursive live-value shape,
/// published output, and structured diagnostics as required by ADR 0023.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompileTimeBudget {
    query: QueryBudget,
    maximum_live_values: u64,
    maximum_output_bytes: u64,
    maximum_diagnostics: u64,
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
    function: FunctionId,
    arguments: Vec<CompileTimeValue>,
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
    value: CompileTimeValue,
    evaluation_key: CompileTimeEvaluationKey,
    origin: SourceSpan,
    function_dependencies: Vec<FunctionId>,
    dependencies: Vec<CompileTimeDependency>,
    budget: CompileTimeBudget,
    usage: EvaluationUsage,
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
    instructions: u64,
    allocated_bytes: u64,
    maximum_call_depth: u32,
    maximum_live_values: u64,
    output_bytes: u64,
    diagnostics: u64,
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
    function: FunctionId,
    call_site: SourceSpan,
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
    kind: EvaluationFailureKind,
    location: SourceSpan,
    context: Box<EvaluationFailureContext>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct EvaluationFailureContext {
    evaluation_key: CompileTimeEvaluationKey,
    origin: SourceSpan,
    call_chain: Vec<CompileTimeCallFrame>,
    dependencies: Vec<CompileTimeDependency>,
    budget: CompileTimeBudget,
    usage: EvaluationUsage,
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

    const fn legacy_error(&self) -> EvaluationError {
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

#[derive(Clone, Debug, Eq, PartialEq)]
struct ActiveCall {
    function: FunctionId,
    arguments: Vec<CompileTimeValue>,
    frame: CompileTimeCallFrame,
}

pub struct CompileTimeInterpreter<'program> {
    program: &'program CompileTimeProgram,
    eligible: &'program BTreeSet<FunctionId>,
    budget: CompileTimeBudget,
    tracker: BudgetTracker,
    dependencies: BTreeSet<CompileTimeDependency>,
    active_calls: Vec<ActiveCall>,
    evaluation_key: Option<CompileTimeEvaluationKey>,
    origin: SourceSpan,
    maximum_live_values: u64,
    temporary_live_values: u64,
    output_bytes: u64,
    diagnostics: u64,
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
            SemanticType::TaggedUnion { definition } => {
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
        CompileTimeBinaryOperator::LessThan | CompileTimeBinaryOperator::GreaterThan => {
            evaluate_ordering(operator, left, right)
        }
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
                CompileTimeBinaryOperator::GreaterThan,
                Some(Ordering::Greater)
            )
    );
    Ok(CompileTimeValue::Boolean(value))
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
