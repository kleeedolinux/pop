//! Fully typed body model published by body checking and consumed by HIR.
//!
//! This module contains data and read-only accessors only. Constraint solving,
//! name lookup, capture analysis, and diagnostics remain in focused checker
//! modules so downstream phases can depend on a stable typed contract.

use pop_foundation::{
    AttributeId, BindingId, CaptureId, ClassId, Diagnostic, EnumCaseId, FieldId, InterfaceId,
    InterfaceMethodId, LocalId, MethodId, ModuleId, NestedFunctionId, SourceSpan,
    StandardFunctionId, SymbolId, SymbolIdentity, TypeId, UnionCaseId, ValueParameterId,
};

use crate::{
    AttributeQuerySubject, FloatKind, FloatValue, IntegerKind, IntegerValue, NumericConversionKind,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedBody {
    pub(crate) statements: Vec<TypedStatement>,
}

impl TypedBody {
    #[must_use]
    pub fn statements(&self) -> &[TypedStatement] {
        &self.statements
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedStatement {
    pub(crate) kind: TypedStatementKind,
    pub(crate) span: SourceSpan,
}

impl TypedStatement {
    #[must_use]
    pub const fn kind(&self) -> &TypedStatementKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedStatementKind {
    Local {
        binding: BindingId,
        local: LocalId,
        name: String,
        local_type: TypeId,
        initializer: TypedExpression,
    },
    MultipleLocal {
        bindings: Vec<TypedLocalBinding>,
        value: TypedExpression,
    },
    LocalSet {
        local: LocalId,
        value: TypedExpression,
    },
    ParameterSet {
        parameter: ValueParameterId,
        value: TypedExpression,
    },
    CaptureSet {
        capture: CaptureId,
        value: TypedExpression,
    },
    Return {
        values: Vec<TypedExpression>,
    },
    If {
        condition: TypedExpression,
        then_body: Vec<TypedStatement>,
        else_body: Vec<TypedStatement>,
    },
    While {
        condition: TypedExpression,
        body: Vec<TypedStatement>,
    },
    RepeatUntil {
        body: Vec<TypedStatement>,
        condition: TypedExpression,
    },
    NumericFor {
        binding: BindingId,
        local: LocalId,
        name: String,
        integer_type: TypeId,
        first: TypedExpression,
        last: TypedExpression,
        step: TypedExpression,
        body: Vec<TypedStatement>,
    },
    Break,
    Continue,
    Match {
        scrutinee: TypedExpression,
        union: SymbolId,
        arms: Vec<TypedMatchArm>,
    },
    FieldSet {
        base: TypedExpression,
        field: FieldId,
        value: TypedExpression,
    },
    CompoundFieldSet {
        base: TypedExpression,
        field: FieldId,
        value_type: TypeId,
        operator: TypedCompoundOperator,
        value: TypedExpression,
    },
    ArraySet {
        array: TypedExpression,
        index: TypedExpression,
        value: TypedExpression,
    },
    TableSet {
        table: TypedExpression,
        key: TypedExpression,
        value: TypedExpression,
    },
    CompoundArraySet {
        array: TypedExpression,
        index: TypedExpression,
        element_type: TypeId,
        operator: TypedCompoundOperator,
        value: TypedExpression,
    },
    MultipleAssignment {
        targets: Vec<TypedAssignmentTarget>,
        value: TypedExpression,
    },
    Call(TypedCall),
    Expression(TypedExpression),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedLocalBinding {
    pub(crate) binding: BindingId,
    pub(crate) local: LocalId,
    pub(crate) name: String,
    pub(crate) local_type: TypeId,
    pub(crate) span: SourceSpan,
}

impl TypedLocalBinding {
    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    #[must_use]
    pub const fn local(&self) -> LocalId {
        self.local
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn local_type(&self) -> TypeId {
        self.local_type
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedAssignmentTarget {
    Local {
        binding: BindingId,
        local: LocalId,
        value_type: TypeId,
    },
    Capture {
        binding: BindingId,
        capture: CaptureId,
        value_type: TypeId,
    },
    Field {
        base: TypedExpression,
        field: FieldId,
        value_type: TypeId,
    },
    Array {
        array: TypedExpression,
        index: TypedExpression,
        element_type: TypeId,
    },
    Table {
        table: TypedExpression,
        key: TypedExpression,
        value_type: TypeId,
    },
}

impl TypedAssignmentTarget {
    #[must_use]
    pub const fn value_type(&self) -> TypeId {
        match self {
            Self::Local { value_type, .. }
            | Self::Capture { value_type, .. }
            | Self::Field { value_type, .. }
            | Self::Table { value_type, .. } => *value_type,
            Self::Array { element_type, .. } => *element_type,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedCall {
    pub(crate) dispatch: TypedCallDispatch,
    pub(crate) type_arguments: Vec<TypeId>,
    pub(crate) arguments: Vec<TypedExpression>,
    pub(crate) span: SourceSpan,
}

impl TypedCall {
    #[must_use]
    pub const fn dispatch(&self) -> &TypedCallDispatch {
        &self.dispatch
    }

    #[must_use]
    pub fn type_arguments(&self) -> &[TypeId] {
        &self.type_arguments
    }

    #[must_use]
    pub fn arguments(&self) -> &[TypedExpression] {
        &self.arguments
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TypedCallDispatch {
    Standard {
        function: StandardFunctionId,
    },
    Direct {
        function: SymbolId,
    },
    Referenced {
        function: SymbolIdentity,
    },
    DirectMethod {
        method: MethodId,
        receiver: Option<Box<TypedExpression>>,
    },
    InterfaceMethod {
        interface: InterfaceId,
        method: InterfaceMethodId,
        receiver: Box<TypedExpression>,
    },
    Indirect {
        callee: Box<TypedExpression>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedExpression {
    pub(crate) kind: TypedExpressionKind,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
}

impl TypedExpression {
    #[must_use]
    pub const fn kind(&self) -> &TypedExpressionKind {
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
pub enum TypedExpressionKind {
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
    Boolean(bool),
    Nil,
    AttributeQuery {
        module: ModuleId,
        attribute: AttributeId,
        subject: AttributeQuerySubject,
    },
    HasAttributeQuery {
        module: ModuleId,
        attribute: AttributeId,
        subject: AttributeQuerySubject,
    },
    Closure(TypedClosure),
    Local(LocalId),
    Parameter(ValueParameterId),
    Capture(CaptureId),
    Function(SymbolId),
    Field {
        base: Box<TypedExpression>,
        field: FieldId,
    },
    ClassConstruct {
        class: ClassId,
        definition: SymbolId,
        fields: Vec<TypedFieldValue>,
    },
    ArrayGet {
        array: Box<TypedExpression>,
        index: Box<TypedExpression>,
    },
    TableGet {
        table: Box<TypedExpression>,
        key: Box<TypedExpression>,
    },
    TupleGet {
        tuple: Box<TypedExpression>,
        index: u32,
    },
    ArrayCreate {
        length: Box<TypedExpression>,
        initial_value: Box<TypedExpression>,
    },
    ArrayLength {
        array: Box<TypedExpression>,
    },
    ArrayGetChecked {
        array: Box<TypedExpression>,
        index: Box<TypedExpression>,
    },
    ArrayFill {
        array: Box<TypedExpression>,
        value: Box<TypedExpression>,
    },
    Record {
        record: SymbolId,
        fields: Vec<TypedFieldValue>,
    },
    RecordUpdate {
        record: SymbolId,
        base: Box<TypedExpression>,
        fields: Vec<TypedFieldValue>,
    },
    Array(Vec<TypedExpression>),
    Table(Vec<TypedTableEntry>),
    UnionCase {
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<TypedExpression>,
    },
    EnumCase {
        definition: SymbolId,
        case: EnumCaseId,
        discriminant: u32,
    },
    Tuple(Vec<TypedExpression>),
    StringConcat {
        left: Box<TypedExpression>,
        right: Box<TypedExpression>,
    },
    StringFormat {
        kind: StringFormatKind,
        value: Box<TypedExpression>,
    },
    Unary {
        operator: TypedUnaryOperator,
        operand: Box<TypedExpression>,
    },
    Binary {
        operator: TypedBinaryOperator,
        left: Box<TypedExpression>,
        right: Box<TypedExpression>,
    },
    Conditional {
        condition: Box<TypedExpression>,
        when_true: Box<TypedExpression>,
        when_false: Box<TypedExpression>,
    },
    DirectCall {
        function: SymbolId,
        type_arguments: Vec<TypeId>,
        arguments: Vec<TypedExpression>,
    },
    ReferencedCall {
        function: SymbolIdentity,
        arguments: Vec<TypedExpression>,
    },
    StandardCall {
        function: StandardFunctionId,
        arguments: Vec<TypedExpression>,
    },
    IndirectCall {
        callee: Box<TypedExpression>,
        arguments: Vec<TypedExpression>,
    },
    DirectMethodCall {
        method: MethodId,
        receiver: Option<Box<TypedExpression>>,
        arguments: Vec<TypedExpression>,
    },
    InterfaceMethodCall {
        interface: InterfaceId,
        method: InterfaceMethodId,
        receiver: Box<TypedExpression>,
        arguments: Vec<TypedExpression>,
    },
    InterfaceUpcast {
        value: Box<TypedExpression>,
        interface: InterfaceId,
    },
    NumericConvert {
        value: Box<TypedExpression>,
        conversion: NumericConversionKind,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMode {
    Value,
    Cell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureSource {
    Local(LocalId),
    Parameter(ValueParameterId),
    Capture(CaptureId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedCapture {
    pub(crate) capture: CaptureId,
    pub(crate) binding: BindingId,
    pub(crate) source: CaptureSource,
    pub(crate) type_id: TypeId,
    pub(crate) mode: CaptureMode,
}

impl TypedCapture {
    #[must_use]
    pub const fn capture(&self) -> CaptureId {
        self.capture
    }

    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    #[must_use]
    pub const fn source(&self) -> CaptureSource {
        self.source
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub const fn mode(&self) -> CaptureMode {
        self.mode
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedClosureParameter {
    pub(crate) binding: BindingId,
    pub(crate) parameter: ValueParameterId,
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
}

impl TypedClosureParameter {
    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    #[must_use]
    pub const fn parameter(&self) -> ValueParameterId {
        self.parameter
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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
pub struct TypedClosure {
    pub(crate) function: NestedFunctionId,
    pub(crate) parameters: Vec<TypedClosureParameter>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) captures: Vec<TypedCapture>,
    pub(crate) body: TypedBody,
    pub(crate) span: SourceSpan,
}

impl TypedClosure {
    #[must_use]
    pub const fn function(&self) -> NestedFunctionId {
        self.function
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypedClosureParameter] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub fn captures(&self) -> &[TypedCapture] {
        &self.captures
    }

    #[must_use]
    pub const fn body(&self) -> &TypedBody {
        &self.body
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedMatchBinding {
    pub(crate) binding: Option<BindingId>,
    pub(crate) local: Option<LocalId>,
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
}

impl TypedMatchBinding {
    #[must_use]
    pub const fn binding(&self) -> Option<BindingId> {
        self.binding
    }

    #[must_use]
    pub const fn local(&self) -> Option<LocalId> {
        self.local
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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
pub struct TypedMatchArm {
    pub(crate) union: SymbolId,
    pub(crate) case: UnionCaseId,
    pub(crate) bindings: Vec<TypedMatchBinding>,
    pub(crate) body: Vec<TypedStatement>,
    pub(crate) span: SourceSpan,
}

impl TypedMatchArm {
    #[must_use]
    pub const fn union(&self) -> SymbolId {
        self.union
    }

    #[must_use]
    pub const fn case(&self) -> UnionCaseId {
        self.case
    }

    #[must_use]
    pub fn bindings(&self) -> &[TypedMatchBinding] {
        &self.bindings
    }

    #[must_use]
    pub fn body(&self) -> &[TypedStatement] {
        &self.body
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedTableEntry {
    pub(crate) key: TypedExpression,
    pub(crate) value: TypedExpression,
    pub(crate) span: SourceSpan,
}

impl TypedTableEntry {
    #[must_use]
    pub const fn key(&self) -> &TypedExpression {
        &self.key
    }

    #[must_use]
    pub const fn value(&self) -> &TypedExpression {
        &self.value
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedFieldValue {
    pub(crate) field: FieldId,
    pub(crate) value: TypedExpression,
    pub(crate) span: SourceSpan,
}

impl TypedFieldValue {
    #[must_use]
    pub const fn field(&self) -> FieldId {
        self.field
    }

    #[must_use]
    pub const fn value(&self) -> &TypedExpression {
        &self.value
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedUnaryOperator {
    Not,
    Negate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedBinaryOperator {
    Or,
    And,
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypedCompoundOperator {
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
    Concat,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StringFormatKind {
    Boolean,
    Integer(IntegerKind),
    Float(FloatKind),
}

#[derive(Clone, Debug)]
pub struct TypedBodyResult {
    pub(crate) body: Option<TypedBody>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct TypedExpressionResult {
    pub(crate) expression: Option<TypedExpression>,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl TypedExpressionResult {
    #[must_use]
    pub const fn expression(&self) -> Option<&TypedExpression> {
        self.expression.as_ref()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

impl TypedBodyResult {
    #[must_use]
    pub const fn body(&self) -> Option<&TypedBody> {
        self.body.as_ref()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        let mut snapshot = String::new();
        for diagnostic in &self.diagnostics {
            let range = diagnostic.primary_span().range();
            snapshot.push_str(diagnostic.code().as_str());
            snapshot.push('@');
            snapshot.push_str(&range.start().to_u32().to_string());
            snapshot.push_str("..");
            snapshot.push_str(&range.end().to_u32().to_string());
            snapshot.push('\n');
        }
        snapshot
    }
}
