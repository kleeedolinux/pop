//! Compile-time values, typed IR, and evaluation implementation.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{
    AttributeId, FieldId, FunctionId, LocalId, ModuleId, SourceSpan, SymbolId, TypeId, UnionCaseId,
};
use pop_types::{AttributeQuerySubject, FloatValue, IntegerValue, NumericConversionKind};

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
    pub(crate) handle_types: BTreeMap<TypeId, CompileTimeHandleKind>,
    pub(crate) types: BTreeSet<TypeId>,
    pub(crate) symbols: BTreeSet<SymbolId>,
    pub(crate) records: BTreeMap<TypeId, Vec<(FieldId, TypeId)>>,
    pub(crate) union_cases: BTreeMap<(SymbolId, UnionCaseId), Vec<TypeId>>,
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

    pub(crate) fn handle_kind(&self, type_id: TypeId) -> Option<CompileTimeHandleKind> {
        self.handle_types.get(&type_id).copied()
    }

    pub(crate) fn contains_symbol(&self, symbol: SymbolId) -> bool {
        self.symbols.contains(&symbol)
    }

    pub(crate) fn contains_type(&self, type_id: TypeId) -> bool {
        self.types.contains(&type_id)
    }

    pub(crate) fn record(&self, type_id: TypeId) -> Option<&[(FieldId, TypeId)]> {
        self.records.get(&type_id).map(Vec::as_slice)
    }

    pub(crate) fn union_case(&self, union: SymbolId, case: UnionCaseId) -> Option<&[TypeId]> {
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

pub const COMPILE_TIME_IR_VERSION: u32 = 2;

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
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
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
    pub(crate) kind: CompileTimeExpressionKind,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
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
    pub fn let_tuple(
        locals: Vec<(LocalId, TypeId)>,
        initializer: Self,
        body: Self,
        span: SourceSpan,
    ) -> Self {
        let type_id = body.type_id();
        Self {
            kind: CompileTimeExpressionKind::LetTuple {
                locals,
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
    pub fn numeric_convert(
        conversion: NumericConversionKind,
        value: Self,
        type_id: TypeId,
        span: SourceSpan,
    ) -> Self {
        Self {
            kind: CompileTimeExpressionKind::NumericConvert {
                conversion,
                value: Box::new(value),
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
    pub fn tuple_get(tuple: Self, index: u32, type_id: TypeId, span: SourceSpan) -> Self {
        Self {
            kind: CompileTimeExpressionKind::TupleGet {
                tuple: Box::new(tuple),
                index,
            },
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
    LetTuple {
        locals: Vec<(LocalId, TypeId)>,
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
    NumericConvert {
        conversion: NumericConversionKind,
        value: Box<CompileTimeExpression>,
    },
    Conditional {
        condition: Box<CompileTimeExpression>,
        when_true: Box<CompileTimeExpression>,
        when_false: Box<CompileTimeExpression>,
    },
    Tuple(Vec<CompileTimeExpression>),
    TupleGet {
        tuple: Box<CompileTimeExpression>,
        index: u32,
    },
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
    pub(crate) function: FunctionId,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) result: TypeId,
    pub(crate) body: CompileTimeExpression,
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
    ReferencedCall,
    FieldAccess,
    ArrayAccess,
    Record,
    ClassConstruction,
    RecordUpdate,
    Array,
    Table,
    UnionCase,
    StringComposition,
    OptionalFlow,
    TypedFailure,
    Suspension,
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
