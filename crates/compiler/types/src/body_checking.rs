use std::collections::{BTreeMap, BTreeSet};

use pop_diagnostics::{resolution as resolution_diagnostics, types as type_diagnostics};
use pop_foundation::{
    AttributeId, BindingId, CaptureId, ClassId, Diagnostic, FieldId, InterfaceId,
    InterfaceMethodId, LocalId, MethodId, ModuleId, NestedFunctionId, SourceSpan, SymbolId,
    StandardFunctionId, TextRange, TextSize, TypeId, UnionCaseId, ValueParameterId,
};
use pop_resolve::SymbolSpace;
use pop_syntax::{
    BinaryOperator as SyntaxBinaryOperator, CaptureFunctionSyntax, ExpressionSyntax,
    ExpressionSyntaxKind, FieldInitializerSyntax, FunctionBodySyntax, MatchArmSyntax,
    StatementSyntax, StatementSyntaxKind, UnaryOperator as SyntaxUnaryOperator,
};

use crate::{
    AttributeQuerySubject, FloatKind, FloatValue, IntegerKind, IntegerValue, PrimitiveType,
    ResolvedFunctionSignature, SemanticType, SignatureResolver,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedBody {
    statements: Vec<TypedStatement>,
}

impl TypedBody {
    #[must_use]
    pub fn statements(&self) -> &[TypedStatement] {
        &self.statements
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedStatement {
    kind: TypedStatementKind,
    span: SourceSpan,
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
    Call(TypedCall),
    Expression(TypedExpression),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypedCall {
    dispatch: TypedCallDispatch,
    arguments: Vec<TypedExpression>,
    span: SourceSpan,
}

impl TypedCall {
    #[must_use]
    pub const fn dispatch(&self) -> &TypedCallDispatch {
        &self.dispatch
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
    kind: TypedExpressionKind,
    type_id: TypeId,
    span: SourceSpan,
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
    Tuple(Vec<TypedExpression>),
    Unary {
        operator: TypedUnaryOperator,
        operand: Box<TypedExpression>,
    },
    Binary {
        operator: TypedBinaryOperator,
        left: Box<TypedExpression>,
        right: Box<TypedExpression>,
    },
    DirectCall {
        function: SymbolId,
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
    capture: CaptureId,
    binding: BindingId,
    source: CaptureSource,
    type_id: TypeId,
    mode: CaptureMode,
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
    binding: BindingId,
    parameter: ValueParameterId,
    name: String,
    type_id: TypeId,
    span: SourceSpan,
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
    function: NestedFunctionId,
    parameters: Vec<TypedClosureParameter>,
    results: Vec<TypeId>,
    captures: Vec<TypedCapture>,
    body: TypedBody,
    span: SourceSpan,
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
    binding: Option<BindingId>,
    local: Option<LocalId>,
    name: String,
    type_id: TypeId,
    span: SourceSpan,
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
    union: SymbolId,
    case: UnionCaseId,
    bindings: Vec<TypedMatchBinding>,
    body: Vec<TypedStatement>,
    span: SourceSpan,
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
    key: TypedExpression,
    value: TypedExpression,
    span: SourceSpan,
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
    field: FieldId,
    value: TypedExpression,
    span: SourceSpan,
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
    GreaterThan,
    Add,
    Subtract,
    Multiply,
    Divide,
    Remainder,
}

#[derive(Clone, Debug)]
pub struct TypedBodyResult {
    body: Option<TypedBody>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug)]
pub struct TypedExpressionResult {
    expression: Option<TypedExpression>,
    diagnostics: Vec<Diagnostic>,
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

#[derive(Clone, Copy)]
struct Binding {
    id: BindingId,
    kind: BindingKind,
    type_id: TypeId,
    function_depth: u32,
}

#[derive(Clone, Copy)]
enum BindingKind {
    Local(LocalId),
    Parameter(ValueParameterId),
}

impl BindingKind {
    const fn capture_source(self) -> CaptureSource {
        match self {
            Self::Local(local) => CaptureSource::Local(local),
            Self::Parameter(parameter) => CaptureSource::Parameter(parameter),
        }
    }
}

#[derive(Clone, Copy)]
struct PendingCapture {
    capture: CaptureId,
    binding: BindingId,
    source: CaptureSource,
    type_id: TypeId,
}

struct ActiveFunction {
    function: NestedFunctionId,
    depth: u32,
    next_capture: u32,
    captures: BTreeMap<BindingId, PendingCapture>,
}

enum UnionCaseLookup {
    NotUnion,
    Missing,
    Found(crate::UnionDefinition, crate::UnionCaseDefinition),
}

enum BoundPathLookup {
    NotBound,
    Error,
    Found(TypedExpression),
}

struct CheckedCall {
    call: TypedCall,
    results: Vec<TypeId>,
}

struct ResolvedClosureShape {
    parameters: Vec<(String, TypeId, SourceSpan)>,
    results: Vec<(TypeId, SourceSpan)>,
    function_type: TypeId,
}

enum CheckedInvocation {
    Call(CheckedCall),
    Value(TypedExpression),
}

#[derive(Clone, Copy)]
enum NumericTarget {
    Integer(IntegerKind),
    Float(FloatKind),
}

#[derive(Clone, Copy)]
struct ExpectedExpressionType {
    type_id: TypeId,
    declaration: Option<SymbolId>,
}

impl ExpectedExpressionType {
    const fn plain(type_id: TypeId) -> Self {
        Self {
            type_id,
            declaration: None,
        }
    }

    fn resolved(resolved: &crate::ResolvedType) -> Option<Self> {
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
    module: ModuleId,
    resolver: &'resolver mut SignatureResolver<'index>,
    signatures: &'resolver BTreeMap<SymbolId, ResolvedFunctionSignature>,
    diagnostics: Vec<Diagnostic>,
    scopes: Vec<BTreeMap<String, Binding>>,
    next_local: u32,
    next_binding: u32,
    next_nested_function: u32,
    function_depth: u32,
    active_functions: Vec<ActiveFunction>,
    written_bindings: BTreeSet<BindingId>,
    signature_stack: Vec<ResolvedFunctionSignature>,
}

impl<'resolver, 'index> BodyChecker<'resolver, 'index> {
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
            diagnostics: Vec::new(),
            scopes: vec![BTreeMap::new()],
            next_local: 0,
            next_binding: 0,
            next_nested_function: 0,
            function_depth: 0,
            active_functions: Vec::new(),
            written_bindings: BTreeSet::new(),
            signature_stack: Vec::new(),
        }
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

    fn check_statement(
        &mut self,
        signature: &ResolvedFunctionSignature,
        statement: &StatementSyntax,
    ) -> Option<TypedStatement> {
        let kind = match statement.kind() {
            StatementSyntaxKind::Local {
                name,
                annotation,
                initializer,
            } => self.check_local(signature, name, annotation.as_ref(), initializer)?,
            StatementSyntaxKind::LocalFunction { name, function } => {
                self.check_local_function(signature, name, function)?
            }
            StatementSyntaxKind::Return { values } => {
                if signature.results().len() != values.len() {
                    self.diagnostics.push(type_diagnostics::wrong_value_arity(
                        statement.span(),
                        "return",
                        signature.results().len(),
                        values.len(),
                    ));
                    return None;
                }
                let mut typed_values = Vec::new();
                for (value, expected) in values.iter().zip(signature.results()) {
                    let typed = self.check_expression_expected(
                        value,
                        ExpectedExpressionType::resolved(expected),
                    )?;
                    if let Some(expected_id) = expected.type_id() {
                        self.require_same_type(
                            expected_id,
                            typed.type_id(),
                            typed.span(),
                            expected.span(),
                        );
                    }
                    typed_values.push(typed);
                }
                TypedStatementKind::Return {
                    values: typed_values,
                }
            }
            StatementSyntaxKind::If {
                condition,
                then_body,
                else_body,
            } => {
                let condition = self.check_condition(condition)?;
                let then_body = self.check_nested_statements(signature, then_body);
                let else_body = self.check_nested_statements(signature, else_body);
                TypedStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                }
            }
            StatementSyntaxKind::While { condition, body } => {
                let condition = self.check_condition(condition)?;
                let body = self.check_nested_statements(signature, body);
                TypedStatementKind::While { condition, body }
            }
            StatementSyntaxKind::Match { scrutinee, arms } => {
                self.check_match(signature, scrutinee, arms, statement.span())?
            }
            StatementSyntaxKind::Assignment { target, value } => {
                self.check_assignment(target, value, statement.span())?
            }
            StatementSyntaxKind::Expression(expression) => {
                self.check_expression_statement(expression)?
            }
        };
        Some(TypedStatement {
            kind,
            span: statement.span(),
        })
    }

    fn check_expression_statement(
        &mut self,
        expression: &ExpressionSyntax,
    ) -> Option<TypedStatementKind> {
        let invocation = match expression.kind() {
            ExpressionSyntaxKind::Call { callee, arguments } => {
                self.check_call_invocation(callee, arguments, expression.span())
            }
            ExpressionSyntaxKind::MethodCall {
                receiver,
                method,
                arguments,
            } => self
                .check_receiver_method_invocation(receiver, method, arguments, expression.span())
                .map(CheckedInvocation::Call),
            _ => {
                return Some(TypedStatementKind::Expression(
                    self.check_expression(expression)?,
                ));
            }
        }?;
        let checked = match invocation {
            CheckedInvocation::Call(checked) => checked,
            CheckedInvocation::Value(value) => {
                return Some(TypedStatementKind::Expression(value));
            }
        };
        if checked.results.is_empty() {
            return Some(TypedStatementKind::Call(checked.call));
        }
        self.checked_call_expression(checked)
            .map(TypedStatementKind::Expression)
    }

    fn check_local(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        annotation: Option<&pop_syntax::TypeSyntax>,
        initializer: &ExpressionSyntax,
    ) -> Option<TypedStatementKind> {
        let annotation_type = if let Some(annotation) = annotation {
            let (resolved, diagnostics) =
                self.resolver
                    .resolve_annotation(self.module, annotation, signature);
            self.diagnostics.extend(diagnostics);
            Some((
                ExpectedExpressionType::resolved(&resolved?)?,
                annotation.span(),
            ))
        } else {
            None
        };
        let initializer = self.check_expression_expected(
            initializer,
            annotation_type.map(|(expected, _)| expected),
        )?;
        let local_type = if let Some((expected, origin)) = annotation_type {
            self.require_same_type(
                expected.type_id,
                initializer.type_id(),
                initializer.span(),
                origin,
            );
            expected.type_id
        } else {
            initializer.type_id()
        };
        let local = LocalId::from_raw(self.next_local);
        self.next_local = self.next_local.saturating_add(1);
        let binding = BindingId::from_raw(self.next_binding);
        self.next_binding = self.next_binding.saturating_add(1);
        self.scopes
            .last_mut()
            .expect("body checker always has a lexical scope")
            .insert(
                name.to_owned(),
                Binding {
                    id: binding,
                    kind: BindingKind::Local(local),
                    type_id: local_type,
                    function_depth: self.function_depth,
                },
            );
        Some(TypedStatementKind::Local {
            binding,
            local,
            name: name.to_owned(),
            local_type,
            initializer,
        })
    }

    fn check_local_function(
        &mut self,
        signature: &ResolvedFunctionSignature,
        name: &str,
        function: &CaptureFunctionSyntax,
    ) -> Option<TypedStatementKind> {
        let shape = self.resolve_closure_shape(signature, function)?;
        let local = LocalId::from_raw(self.next_local);
        self.next_local = self.next_local.saturating_add(1);
        let binding = BindingId::from_raw(self.next_binding);
        self.next_binding = self.next_binding.saturating_add(1);
        self.scopes
            .last_mut()
            .expect("body checker always has a lexical scope")
            .insert(
                name.to_owned(),
                Binding {
                    id: binding,
                    kind: BindingKind::Local(local),
                    type_id: shape.function_type,
                    function_depth: self.function_depth,
                },
            );
        self.written_bindings.insert(binding);
        let closure = self.check_resolved_closure(signature, function, shape)?;
        Some(TypedStatementKind::Local {
            binding,
            local,
            name: name.to_owned(),
            local_type: closure.type_id(),
            initializer: closure,
        })
    }

    fn resolve_closure_shape(
        &mut self,
        outer: &ResolvedFunctionSignature,
        function: &CaptureFunctionSyntax,
    ) -> Option<ResolvedClosureShape> {
        let mut names = BTreeMap::new();
        let mut parameters = Vec::new();
        for parameter in function.parameters() {
            if let Some(original) = names.insert(parameter.name().to_owned(), parameter.span()) {
                self.diagnostics.push(type_diagnostics::duplicate_binding(
                    parameter.span(),
                    parameter.name(),
                    original,
                ));
                continue;
            }
            let (resolved, diagnostics) =
                self.resolver
                    .resolve_annotation(self.module, parameter.parameter_type(), outer);
            self.diagnostics.extend(diagnostics);
            parameters.push((
                parameter.name().to_owned(),
                resolved?.type_id()?,
                parameter.span(),
            ));
        }
        let mut results = Vec::new();
        for result in function.results() {
            let (resolved, diagnostics) =
                self.resolver.resolve_annotation(self.module, result, outer);
            self.diagnostics.extend(diagnostics);
            results.push((resolved?.type_id()?, result.span()));
        }
        if !self.diagnostics.is_empty() {
            return None;
        }
        let function_type = self
            .resolver
            .arena_mut()
            .intern(SemanticType::Function {
                parameters: parameters.iter().map(|(_, type_id, _)| *type_id).collect(),
                results: results.iter().map(|(type_id, _)| *type_id).collect(),
                effects: crate::EffectSummary::empty(),
            })
            .ok()?;
        Some(ResolvedClosureShape {
            parameters,
            results,
            function_type,
        })
    }

    #[allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]
    fn check_resolved_closure(
        &mut self,
        outer: &ResolvedFunctionSignature,
        function: &CaptureFunctionSyntax,
        shape: ResolvedClosureShape,
    ) -> Option<TypedExpression> {
        let nested = NestedFunctionId::from_raw(self.next_nested_function);
        self.next_nested_function = self.next_nested_function.saturating_add(1);
        self.function_depth = self.function_depth.saturating_add(1);
        let depth = self.function_depth;
        self.active_functions.push(ActiveFunction {
            function: nested,
            depth,
            next_capture: 0,
            captures: BTreeMap::new(),
        });
        self.scopes.push(BTreeMap::new());

        let mut typed_parameters = Vec::new();
        for (index, (name, type_id, span)) in shape.parameters.iter().enumerate() {
            let parameter = ValueParameterId::from_raw(u32::try_from(index).unwrap_or(u32::MAX));
            let binding = BindingId::from_raw(self.next_binding);
            self.next_binding = self.next_binding.saturating_add(1);
            self.scopes
                .last_mut()
                .expect("closure scope was just pushed")
                .insert(
                    name.clone(),
                    Binding {
                        id: binding,
                        kind: BindingKind::Parameter(parameter),
                        type_id: *type_id,
                        function_depth: depth,
                    },
                );
            typed_parameters.push(TypedClosureParameter {
                binding,
                parameter,
                name: name.clone(),
                type_id: *type_id,
                span: *span,
            });
        }

        let nested_signature = ResolvedFunctionSignature::canonical(
            outer.symbol(),
            format!("{}$closure{}", outer.name(), nested.raw()),
            shape.parameters.clone(),
            shape.results.clone(),
        );
        self.signature_stack.push(nested_signature.clone());
        let mut statements = Vec::new();
        for statement in function.body() {
            if let Some(typed) = self.check_statement(&nested_signature, statement) {
                statements.push(typed);
            }
        }
        if !shape.results.is_empty() && !statements_definitely_return(&statements) {
            self.diagnostics
                .push(type_diagnostics::not_all_paths_return(function.span()));
        }
        self.signature_stack
            .pop()
            .expect("closure signature was just pushed");

        self.scopes.pop().expect("closure scope was just pushed");
        let active = self
            .active_functions
            .pop()
            .expect("closure capture context was just pushed");
        debug_assert_eq!(active.function, nested);
        self.function_depth = self.function_depth.saturating_sub(1);
        let captures = active
            .captures
            .into_values()
            .map(|capture| TypedCapture {
                capture: capture.capture,
                binding: capture.binding,
                source: capture.source,
                type_id: capture.type_id,
                mode: if self.written_bindings.contains(&capture.binding) {
                    CaptureMode::Cell
                } else {
                    CaptureMode::Value
                },
            })
            .collect();
        Some(TypedExpression {
            kind: TypedExpressionKind::Closure(TypedClosure {
                function: nested,
                parameters: typed_parameters,
                results: shape.results.iter().map(|(type_id, _)| *type_id).collect(),
                captures,
                body: TypedBody { statements },
                span: function.span(),
            }),
            type_id: shape.function_type,
            span: function.span(),
        })
    }

    fn check_closure_expression(
        &mut self,
        outer: &ResolvedFunctionSignature,
        function: &CaptureFunctionSyntax,
    ) -> Option<TypedExpression> {
        let shape = self.resolve_closure_shape(outer, function)?;
        self.check_resolved_closure(outer, function, shape)
    }

    fn check_assignment(
        &mut self,
        target: &ExpressionSyntax,
        value: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        if let ExpressionSyntaxKind::Name(path) = target.kind()
            && path.len() == 1
            && let Some(binding) = self.binding_by_name(&path[0])
        {
            let target_kind = self.binding_reference_kind(binding)?;
            if matches!(target_kind, TypedExpressionKind::Parameter(_)) {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "assignment",
                    "immutable parameter",
                ));
                return None;
            }
            let value = self.check_expression_expected(
                value,
                Some(ExpectedExpressionType::plain(binding.type_id)),
            )?;
            self.require_same_type(binding.type_id, value.type_id(), value.span(), span);
            self.written_bindings.insert(binding.id);
            return match target_kind {
                TypedExpressionKind::Local(local) => {
                    Some(TypedStatementKind::LocalSet { local, value })
                }
                TypedExpressionKind::Parameter(parameter) => {
                    Some(TypedStatementKind::ParameterSet { parameter, value })
                }
                TypedExpressionKind::Capture(capture) => {
                    Some(TypedStatementKind::CaptureSet { capture, value })
                }
                _ => None,
            };
        }
        let target = self.check_expression(target)?;
        let target_type = target.type_id();
        let TypedExpressionKind::Field { base, field } = target.kind else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "assignment",
                self.type_name(target_type),
            ));
            return None;
        };
        if self
            .resolver
            .class_definition_for_type(base.type_id())
            .is_none()
        {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "assignment",
                "immutable field",
            ));
            return None;
        }
        let value = self
            .check_expression_expected(value, Some(ExpectedExpressionType::plain(target_type)))?;
        self.require_same_type(target_type, value.type_id(), value.span(), span);
        Some(TypedStatementKind::FieldSet {
            base: *base,
            field,
            value,
        })
    }

    fn check_nested_statements(
        &mut self,
        signature: &ResolvedFunctionSignature,
        statements: &[StatementSyntax],
    ) -> Vec<TypedStatement> {
        self.scopes.push(BTreeMap::new());
        let typed = statements
            .iter()
            .filter_map(|statement| self.check_statement(signature, statement))
            .collect();
        self.scopes
            .pop()
            .expect("nested lexical scope was just pushed");
        typed
    }

    #[allow(clippy::single_match_else, clippy::too_many_lines)]
    fn check_match(
        &mut self,
        signature: &ResolvedFunctionSignature,
        scrutinee: &ExpressionSyntax,
        arms: &[MatchArmSyntax],
        span: SourceSpan,
    ) -> Option<TypedStatementKind> {
        let scrutinee = self.check_expression(scrutinee)?;
        let definition_symbol = match self.resolver.arena().get(scrutinee.type_id()) {
            Some(SemanticType::TaggedUnion { definition }) => *definition,
            _ => {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "match",
                    self.type_name(scrutinee.type_id()),
                ));
                return None;
            }
        };
        let definition = self.resolver.union_definition(definition_symbol)?.clone();
        let mut seen = BTreeMap::new();
        let mut typed_arms = Vec::new();
        for arm in arms {
            let (arm_definition, case) = match self.lookup_union_case(arm.case_path(), arm.span()) {
                UnionCaseLookup::Found(definition, case) => (definition, case),
                UnionCaseLookup::Missing | UnionCaseLookup::NotUnion => continue,
            };
            if arm_definition.symbol() != definition.symbol() {
                self.diagnostics.push(type_diagnostics::foreign_match_case(
                    arm.span(),
                    arm.case_path().join("."),
                ));
                continue;
            }
            if let Some(original) = seen.insert(case.case(), arm.span()) {
                self.diagnostics
                    .push(type_diagnostics::duplicate_match_case(
                        arm.span(),
                        case.name(),
                        original,
                    ));
                continue;
            }
            if case.parameters().len() != arm.bindings().len() {
                self.diagnostics.push(type_diagnostics::wrong_value_arity(
                    arm.span(),
                    "match case payload",
                    case.parameters().len(),
                    arm.bindings().len(),
                ));
                continue;
            }

            self.scopes.push(BTreeMap::new());
            let mut names = BTreeMap::new();
            let mut bindings = Vec::new();
            for (name, (_, type_id, parameter_span)) in arm.bindings().iter().zip(case.parameters())
            {
                if name == "_" {
                    bindings.push(TypedMatchBinding {
                        binding: None,
                        local: None,
                        name: name.clone(),
                        type_id: *type_id,
                        span: arm.span(),
                    });
                    continue;
                }
                if let Some(original) = names.insert(name.clone(), arm.span()) {
                    self.diagnostics.push(type_diagnostics::duplicate_binding(
                        arm.span(),
                        name,
                        original,
                    ));
                    continue;
                }
                let local = LocalId::from_raw(self.next_local);
                self.next_local = self.next_local.saturating_add(1);
                let binding = BindingId::from_raw(self.next_binding);
                self.next_binding = self.next_binding.saturating_add(1);
                self.scopes
                    .last_mut()
                    .expect("match arm scope was just pushed")
                    .insert(
                        name.clone(),
                        Binding {
                            id: binding,
                            kind: BindingKind::Local(local),
                            type_id: *type_id,
                            function_depth: self.function_depth,
                        },
                    );
                bindings.push(TypedMatchBinding {
                    binding: Some(binding),
                    local: Some(local),
                    name: name.clone(),
                    type_id: *type_id,
                    span: *parameter_span,
                });
            }
            let body = arm
                .body()
                .iter()
                .filter_map(|statement| self.check_statement(signature, statement))
                .collect();
            self.scopes.pop().expect("match arm scope was just pushed");
            typed_arms.push(TypedMatchArm {
                union: definition.symbol(),
                case: case.case(),
                bindings,
                body,
                span: arm.span(),
            });
        }

        let missing: Vec<_> = definition
            .cases()
            .iter()
            .filter(|case| !seen.contains_key(&case.case()))
            .collect();
        if !missing.is_empty() {
            let declaration_name = self
                .resolver
                .database()
                .index()
                .declaration(definition.symbol())
                .map_or("Union", pop_resolve::Declaration::name);
            let replacement = missing_match_arms(declaration_name, &missing);
            let insert_offset = span.range().end().to_u32().saturating_sub(3);
            let insertion = SourceSpan::new(
                span.file(),
                TextRange::empty(TextSize::from_u32(insert_offset)),
            );
            let missing_names: Vec<_> = missing.iter().map(|case| case.name()).collect();
            self.diagnostics.push(type_diagnostics::missing_match_cases(
                span,
                &missing_names,
                insertion,
                replacement,
            ));
        }

        Some(TypedStatementKind::Match {
            scrutinee,
            union: definition.symbol(),
            arms: typed_arms,
        })
    }

    fn check_condition(&mut self, condition: &ExpressionSyntax) -> Option<TypedExpression> {
        let typed = self.check_expression(condition)?;
        let boolean = self.resolver.arena().source_type("Boolean")?;
        self.require_same_type(boolean, typed.type_id(), typed.span(), condition.span());
        Some(typed)
    }

    fn check_expression(&mut self, expression: &ExpressionSyntax) -> Option<TypedExpression> {
        self.check_expression_expected(expression, None)
    }

    fn check_expression_expected(
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
                    interface: *interface,
                },
                type_id: expected.type_id,
                span: expression.span(),
            });
        }
        Some(typed)
    }

    fn check_expression_uncoerced(
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
            ExpressionSyntaxKind::String(value) => self.primitive_expression(
                TypedExpressionKind::String(value.clone()),
                "String",
                span,
            ),
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
            ExpressionSyntaxKind::Name(path) => self.check_name(path, span),
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
            } => self.check_receiver_method_call(receiver, method, arguments, span),
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
            ExpressionSyntaxKind::Binary {
                operator,
                left,
                right,
            } => self.check_binary(*operator, left, right, expected, span),
            ExpressionSyntaxKind::Call { callee, arguments } => {
                self.check_call(callee, arguments, span)
            }
            ExpressionSyntaxKind::GenericCall {
                callee,
                type_arguments,
                arguments,
            } => self.check_generic_call(callee, type_arguments, arguments, span),
        }
    }

    #[allow(clippy::too_many_lines)]
    fn check_generic_call(
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

    fn numeric_literal_expression(
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

    fn primitive_expression(
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

    fn check_name(&mut self, path: &[String], span: SourceSpan) -> Option<TypedExpression> {
        match self.check_bound_path(path, span) {
            BoundPathLookup::Found(bound) => return Some(bound),
            BoundPathLookup::Error => return None,
            BoundPathLookup::NotBound => {}
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
        let symbol = resolution.symbol()?;
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

    fn check_bound_path(&mut self, path: &[String], span: SourceSpan) -> BoundPathLookup {
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

    fn binding_by_name(&self, name: &str) -> Option<Binding> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name))
            .copied()
    }

    fn binding_reference_kind(&mut self, binding: Binding) -> Option<TypedExpressionKind> {
        if self.function_depth > binding.function_depth {
            return self
                .record_capture(binding)
                .map(TypedExpressionKind::Capture);
        }
        Some(match binding.kind {
            BindingKind::Local(local) => TypedExpressionKind::Local(local),
            BindingKind::Parameter(parameter) => TypedExpressionKind::Parameter(parameter),
        })
    }

    fn record_capture(&mut self, binding: Binding) -> Option<CaptureId> {
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

    fn check_call(
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

    fn check_call_invocation(
        &mut self,
        callee: &ExpressionSyntax,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<CheckedInvocation> {
        if let ExpressionSyntaxKind::Name(path) = callee.kind() {
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
        let dispatch = if let TypedExpressionKind::Function(function) = callee.kind() {
            TypedCallDispatch::Direct {
                function: *function,
            }
        } else {
            TypedCallDispatch::Indirect {
                callee: Box::new(callee),
            }
        };
        Some(CheckedInvocation::Call(CheckedCall {
            call: TypedCall {
                dispatch,
                arguments: typed_arguments,
                span,
            },
            results,
        }))
    }

    fn check_standard_invocation(
        &mut self,
        path: &[String],
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<CheckedCall> {
        let [name] = path else {
            return None;
        };
        if self
            .resolver
            .database()
            .resolve(self.module, name, SymbolSpace::Value, span)
            .symbol()
            .is_some()
        {
            return None;
        }
        let entry = self
            .resolver
            .schema()
            .standard_function_by_source_name(name)?;
        let function = entry.id();
        let parameter_names = entry.parameter_types().to_vec();
        let result_names = entry.result_types().to_vec();
        if arguments.len() != parameter_names.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "call",
                parameter_names.len(),
                arguments.len(),
            ));
            return None;
        }
        let parameter_types = parameter_names
            .iter()
            .map(|name| self.resolver.arena().source_type(name))
            .collect::<Option<Vec<_>>>()?;
        let result_types = result_names
            .iter()
            .map(|name| self.resolver.arena().source_type(name))
            .collect::<Option<Vec<_>>>()?;
        let typed_arguments = arguments
            .iter()
            .zip(parameter_types)
            .map(|(argument, expected)| {
                self.check_expression_expected(
                    argument,
                    Some(ExpectedExpressionType::plain(expected)),
                )
            })
            .collect::<Option<Vec<_>>>()?;
        Some(CheckedCall {
            call: TypedCall {
                dispatch: TypedCallDispatch::Standard { function },
                arguments: typed_arguments,
                span,
            },
            results: result_types,
        })
    }

    fn check_static_method_invocation(
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

    fn check_receiver_method_call(
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

    fn check_receiver_method_invocation(
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

    fn check_interface_method_invocation(
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

    fn check_direct_method_invocation(
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

    fn checked_call_expression(&mut self, checked: CheckedCall) -> Option<TypedExpression> {
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

    fn lookup_union_case(&mut self, path: &[String], span: SourceSpan) -> UnionCaseLookup {
        if path.len() < 2 {
            return UnionCaseLookup::NotUnion;
        }
        let type_name = path[..path.len() - 1].join(".");
        let resolution =
            self.resolver
                .database()
                .resolve(self.module, &type_name, SymbolSpace::Type, span);
        let Some(symbol) = resolution.symbol() else {
            return UnionCaseLookup::NotUnion;
        };
        let Some(definition) = self.resolver.union_definition(symbol).cloned() else {
            return UnionCaseLookup::NotUnion;
        };
        let case_name = &path[path.len() - 1];
        let Some(case) = definition
            .cases()
            .iter()
            .find(|case| case.name() == case_name)
            .cloned()
        else {
            self.diagnostics
                .push(resolution_diagnostics::unknown_name(span, path.join(".")));
            return UnionCaseLookup::Missing;
        };
        UnionCaseLookup::Found(definition, case)
    }

    fn check_union_case_call(
        &mut self,
        definition: &crate::UnionDefinition,
        case: &crate::UnionCaseDefinition,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if case.parameters().len() != arguments.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "union case",
                case.parameters().len(),
                arguments.len(),
            ));
            return None;
        }
        let mut typed_arguments = Vec::new();
        for (argument, (_, parameter_type, parameter_span)) in
            arguments.iter().zip(case.parameters())
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
        Some(TypedExpression {
            kind: TypedExpressionKind::UnionCase {
                union: definition.symbol(),
                case: case.case(),
                arguments: typed_arguments,
            },
            type_id: definition.type_id(),
            span,
        })
    }

    fn check_class_construct(
        &mut self,
        type_name: &[String],
        fields: &[FieldInitializerSyntax],
        expected: Option<TypeId>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let resolution = self.resolver.database().resolve(
            self.module,
            &type_name.join("."),
            SymbolSpace::Type,
            span,
        );
        if !resolution.diagnostics().is_empty() {
            self.diagnostics
                .extend(resolution.diagnostics().iter().cloned());
            return None;
        }
        let symbol = resolution.symbol()?;
        let Some(definition) = self.resolver.class_definition(symbol).cloned() else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "construct",
                type_name.join("."),
            ));
            return None;
        };
        if let Some(expected) = expected {
            self.require_same_type(expected, definition.type_id(), span, span);
        }
        let typed_fields = self.check_class_fields(&definition, fields, span)?;
        self.diagnostics.is_empty().then_some(TypedExpression {
            kind: TypedExpressionKind::ClassConstruct {
                class: definition.class(),
                definition: definition.symbol(),
                fields: typed_fields,
            },
            type_id: definition.type_id(),
            span,
        })
    }

    fn check_class_fields(
        &mut self,
        definition: &crate::ClassDefinition,
        fields: &[FieldInitializerSyntax],
        span: SourceSpan,
    ) -> Option<Vec<TypedFieldValue>> {
        let mut seen = BTreeMap::new();
        let mut typed = Vec::new();
        for field_syntax in fields {
            if let Some(original) = seen.insert(field_syntax.name().to_owned(), field_syntax.span())
            {
                self.diagnostics
                    .push(type_diagnostics::duplicate_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                        original,
                    ));
                continue;
            }
            let Some(field) = definition
                .fields()
                .iter()
                .find(|field| field.name() == field_syntax.name())
            else {
                self.diagnostics
                    .push(type_diagnostics::unknown_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                    ));
                continue;
            };
            if !self.can_access_class_member(definition, field.visibility()) {
                self.diagnostics
                    .push(resolution_diagnostics::inaccessible_name(
                        field_syntax.span(),
                        field.name(),
                        field.span(),
                    ));
                continue;
            }
            let value = self.check_expression_expected(
                field_syntax.value(),
                Some(ExpectedExpressionType::plain(field.field_type())),
            )?;
            self.require_same_type(
                field.field_type(),
                value.type_id(),
                value.span(),
                field.span(),
            );
            typed.push(TypedFieldValue {
                field: field.field(),
                value,
                span: field_syntax.span(),
            });
        }
        for field in definition.fields() {
            if seen.contains_key(field.name()) {
                continue;
            }
            if let Some(default) = field.default() {
                typed.push(TypedFieldValue {
                    field: field.field(),
                    value: typed_field_default(default, field.field_type(), field.span()),
                    span: field.span(),
                });
            } else {
                self.diagnostics
                    .push(type_diagnostics::missing_record_field(span, field.name()));
            }
        }
        self.diagnostics.is_empty().then_some(typed)
    }

    fn check_array_literal(
        &mut self,
        elements: &[ExpressionSyntax],
        expected: Option<TypeId>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let Some(expected) = expected else {
            self.diagnostics
                .push(type_diagnostics::aggregate_needs_context(span));
            return None;
        };
        let Some(SemanticType::Array(element_type)) = self.resolver.arena().get(expected).cloned()
        else {
            self.diagnostics
                .push(type_diagnostics::aggregate_needs_context(span));
            return None;
        };
        let mut typed_elements = Vec::with_capacity(elements.len());
        for element in elements {
            let typed = self.check_expression_expected(
                element,
                Some(ExpectedExpressionType::plain(element_type)),
            )?;
            self.require_same_type(element_type, typed.type_id(), typed.span(), span);
            typed_elements.push(typed);
        }
        self.diagnostics.is_empty().then_some(TypedExpression {
            kind: TypedExpressionKind::Array(typed_elements),
            type_id: expected,
            span,
        })
    }

    fn check_array_get(
        &mut self,
        base: &ExpressionSyntax,
        index: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let array = self.check_expression(base)?;
        let Some(SemanticType::Array(element_type)) =
            self.resolver.arena().get(array.type_id()).cloned()
        else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "[]",
                self.type_name(array.type_id()),
            ));
            return None;
        };
        let index_type = self.resolver.arena().source_type("Int")?;
        let typed_index =
            self.check_expression_expected(index, Some(ExpectedExpressionType::plain(index_type)))?;
        self.require_same_type(index_type, typed_index.type_id(), typed_index.span(), span);
        let result_type = self.resolver.arena_mut().optional(element_type).ok()?;
        self.diagnostics.is_empty().then_some(TypedExpression {
            kind: TypedExpressionKind::ArrayGet {
                array: Box::new(array),
                index: Box::new(typed_index),
            },
            type_id: result_type,
            span,
        })
    }

    fn check_aggregate_literal(
        &mut self,
        fields: &[FieldInitializerSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let Some(expected) = expected else {
            self.diagnostics
                .push(type_diagnostics::aggregate_needs_context(span));
            return None;
        };
        let definition = expected
            .declaration
            .and_then(|symbol| self.resolver.record_definition(symbol))
            .filter(|definition| definition.type_id() == expected.type_id)
            .cloned()
            .or_else(|| {
                self.resolver
                    .record_definition_for_type(expected.type_id)
                    .cloned()
            });
        if let Some(definition) = definition {
            let typed_fields = self.check_record_fields(&definition, fields, true, span)?;
            return Some(TypedExpression {
                kind: TypedExpressionKind::Record {
                    record: definition.symbol(),
                    fields: typed_fields,
                },
                type_id: expected.type_id,
                span,
            });
        }
        match self.resolver.arena().get(expected.type_id).cloned() {
            Some(SemanticType::Array(_)) if fields.is_empty() => {
                self.check_array_literal(&[], Some(expected.type_id), span)
            }
            Some(SemanticType::Table { key, value }) => {
                self.check_named_table_literal(fields, expected.type_id, key, value, span)
            }
            _ => {
                self.diagnostics
                    .push(type_diagnostics::aggregate_needs_context(span));
                None
            }
        }
    }

    fn check_named_table_literal(
        &mut self,
        fields: &[FieldInitializerSyntax],
        table_type: TypeId,
        key_type: TypeId,
        value_type: TypeId,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let string_type = self.resolver.arena().source_type("String")?;
        let mut entries = Vec::with_capacity(fields.len());
        for field in fields {
            let key = TypedExpression {
                kind: TypedExpressionKind::String(format!("\"{}\"", field.name())),
                type_id: string_type,
                span: field.span(),
            };
            self.require_same_type(key_type, string_type, field.span(), span);
            let value = self.check_expression_expected(
                field.value(),
                Some(ExpectedExpressionType::plain(value_type)),
            )?;
            self.require_same_type(value_type, value.type_id(), value.span(), span);
            entries.push(TypedTableEntry {
                key,
                value,
                span: field.span(),
            });
        }
        self.diagnostics.is_empty().then_some(TypedExpression {
            kind: TypedExpressionKind::Table(entries),
            type_id: table_type,
            span,
        })
    }

    fn check_record_update(
        &mut self,
        base: &ExpressionSyntax,
        fields: &[FieldInitializerSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let base = self.check_expression(base)?;
        let Some(definition) = self
            .resolver
            .record_definition_for_type(base.type_id())
            .cloned()
        else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "with",
                self.type_name(base.type_id()),
            ));
            return None;
        };
        let typed_fields = self.check_record_fields(&definition, fields, false, span)?;
        let type_id = base.type_id();
        Some(TypedExpression {
            kind: TypedExpressionKind::RecordUpdate {
                record: definition.symbol(),
                base: Box::new(base),
                fields: typed_fields,
            },
            type_id,
            span,
        })
    }

    fn check_record_fields(
        &mut self,
        definition: &crate::RecordDefinition,
        fields: &[FieldInitializerSyntax],
        require_complete: bool,
        aggregate_span: SourceSpan,
    ) -> Option<Vec<TypedFieldValue>> {
        let mut seen = BTreeMap::new();
        let mut typed = Vec::new();
        for field_syntax in fields {
            if let Some(original) = seen.insert(field_syntax.name().to_owned(), field_syntax.span())
            {
                self.diagnostics
                    .push(type_diagnostics::duplicate_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                        original,
                    ));
                continue;
            }
            let Some(field) = definition
                .fields()
                .iter()
                .find(|field| field.name() == field_syntax.name())
            else {
                self.diagnostics
                    .push(type_diagnostics::unknown_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                    ));
                continue;
            };
            let value = self.check_expression_expected(
                field_syntax.value(),
                Some(ExpectedExpressionType::plain(field.field_type())),
            )?;
            self.require_same_type(
                field.field_type(),
                value.type_id(),
                value.span(),
                field.span(),
            );
            typed.push(TypedFieldValue {
                field: field.field(),
                value,
                span: field_syntax.span(),
            });
        }
        if require_complete {
            for field in definition.fields() {
                if seen.contains_key(field.name()) {
                    continue;
                }
                if let Some(default) = field.default() {
                    typed.push(TypedFieldValue {
                        field: field.field(),
                        value: typed_field_default(default, field.field_type(), field.span()),
                        span: field.span(),
                    });
                } else {
                    self.diagnostics
                        .push(type_diagnostics::missing_record_field(
                            aggregate_span,
                            field.name(),
                        ));
                }
            }
        }
        self.diagnostics.is_empty().then_some(typed)
    }

    fn check_unary(
        &mut self,
        operator: SyntaxUnaryOperator,
        operand: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if operator == SyntaxUnaryOperator::Negate
            && let ExpressionSyntaxKind::Integer(value) = operand.kind()
        {
            return self.numeric_literal_expression(
                value,
                expected.map(|expected| expected.type_id),
                true,
                span,
            );
        }
        let operand_expectation = match operator {
            SyntaxUnaryOperator::Not => self
                .resolver
                .arena()
                .source_type("Boolean")
                .map(ExpectedExpressionType::plain),
            SyntaxUnaryOperator::Negate => {
                expected.filter(|expected| self.is_numeric(expected.type_id))
            }
        };
        let operand = self.check_expression_expected(operand, operand_expectation)?;
        let valid = match operator {
            SyntaxUnaryOperator::Not => {
                self.is_primitive(operand.type_id(), PrimitiveType::Boolean)
            }
            SyntaxUnaryOperator::Negate => self.is_negatable_numeric(operand.type_id()),
        };
        if !valid {
            self.invalid_operator(span, unary_text(operator), &[operand.type_id()]);
            return None;
        }
        let type_id = match operator {
            SyntaxUnaryOperator::Not => self.resolver.arena().source_type("Boolean")?,
            SyntaxUnaryOperator::Negate => operand.type_id(),
        };
        Some(TypedExpression {
            kind: TypedExpressionKind::Unary {
                operator: typed_unary(operator),
                operand: Box::new(operand),
            },
            type_id,
            span,
        })
    }

    fn check_binary(
        &mut self,
        operator: SyntaxBinaryOperator,
        left: &ExpressionSyntax,
        right: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let (left, right) = self.check_binary_operands(operator, left, right, expected)?;
        let operands_match = left.type_id() == right.type_id();
        let valid = match operator {
            SyntaxBinaryOperator::Or | SyntaxBinaryOperator::And => {
                operands_match && self.is_primitive(left.type_id(), PrimitiveType::Boolean)
            }
            SyntaxBinaryOperator::Equal | SyntaxBinaryOperator::NotEqual => {
                self.equality_comparable(left.type_id(), right.type_id())
            }
            SyntaxBinaryOperator::LessThan | SyntaxBinaryOperator::GreaterThan => {
                operands_match && self.is_numeric(left.type_id())
            }
            SyntaxBinaryOperator::Add
            | SyntaxBinaryOperator::Subtract
            | SyntaxBinaryOperator::Multiply
            | SyntaxBinaryOperator::Divide => operands_match && self.is_numeric(left.type_id()),
            SyntaxBinaryOperator::Remainder => operands_match && self.is_integer(left.type_id()),
        };
        if !valid {
            self.invalid_operator(
                span,
                binary_text(operator),
                &[left.type_id(), right.type_id()],
            );
            return None;
        }
        let type_id = match operator {
            SyntaxBinaryOperator::Equal
            | SyntaxBinaryOperator::NotEqual
            | SyntaxBinaryOperator::LessThan
            | SyntaxBinaryOperator::GreaterThan => self.resolver.arena().source_type("Boolean")?,
            _ => left.type_id(),
        };
        Some(TypedExpression {
            kind: TypedExpressionKind::Binary {
                operator: typed_binary(operator),
                left: Box::new(left),
                right: Box::new(right),
            },
            type_id,
            span,
        })
    }

    fn check_binary_operands(
        &mut self,
        operator: SyntaxBinaryOperator,
        left: &ExpressionSyntax,
        right: &ExpressionSyntax,
        expected: Option<ExpectedExpressionType>,
    ) -> Option<(TypedExpression, TypedExpression)> {
        if matches!(
            operator,
            SyntaxBinaryOperator::And | SyntaxBinaryOperator::Or
        ) {
            let boolean = self
                .resolver
                .arena()
                .source_type("Boolean")
                .map(ExpectedExpressionType::plain);
            return Some((
                self.check_expression_expected(left, boolean)?,
                self.check_expression_expected(right, boolean)?,
            ));
        }
        let arithmetic = matches!(
            operator,
            SyntaxBinaryOperator::Add
                | SyntaxBinaryOperator::Subtract
                | SyntaxBinaryOperator::Multiply
                | SyntaxBinaryOperator::Divide
                | SyntaxBinaryOperator::Remainder
        );
        let outer_numeric = arithmetic
            .then_some(expected)
            .flatten()
            .filter(|expected| self.is_numeric(expected.type_id));
        if let Some(expected) = outer_numeric {
            return Some((
                self.check_expression_expected(left, Some(expected))?,
                self.check_expression_expected(right, Some(expected))?,
            ));
        }
        if matches!(left.kind(), ExpressionSyntaxKind::Integer(_))
            && !matches!(right.kind(), ExpressionSyntaxKind::Integer(_))
        {
            let right = self.check_expression(right)?;
            let expectation = self
                .is_numeric(right.type_id())
                .then_some(ExpectedExpressionType::plain(right.type_id()));
            let left = self.check_expression_expected(left, expectation)?;
            return Some((left, right));
        }
        let left = self.check_expression(left)?;
        let expectation = self
            .is_numeric(left.type_id())
            .then_some(ExpectedExpressionType::plain(left.type_id()));
        let right = self.check_expression_expected(right, expectation)?;
        Some((left, right))
    }

    fn require_same_type(
        &mut self,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
        expected_origin: SourceSpan,
    ) {
        if expected != found {
            self.diagnostics.push(type_diagnostics::type_mismatch(
                span,
                self.type_name(expected),
                self.type_name(found),
                expected_origin,
            ));
        }
    }

    fn invalid_operator(&mut self, span: SourceSpan, operator: &str, operands: &[TypeId]) {
        let operands = operands
            .iter()
            .map(|type_id| self.type_name(*type_id))
            .collect::<Vec<_>>()
            .join(", ");
        self.diagnostics
            .push(type_diagnostics::invalid_operator(span, operator, operands));
    }

    fn is_numeric(&self, type_id: TypeId) -> bool {
        self.numeric_target(type_id).is_some()
    }

    fn is_integer(&self, type_id: TypeId) -> bool {
        matches!(
            self.resolver.arena().get(type_id),
            Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
        )
    }

    fn is_negatable_numeric(&self, type_id: TypeId) -> bool {
        matches!(
            self.numeric_target(type_id),
            Some(NumericTarget::Integer(kind)) if kind.is_signed()
        ) || matches!(self.numeric_target(type_id), Some(NumericTarget::Float(_)))
    }

    fn numeric_target(&self, type_id: TypeId) -> Option<NumericTarget> {
        match self.resolver.arena().get(type_id) {
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
                Some(NumericTarget::Integer(*kind))
            }
            Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
                Some(NumericTarget::Float(FloatKind::Float32))
            }
            Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
                Some(NumericTarget::Float(FloatKind::Float64))
            }
            _ => None,
        }
    }

    fn equality_comparable(&self, left: TypeId, right: TypeId) -> bool {
        left == right && self.supports_default_equality(left)
    }

    fn supports_default_equality(&self, type_id: TypeId) -> bool {
        match self.resolver.arena().get(type_id) {
            Some(
                SemanticType::Primitive(
                    PrimitiveType::Nil
                    | PrimitiveType::Boolean
                    | PrimitiveType::Integer(_)
                    | PrimitiveType::String,
                )
                | SemanticType::Class { .. },
            ) => true,
            Some(SemanticType::Tuple(elements) | SemanticType::Union(elements)) => elements
                .iter()
                .all(|element| self.supports_default_equality(*element)),
            Some(SemanticType::Record(fields)) => fields
                .iter()
                .all(|(_, field_type)| self.supports_default_equality(*field_type)),
            _ => false,
        }
    }

    fn can_access_class_member(
        &self,
        definition: &crate::ClassDefinition,
        visibility: pop_resolve::Visibility,
    ) -> bool {
        match visibility {
            pop_resolve::Visibility::Public => true,
            pop_resolve::Visibility::Private => self.module == definition.module(),
            pop_resolve::Visibility::Internal => self
                .resolver
                .database()
                .index()
                .module(self.module)
                .is_some_and(|module| module.bubble() == definition.bubble()),
        }
    }

    fn is_primitive(&self, type_id: TypeId, primitive: PrimitiveType) -> bool {
        self.resolver.arena().get(type_id) == Some(&SemanticType::Primitive(primitive))
    }

    fn type_name(&self, type_id: TypeId) -> String {
        match self.resolver.arena().get(type_id) {
            Some(SemanticType::Primitive(primitive)) => primitive_name(*primitive).to_owned(),
            Some(SemanticType::Tuple(elements)) => format!("tuple/{}", elements.len()),
            Some(SemanticType::Function { .. }) => "function".to_owned(),
            Some(_) => format!("type#{}", type_id.raw()),
            None => format!("invalid-type#{}", type_id.raw()),
        }
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

fn typed_field_default(
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

fn statements_definitely_return(statements: &[TypedStatement]) -> bool {
    statements.iter().any(|statement| match statement.kind() {
        TypedStatementKind::Return { .. } => true,
        TypedStatementKind::If {
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
        TypedStatementKind::Local { .. }
        | TypedStatementKind::LocalSet { .. }
        | TypedStatementKind::ParameterSet { .. }
        | TypedStatementKind::CaptureSet { .. }
        | TypedStatementKind::While { .. }
        | TypedStatementKind::FieldSet { .. }
        | TypedStatementKind::Call(_)
        | TypedStatementKind::Expression(_) => false,
    })
}

fn missing_match_arms(union_name: &str, cases: &[&crate::UnionCaseDefinition]) -> String {
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

fn finalize_capture_modes(body: &mut TypedBody, written: &BTreeSet<BindingId>) {
    for statement in &mut body.statements {
        finalize_statement_captures(statement, written);
    }
}

fn finalize_statement_captures(statement: &mut TypedStatement, written: &BTreeSet<BindingId>) {
    match &mut statement.kind {
        TypedStatementKind::Local { initializer, .. } => {
            finalize_expression_captures(initializer, written);
        }
        TypedStatementKind::LocalSet { value, .. }
        | TypedStatementKind::ParameterSet { value, .. }
        | TypedStatementKind::CaptureSet { value, .. }
        | TypedStatementKind::Expression(value) => finalize_expression_captures(value, written),
        TypedStatementKind::Return { values } => {
            for value in values {
                finalize_expression_captures(value, written);
            }
        }
        TypedStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            finalize_expression_captures(condition, written);
            for statement in then_body.iter_mut().chain(else_body.iter_mut()) {
                finalize_statement_captures(statement, written);
            }
        }
        TypedStatementKind::While { condition, body } => {
            finalize_expression_captures(condition, written);
            for statement in body {
                finalize_statement_captures(statement, written);
            }
        }
        TypedStatementKind::Match {
            scrutinee, arms, ..
        } => {
            finalize_expression_captures(scrutinee, written);
            for arm in arms {
                for statement in &mut arm.body {
                    finalize_statement_captures(statement, written);
                }
            }
        }
        TypedStatementKind::FieldSet { base, value, .. } => {
            finalize_expression_captures(base, written);
            finalize_expression_captures(value, written);
        }
        TypedStatementKind::Call(call) => finalize_call_captures(call, written),
    }
}

fn finalize_call_captures(call: &mut TypedCall, written: &BTreeSet<BindingId>) {
    match &mut call.dispatch {
        TypedCallDispatch::Standard { .. } => {}
        TypedCallDispatch::Direct { .. } => {}
        TypedCallDispatch::DirectMethod { receiver, .. } => {
            if let Some(receiver) = receiver {
                finalize_expression_captures(receiver, written);
            }
        }
        TypedCallDispatch::InterfaceMethod { receiver, .. } => {
            finalize_expression_captures(receiver, written);
        }
        TypedCallDispatch::Indirect { callee } => finalize_expression_captures(callee, written),
    }
    for argument in &mut call.arguments {
        finalize_expression_captures(argument, written);
    }
}

fn finalize_expression_captures(expression: &mut TypedExpression, written: &BTreeSet<BindingId>) {
    match &mut expression.kind {
        TypedExpressionKind::Integer(_)
        | TypedExpressionKind::Float(_)
        | TypedExpressionKind::String(_)
        | TypedExpressionKind::Boolean(_)
        | TypedExpressionKind::Nil
        | TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. }
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::Parameter(_)
        | TypedExpressionKind::Capture(_)
        | TypedExpressionKind::Function(_) => {}
        TypedExpressionKind::Closure(closure) => {
            for capture in &mut closure.captures {
                capture.mode = if written.contains(&capture.binding) {
                    CaptureMode::Cell
                } else {
                    CaptureMode::Value
                };
            }
            finalize_capture_modes(&mut closure.body, written);
        }
        TypedExpressionKind::Field { base, .. } => finalize_expression_captures(base, written),
        TypedExpressionKind::ClassConstruct { fields, .. }
        | TypedExpressionKind::Record { fields, .. } => {
            for field in fields {
                finalize_expression_captures(&mut field.value, written);
            }
        }
        TypedExpressionKind::ArrayGet { array, index } => {
            finalize_expression_captures(array, written);
            finalize_expression_captures(index, written);
        }
        TypedExpressionKind::RecordUpdate { base, fields, .. } => {
            finalize_expression_captures(base, written);
            for field in fields {
                finalize_expression_captures(&mut field.value, written);
            }
        }
        TypedExpressionKind::Array(elements) | TypedExpressionKind::Tuple(elements) => {
            for element in elements {
                finalize_expression_captures(element, written);
            }
        }
        TypedExpressionKind::Table(entries) => {
            for entry in entries {
                finalize_expression_captures(&mut entry.key, written);
                finalize_expression_captures(&mut entry.value, written);
            }
        }
        TypedExpressionKind::UnionCase { arguments, .. }
        | TypedExpressionKind::DirectCall { arguments, .. }
        | TypedExpressionKind::StandardCall { arguments, .. } => {
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::Unary { operand, .. } => {
            finalize_expression_captures(operand, written);
        }
        TypedExpressionKind::Binary { left, right, .. } => {
            finalize_expression_captures(left, written);
            finalize_expression_captures(right, written);
        }
        TypedExpressionKind::IndirectCall { callee, arguments } => {
            finalize_expression_captures(callee, written);
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::DirectMethodCall {
            receiver,
            arguments,
            ..
        } => {
            if let Some(receiver) = receiver {
                finalize_expression_captures(receiver, written);
            }
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::InterfaceMethodCall {
            receiver,
            arguments,
            ..
        } => {
            finalize_expression_captures(receiver, written);
            for argument in arguments {
                finalize_expression_captures(argument, written);
            }
        }
        TypedExpressionKind::InterfaceUpcast { value, .. } => {
            finalize_expression_captures(value, written);
        }
    }
}

const fn typed_unary(operator: SyntaxUnaryOperator) -> TypedUnaryOperator {
    match operator {
        SyntaxUnaryOperator::Not => TypedUnaryOperator::Not,
        SyntaxUnaryOperator::Negate => TypedUnaryOperator::Negate,
    }
}

const fn typed_binary(operator: SyntaxBinaryOperator) -> TypedBinaryOperator {
    match operator {
        SyntaxBinaryOperator::Or => TypedBinaryOperator::Or,
        SyntaxBinaryOperator::And => TypedBinaryOperator::And,
        SyntaxBinaryOperator::Equal => TypedBinaryOperator::Equal,
        SyntaxBinaryOperator::NotEqual => TypedBinaryOperator::NotEqual,
        SyntaxBinaryOperator::LessThan => TypedBinaryOperator::LessThan,
        SyntaxBinaryOperator::GreaterThan => TypedBinaryOperator::GreaterThan,
        SyntaxBinaryOperator::Add => TypedBinaryOperator::Add,
        SyntaxBinaryOperator::Subtract => TypedBinaryOperator::Subtract,
        SyntaxBinaryOperator::Multiply => TypedBinaryOperator::Multiply,
        SyntaxBinaryOperator::Divide => TypedBinaryOperator::Divide,
        SyntaxBinaryOperator::Remainder => TypedBinaryOperator::Remainder,
    }
}

const fn unary_text(operator: SyntaxUnaryOperator) -> &'static str {
    match operator {
        SyntaxUnaryOperator::Not => "not",
        SyntaxUnaryOperator::Negate => "unary -",
    }
}

const fn binary_text(operator: SyntaxBinaryOperator) -> &'static str {
    match operator {
        SyntaxBinaryOperator::Or => "or",
        SyntaxBinaryOperator::And => "and",
        SyntaxBinaryOperator::Equal => "==",
        SyntaxBinaryOperator::NotEqual => "~=",
        SyntaxBinaryOperator::LessThan => "<",
        SyntaxBinaryOperator::GreaterThan => ">",
        SyntaxBinaryOperator::Add => "+",
        SyntaxBinaryOperator::Subtract => "-",
        SyntaxBinaryOperator::Multiply => "*",
        SyntaxBinaryOperator::Divide => "/",
        SyntaxBinaryOperator::Remainder => "%",
    }
}

const fn primitive_name(primitive: PrimitiveType) -> &'static str {
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
