//! Typed, resolved, backend-neutral high-level IR.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use pop_foundation::{
    AttributeId, BubbleId, ClassId, FieldId, FunctionId, LocalId, MethodId, ModuleId, NamespaceId,
    SourceSpan, SymbolId, TypeId, UnionCaseId, ValueParameterId,
};
use pop_resolve::Visibility;
use pop_types::{
    AttributeConstant, AttributeDefinition, ClassDefinition, ClassFieldDefault,
    ClassMethodDefinition, ClassMethodDispatch, FieldDefault, FloatValue, IntegerValue,
    PrimitiveType, RecordDefinition, ResolvedAttribute, ResolvedFunctionSignature, SemanticType,
    TypeArena, TypedBinaryOperator, TypedBody, TypedCall, TypedCallDispatch, TypedExpression,
    TypedExpressionKind, TypedFieldValue, TypedStatement, TypedStatementKind, TypedTableEntry,
    TypedUnaryOperator, UnionDefinition,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirBubble {
    bubble: BubbleId,
    namespace: NamespaceId,
    dependencies: Vec<BubbleId>,
    declarations: Vec<HirDeclaration>,
    functions: Vec<HirFunction>,
    methods: Vec<HirMethod>,
    public_symbols: Vec<SymbolId>,
}

impl HirBubble {
    /// Creates a deterministic HIR Bubble and derives its public symbol surface.
    ///
    /// # Errors
    ///
    /// Returns an error if a function has the wrong Bubble owner or two
    /// functions carry the same stable symbol.
    pub fn new(
        bubble: BubbleId,
        namespace: NamespaceId,
        dependencies: Vec<BubbleId>,
        functions: Vec<HirFunction>,
    ) -> Result<Self, HirBubbleError> {
        Self::new_with_methods(bubble, namespace, dependencies, functions, Vec::new())
    }

    /// Creates a deterministic HIR Bubble with native class methods.
    ///
    /// # Errors
    ///
    /// Returns an ownership or duplicate callable error.
    pub fn new_with_methods(
        bubble: BubbleId,
        namespace: NamespaceId,
        dependencies: Vec<BubbleId>,
        functions: Vec<HirFunction>,
        methods: Vec<HirMethod>,
    ) -> Result<Self, HirBubbleError> {
        Self::new_with_declarations_and_methods(
            bubble,
            namespace,
            dependencies,
            Vec::new(),
            functions,
            methods,
        )
    }

    /// Creates a deterministic HIR Bubble with retained typed declarations and methods.
    ///
    /// # Errors
    ///
    /// Returns an ownership or duplicate stable-identity error.
    pub fn new_with_declarations_and_methods(
        bubble: BubbleId,
        namespace: NamespaceId,
        mut dependencies: Vec<BubbleId>,
        mut declarations: Vec<HirDeclaration>,
        mut functions: Vec<HirFunction>,
        mut methods: Vec<HirMethod>,
    ) -> Result<Self, HirBubbleError> {
        dependencies.sort_unstable();
        dependencies.dedup();
        declarations.sort_by_key(HirDeclaration::symbol);
        let mut previous_declaration = None;
        for declaration in &declarations {
            if declaration.bubble() != bubble {
                return Err(HirBubbleError::WrongOwner {
                    symbol: declaration.symbol(),
                    expected: bubble,
                    found: declaration.bubble(),
                });
            }
            if previous_declaration == Some(declaration.symbol()) {
                return Err(HirBubbleError::DuplicateDeclaration(declaration.symbol()));
            }
            previous_declaration = Some(declaration.symbol());
        }
        functions.sort_by_key(HirFunction::symbol);
        let mut previous = None;
        for function in &functions {
            if function.bubble() != bubble {
                return Err(HirBubbleError::WrongOwner {
                    symbol: function.symbol(),
                    expected: bubble,
                    found: function.bubble(),
                });
            }
            if previous == Some(function.symbol()) {
                return Err(HirBubbleError::DuplicateFunction(function.symbol()));
            }
            previous = Some(function.symbol());
        }
        let mut public_symbols: Vec<_> = declarations
            .iter()
            .filter(|declaration| declaration.visibility() == Visibility::Public)
            .map(HirDeclaration::symbol)
            .chain(
                functions
                    .iter()
                    .filter(|function| function.visibility() == Visibility::Public)
                    .map(HirFunction::symbol),
            )
            .collect();
        public_symbols.sort_unstable();
        public_symbols.dedup();
        methods.sort_by_key(HirMethod::method);
        let mut previous_method = None;
        for method in &methods {
            if method.bubble() != bubble {
                return Err(HirBubbleError::WrongOwner {
                    symbol: method.definition(),
                    expected: bubble,
                    found: method.bubble(),
                });
            }
            if previous_method == Some(method.method()) {
                return Err(HirBubbleError::DuplicateMethod(method.method()));
            }
            previous_method = Some(method.method());
        }
        Ok(Self {
            bubble,
            namespace,
            dependencies,
            declarations,
            functions,
            methods,
            public_symbols,
        })
    }

    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub const fn namespace(&self) -> NamespaceId {
        self.namespace
    }

    #[must_use]
    pub fn dependencies(&self) -> &[BubbleId] {
        &self.dependencies
    }

    #[must_use]
    pub fn declarations(&self) -> &[HirDeclaration] {
        &self.declarations
    }

    #[must_use]
    pub fn functions(&self) -> &[HirFunction] {
        &self.functions
    }

    #[must_use]
    pub fn methods(&self) -> &[HirMethod] {
        &self.methods
    }

    #[must_use]
    pub fn public_symbols(&self) -> &[SymbolId] {
        &self.public_symbols
    }

    /// Independently verifies this complete HIR Bubble against its semantic
    /// type arena and the declaration/callable schema carried by the Bubble.
    ///
    /// # Errors
    ///
    /// Returns every deterministic HIR invariant violation found in the
    /// Bubble. A caller must not publish or lower a Bubble that fails this
    /// check.
    pub fn verify(&self, arena: &TypeArena) -> Result<(), Vec<HirVerificationError>> {
        verify_hir_bubble(self, arena)
    }

    #[must_use]
    pub fn dump(&self, arena: &TypeArena) -> String {
        let mut output = format!(
            "hir bubble b{} namespace n{}\n",
            self.bubble.raw(),
            self.namespace.raw()
        );
        output.push_str("dependencies");
        for dependency in &self.dependencies {
            let _ = write!(output, " b{}", dependency.raw());
        }
        output.push('\n');
        output.push_str("public");
        for symbol in &self.public_symbols {
            let _ = write!(output, " s{}", symbol.raw());
        }
        output.push('\n');
        for declaration in &self.declarations {
            dump_declaration(&mut output, declaration, arena);
        }
        for function in &self.functions {
            dump_function(&mut output, function, arena);
        }
        for method in &self.methods {
            dump_method(&mut output, method, arena);
        }
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirBubbleError {
    WrongOwner {
        symbol: SymbolId,
        expected: BubbleId,
        found: BubbleId,
    },
    DuplicateFunction(SymbolId),
    DuplicateDeclaration(SymbolId),
    DuplicateMethod(MethodId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirDeclaration {
    symbol: SymbolId,
    module: ModuleId,
    bubble: BubbleId,
    visibility: Visibility,
    name: String,
    kind: HirDeclarationKind,
    span: SourceSpan,
}

impl HirDeclaration {
    #[must_use]
    pub fn record(
        module: ModuleId,
        bubble: BubbleId,
        visibility: Visibility,
        name: impl Into<String>,
        definition: &RecordDefinition,
    ) -> Self {
        Self {
            symbol: definition.symbol(),
            module,
            bubble,
            visibility,
            name: name.into(),
            kind: HirDeclarationKind::Record(HirRecordDeclaration {
                type_id: definition.type_id(),
                fields: definition
                    .fields()
                    .iter()
                    .map(|field| HirRecordField {
                        field: field.field(),
                        name: field.name().to_owned(),
                        field_type: field.field_type(),
                        default: field.default().cloned(),
                        span: field.span(),
                    })
                    .collect(),
            }),
            span: definition.span(),
        }
    }

    #[must_use]
    pub fn tagged_union(
        module: ModuleId,
        bubble: BubbleId,
        visibility: Visibility,
        name: impl Into<String>,
        definition: &UnionDefinition,
    ) -> Self {
        Self {
            symbol: definition.symbol(),
            module,
            bubble,
            visibility,
            name: name.into(),
            kind: HirDeclarationKind::Union(HirUnionDeclaration {
                type_id: definition.type_id(),
                cases: definition
                    .cases()
                    .iter()
                    .map(|case| HirUnionCase {
                        case: case.case(),
                        name: case.name().to_owned(),
                        parameters: case
                            .parameters()
                            .iter()
                            .map(|(name, type_id, span)| HirNamedType {
                                name: name.clone(),
                                type_id: *type_id,
                                span: *span,
                            })
                            .collect(),
                        span: case.span(),
                    })
                    .collect(),
            }),
            span: definition.span(),
        }
    }

    #[must_use]
    pub fn class(
        module: ModuleId,
        bubble: BubbleId,
        visibility: Visibility,
        name: impl Into<String>,
        definition: &ClassDefinition,
    ) -> Self {
        Self {
            symbol: definition.symbol(),
            module,
            bubble,
            visibility,
            name: name.into(),
            kind: HirDeclarationKind::Class(HirClassDeclaration {
                class: definition.class(),
                type_id: definition.type_id(),
                is_open: definition.is_open(),
                fields: definition
                    .fields()
                    .iter()
                    .map(|field| HirClassField {
                        field: field.field(),
                        visibility: field.visibility(),
                        name: field.name().to_owned(),
                        field_type: field.field_type(),
                        default: field.default().cloned(),
                        span: field.span(),
                    })
                    .collect(),
                methods: definition
                    .methods()
                    .iter()
                    .map(|method| HirClassMethod {
                        method: method.method(),
                        visibility: method.visibility(),
                        name: method.name().to_owned(),
                        dispatch: method.dispatch(),
                        parameters: method
                            .parameters()
                            .iter()
                            .map(|(name, type_id, span)| HirNamedType {
                                name: name.clone(),
                                type_id: *type_id,
                                span: *span,
                            })
                            .collect(),
                        results: method.results().to_vec(),
                        span: method.span(),
                    })
                    .collect(),
            }),
            span: definition.span(),
        }
    }

    #[must_use]
    pub fn attribute(
        module: ModuleId,
        bubble: BubbleId,
        visibility: Visibility,
        name: impl Into<String>,
        definition: &AttributeDefinition,
    ) -> Self {
        Self {
            symbol: definition.symbol(),
            module,
            bubble,
            visibility,
            name: name.into(),
            kind: HirDeclarationKind::Attribute(HirAttributeDeclaration {
                attribute: definition.attribute(),
                parameters: definition
                    .parameters()
                    .iter()
                    .map(|parameter| HirAttributeParameter {
                        name: parameter.name().to_owned(),
                        parameter_type: parameter.parameter_type(),
                        default: parameter.default_value().cloned(),
                        span: parameter.span(),
                    })
                    .collect(),
            }),
            span: definition.span(),
        }
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn module(&self) -> ModuleId {
        self.module
    }

    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn kind(&self) -> &HirDeclarationKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HirDeclarationKind {
    Record(HirRecordDeclaration),
    Union(HirUnionDeclaration),
    Class(HirClassDeclaration),
    Attribute(HirAttributeDeclaration),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirRecordDeclaration {
    type_id: TypeId,
    fields: Vec<HirRecordField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirRecordField {
    field: FieldId,
    name: String,
    field_type: TypeId,
    default: Option<FieldDefault>,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirUnionDeclaration {
    type_id: TypeId,
    cases: Vec<HirUnionCase>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirUnionCase {
    case: UnionCaseId,
    name: String,
    parameters: Vec<HirNamedType>,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClassDeclaration {
    class: ClassId,
    type_id: TypeId,
    is_open: bool,
    fields: Vec<HirClassField>,
    methods: Vec<HirClassMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClassField {
    field: FieldId,
    visibility: Visibility,
    name: String,
    field_type: TypeId,
    default: Option<ClassFieldDefault>,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClassMethod {
    method: MethodId,
    visibility: Visibility,
    name: String,
    dispatch: ClassMethodDispatch,
    parameters: Vec<HirNamedType>,
    results: Vec<TypeId>,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirAttributeDeclaration {
    attribute: AttributeId,
    parameters: Vec<HirAttributeParameter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirAttributeParameter {
    name: String,
    parameter_type: TypeId,
    default: Option<AttributeConstant>,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirNamedType {
    name: String,
    type_id: TypeId,
    span: SourceSpan,
}

impl HirRecordDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn fields(&self) -> &[HirRecordField] {
        &self.fields
    }
}

impl HirRecordField {
    #[must_use]
    pub const fn field(&self) -> FieldId {
        self.field
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn field_type(&self) -> TypeId {
        self.field_type
    }

    #[must_use]
    pub const fn has_default(&self) -> bool {
        self.default.is_some()
    }

    #[must_use]
    pub const fn default(&self) -> Option<&FieldDefault> {
        self.default.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl HirUnionDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[HirUnionCase] {
        &self.cases
    }
}

impl HirUnionCase {
    #[must_use]
    pub const fn case(&self) -> UnionCaseId {
        self.case
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parameters(&self) -> &[HirNamedType] {
        &self.parameters
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl HirClassDeclaration {
    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.is_open
    }

    #[must_use]
    pub fn fields(&self) -> &[HirClassField] {
        &self.fields
    }

    #[must_use]
    pub fn methods(&self) -> &[HirClassMethod] {
        &self.methods
    }
}

impl HirClassField {
    #[must_use]
    pub const fn field(&self) -> FieldId {
        self.field
    }

    #[must_use]
    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn field_type(&self) -> TypeId {
        self.field_type
    }

    #[must_use]
    pub const fn default(&self) -> Option<&ClassFieldDefault> {
        self.default.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl HirClassMethod {
    #[must_use]
    pub const fn method(&self) -> MethodId {
        self.method
    }

    #[must_use]
    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn dispatch(&self) -> ClassMethodDispatch {
        self.dispatch
    }

    #[must_use]
    pub fn parameters(&self) -> &[HirNamedType] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl HirAttributeDeclaration {
    #[must_use]
    pub const fn attribute(&self) -> AttributeId {
        self.attribute
    }

    #[must_use]
    pub fn parameters(&self) -> &[HirAttributeParameter] {
        &self.parameters
    }
}

impl HirAttributeParameter {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn parameter_type(&self) -> TypeId {
        self.parameter_type
    }

    #[must_use]
    pub const fn default(&self) -> Option<&AttributeConstant> {
        self.default.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl HirNamedType {
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
pub struct HirMethod {
    method: MethodId,
    class: ClassId,
    definition: SymbolId,
    function: HirFunction,
}

impl HirMethod {
    #[must_use]
    pub const fn method(&self) -> MethodId {
        self.method
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn definition(&self) -> SymbolId {
        self.definition
    }

    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.function.bubble()
    }

    #[must_use]
    pub fn parameters(&self) -> &[HirParameter] {
        self.function.parameters()
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        self.function.results()
    }

    #[must_use]
    pub fn body(&self) -> &[HirStatement] {
        self.function.body()
    }

    #[must_use]
    pub const fn function(&self) -> &HirFunction {
        &self.function
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirFunction {
    function: FunctionId,
    symbol: SymbolId,
    module: ModuleId,
    bubble: BubbleId,
    visibility: Visibility,
    name: String,
    parameters: Vec<HirParameter>,
    results: Vec<TypeId>,
    body: Vec<HirStatement>,
    attributes: Vec<HirAttribute>,
}

impl HirFunction {
    #[must_use]
    pub const fn function(&self) -> FunctionId {
        self.function
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn module(&self) -> ModuleId {
        self.module
    }

    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parameters(&self) -> &[HirParameter] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub fn body(&self) -> &[HirStatement] {
        &self.body
    }

    #[must_use]
    pub fn attributes(&self) -> &[HirAttribute] {
        &self.attributes
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirAttribute {
    attribute: AttributeId,
    definition: SymbolId,
    arguments: Vec<HirAttributeArgument>,
    span: SourceSpan,
}

impl HirAttribute {
    #[must_use]
    pub const fn attribute(&self) -> AttributeId {
        self.attribute
    }

    #[must_use]
    pub const fn definition(&self) -> SymbolId {
        self.definition
    }

    #[must_use]
    pub fn arguments(&self) -> &[HirAttributeArgument] {
        &self.arguments
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirAttributeArgument {
    name: String,
    value: AttributeConstant,
    value_type: TypeId,
    origin: SourceSpan,
}

impl HirAttributeArgument {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn value(&self) -> &AttributeConstant {
        &self.value
    }

    #[must_use]
    pub const fn value_type(&self) -> TypeId {
        self.value_type
    }

    #[must_use]
    pub const fn origin(&self) -> SourceSpan {
        self.origin
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirParameter {
    parameter: ValueParameterId,
    name: String,
    type_id: TypeId,
    span: SourceSpan,
}

impl HirParameter {
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
pub struct HirStatement {
    kind: HirStatementKind,
    span: SourceSpan,
}

impl HirStatement {
    #[must_use]
    pub const fn kind(&self) -> &HirStatementKind {
        &self.kind
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HirStatementKind {
    Local {
        local: LocalId,
        name: String,
        local_type: TypeId,
        initializer: HirExpression,
    },
    Return {
        values: Vec<HirExpression>,
    },
    If {
        condition: HirExpression,
        then_body: Vec<HirStatement>,
        else_body: Vec<HirStatement>,
    },
    While {
        condition: HirExpression,
        body: Vec<HirStatement>,
    },
    FieldSet {
        base: HirExpression,
        field: FieldId,
        value: HirExpression,
    },
    Call(HirCall),
    Expression(HirExpression),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirCall {
    dispatch: HirCallDispatch,
    arguments: Vec<HirExpression>,
    span: SourceSpan,
}

impl HirCall {
    #[must_use]
    pub const fn dispatch(&self) -> &HirCallDispatch {
        &self.dispatch
    }

    #[must_use]
    pub fn arguments(&self) -> &[HirExpression] {
        &self.arguments
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirExpression {
    kind: HirExpressionKind,
    type_id: TypeId,
    span: SourceSpan,
}

impl HirExpression {
    #[must_use]
    pub const fn kind(&self) -> &HirExpressionKind {
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
pub enum HirExpressionKind {
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
    Boolean(bool),
    Nil,
    Local(LocalId),
    Parameter(ValueParameterId),
    Function(SymbolId),
    Field {
        base: Box<HirExpression>,
        field: FieldId,
    },
    ArrayGet {
        array: Box<HirExpression>,
        index: Box<HirExpression>,
    },
    Record {
        record: SymbolId,
        fields: Vec<HirFieldValue>,
    },
    ClassConstruct {
        class: ClassId,
        definition: SymbolId,
        fields: Vec<HirFieldValue>,
    },
    RecordUpdate {
        record: SymbolId,
        base: Box<HirExpression>,
        fields: Vec<HirFieldValue>,
    },
    Array(Vec<HirExpression>),
    Table(Vec<HirTableEntry>),
    UnionCase {
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<HirExpression>,
    },
    Tuple(Vec<HirExpression>),
    Unary {
        operator: TypedUnaryOperator,
        operand: Box<HirExpression>,
    },
    Binary {
        operator: TypedBinaryOperator,
        left: Box<HirExpression>,
        right: Box<HirExpression>,
    },
    Call {
        dispatch: HirCallDispatch,
        arguments: Vec<HirExpression>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirTableEntry {
    key: HirExpression,
    value: HirExpression,
    span: SourceSpan,
}

impl HirTableEntry {
    #[must_use]
    pub const fn key(&self) -> &HirExpression {
        &self.key
    }

    #[must_use]
    pub const fn value(&self) -> &HirExpression {
        &self.value
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirFieldValue {
    field: FieldId,
    value: HirExpression,
    span: SourceSpan,
}

impl HirFieldValue {
    #[must_use]
    pub const fn field(&self) -> FieldId {
        self.field
    }

    #[must_use]
    pub const fn value(&self) -> &HirExpression {
        &self.value
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HirCallDispatch {
    Direct { function: SymbolId },
    DirectMethod { method: MethodId },
    Indirect { callee: Box<HirExpression> },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HirFunctionContext {
    module: ModuleId,
    bubble: BubbleId,
    visibility: Visibility,
}

#[derive(Clone, Copy)]
pub struct HirKnownCallables<'a> {
    functions: &'a BTreeSet<SymbolId>,
    methods: &'a BTreeSet<MethodId>,
}

impl<'a> HirKnownCallables<'a> {
    #[must_use]
    pub const fn new(functions: &'a BTreeSet<SymbolId>, methods: &'a BTreeSet<MethodId>) -> Self {
        Self { functions, methods }
    }
}

impl HirFunctionContext {
    #[must_use]
    pub const fn new(module: ModuleId, bubble: BubbleId, visibility: Visibility) -> Self {
        Self {
            module,
            bubble,
            visibility,
        }
    }
}

/// Constructs HIR from an accepted typed body, then verifies the result.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_function(
    module: ModuleId,
    bubble: BubbleId,
    visibility: Visibility,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
) -> Result<HirFunction, Vec<HirVerificationError>> {
    build_hir_function_with_attributes(
        HirFunctionContext::new(module, bubble, visibility),
        signature,
        body,
        arena,
        known_functions,
        &[],
    )
}

/// Constructs and verifies HIR while retaining accepted compile-time attributes.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_function_with_attributes(
    context: HirFunctionContext,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    attributes: &[ResolvedAttribute],
) -> Result<HirFunction, Vec<HirVerificationError>> {
    build_hir_function_with_methods_and_attributes(
        context,
        signature,
        body,
        arena,
        known_functions,
        &BTreeSet::new(),
        attributes,
    )
}

/// Constructs and verifies a function that may directly call known class methods.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_function_with_methods_and_attributes(
    context: HirFunctionContext,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    known_methods: &BTreeSet<MethodId>,
    attributes: &[ResolvedAttribute],
) -> Result<HirFunction, Vec<HirVerificationError>> {
    let parameters: Option<Vec<_>> = signature
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            Some(HirParameter {
                parameter: ValueParameterId::from_raw(u32::try_from(index).ok()?),
                name: parameter.name().to_owned(),
                type_id: parameter.parameter_type().type_id()?,
                span: parameter.span(),
            })
        })
        .collect();
    let results: Option<Vec<_>> = signature
        .results()
        .iter()
        .map(pop_types::ResolvedType::type_id)
        .collect();
    let Some((parameters, results)) = parameters.zip(results) else {
        return Err(vec![HirVerificationError::MissingCanonicalType]);
    };
    let function = HirFunction {
        function: FunctionId::from_raw(signature.symbol().raw()),
        symbol: signature.symbol(),
        module: context.module,
        bubble: context.bubble,
        visibility: context.visibility,
        name: signature.name().to_owned(),
        parameters,
        results,
        body: body.statements().iter().map(lower_statement).collect(),
        attributes: attributes.iter().map(lower_attribute).collect(),
    };
    verify_hir_callable(&function, arena, known_functions, known_methods)?;
    Ok(function)
}

/// Constructs one verified native method body while retaining its `MethodId`.
///
/// # Errors
///
/// Returns all HIR invariant failures in deterministic traversal order.
pub fn build_hir_method(
    context: HirFunctionContext,
    definition: &ClassDefinition,
    method: &ClassMethodDefinition,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known: HirKnownCallables<'_>,
) -> Result<HirMethod, Vec<HirVerificationError>> {
    let function = build_hir_function_with_methods_and_attributes(
        context,
        signature,
        body,
        arena,
        known.functions,
        known.methods,
        &[],
    )?;
    Ok(HirMethod {
        method: method.method(),
        class: definition.class(),
        definition: definition.symbol(),
        function,
    })
}

fn lower_attribute(attribute: &ResolvedAttribute) -> HirAttribute {
    HirAttribute {
        attribute: attribute.attribute(),
        definition: attribute.definition(),
        arguments: attribute
            .arguments()
            .iter()
            .map(|argument| HirAttributeArgument {
                name: argument.name().to_owned(),
                value: argument.value().clone(),
                value_type: argument.value_type(),
                origin: argument.origin(),
            })
            .collect(),
        span: attribute.span(),
    }
}

fn lower_statement(statement: &TypedStatement) -> HirStatement {
    let kind = match statement.kind() {
        TypedStatementKind::Local {
            local,
            name,
            local_type,
            initializer,
        } => HirStatementKind::Local {
            local: *local,
            name: name.clone(),
            local_type: *local_type,
            initializer: lower_expression(initializer),
        },
        TypedStatementKind::Return { values } => HirStatementKind::Return {
            values: values.iter().map(lower_expression).collect(),
        },
        TypedStatementKind::If {
            condition,
            then_body,
            else_body,
        } => HirStatementKind::If {
            condition: lower_expression(condition),
            then_body: then_body.iter().map(lower_statement).collect(),
            else_body: else_body.iter().map(lower_statement).collect(),
        },
        TypedStatementKind::While { condition, body } => HirStatementKind::While {
            condition: lower_expression(condition),
            body: body.iter().map(lower_statement).collect(),
        },
        TypedStatementKind::FieldSet { base, field, value } => HirStatementKind::FieldSet {
            base: lower_expression(base),
            field: *field,
            value: lower_expression(value),
        },
        TypedStatementKind::Call(call) => HirStatementKind::Call(lower_call(call)),
        TypedStatementKind::Expression(expression) => {
            HirStatementKind::Expression(lower_expression(expression))
        }
    };
    HirStatement {
        kind,
        span: statement.span(),
    }
}

fn lower_call(call: &TypedCall) -> HirCall {
    let dispatch = match call.dispatch() {
        TypedCallDispatch::Direct { function } => HirCallDispatch::Direct {
            function: *function,
        },
        TypedCallDispatch::DirectMethod { method, receiver } => {
            return HirCall {
                dispatch: HirCallDispatch::DirectMethod { method: *method },
                arguments: receiver
                    .iter()
                    .map(|receiver| lower_expression(receiver))
                    .chain(call.arguments().iter().map(lower_expression))
                    .collect(),
                span: call.span(),
            };
        }
        TypedCallDispatch::Indirect { callee } => HirCallDispatch::Indirect {
            callee: Box::new(lower_expression(callee)),
        },
    };
    HirCall {
        dispatch,
        arguments: call.arguments().iter().map(lower_expression).collect(),
        span: call.span(),
    }
}

fn lower_expression(expression: &TypedExpression) -> HirExpression {
    let kind = match expression.kind() {
        TypedExpressionKind::Integer(value) => HirExpressionKind::Integer(*value),
        TypedExpressionKind::Float(value) => HirExpressionKind::Float(*value),
        TypedExpressionKind::String(value) => HirExpressionKind::String(value.clone()),
        TypedExpressionKind::Boolean(value) => HirExpressionKind::Boolean(*value),
        TypedExpressionKind::Nil => HirExpressionKind::Nil,
        TypedExpressionKind::Local(local) => HirExpressionKind::Local(*local),
        TypedExpressionKind::Parameter(parameter) => HirExpressionKind::Parameter(*parameter),
        TypedExpressionKind::Function(function) => HirExpressionKind::Function(*function),
        TypedExpressionKind::Field { base, field } => HirExpressionKind::Field {
            base: Box::new(lower_expression(base)),
            field: *field,
        },
        TypedExpressionKind::ArrayGet { array, index } => HirExpressionKind::ArrayGet {
            array: Box::new(lower_expression(array)),
            index: Box::new(lower_expression(index)),
        },
        TypedExpressionKind::Record { record, fields } => HirExpressionKind::Record {
            record: *record,
            fields: fields.iter().map(lower_field_value).collect(),
        },
        TypedExpressionKind::ClassConstruct {
            class,
            definition,
            fields,
        } => HirExpressionKind::ClassConstruct {
            class: *class,
            definition: *definition,
            fields: fields.iter().map(lower_field_value).collect(),
        },
        TypedExpressionKind::RecordUpdate {
            record,
            base,
            fields,
        } => HirExpressionKind::RecordUpdate {
            record: *record,
            base: Box::new(lower_expression(base)),
            fields: fields.iter().map(lower_field_value).collect(),
        },
        TypedExpressionKind::Array(elements) => {
            HirExpressionKind::Array(elements.iter().map(lower_expression).collect())
        }
        TypedExpressionKind::Table(entries) => {
            HirExpressionKind::Table(entries.iter().map(lower_table_entry).collect())
        }
        TypedExpressionKind::UnionCase {
            union,
            case,
            arguments,
        } => HirExpressionKind::UnionCase {
            union: *union,
            case: *case,
            arguments: arguments.iter().map(lower_expression).collect(),
        },
        TypedExpressionKind::Tuple(elements) => {
            HirExpressionKind::Tuple(elements.iter().map(lower_expression).collect())
        }
        TypedExpressionKind::Unary { operator, operand } => HirExpressionKind::Unary {
            operator: *operator,
            operand: Box::new(lower_expression(operand)),
        },
        TypedExpressionKind::Binary {
            operator,
            left,
            right,
        } => HirExpressionKind::Binary {
            operator: *operator,
            left: Box::new(lower_expression(left)),
            right: Box::new(lower_expression(right)),
        },
        call @ (TypedExpressionKind::DirectCall { .. }
        | TypedExpressionKind::IndirectCall { .. }
        | TypedExpressionKind::DirectMethodCall { .. }) => lower_call_expression(call),
    };
    HirExpression {
        kind,
        type_id: expression.type_id(),
        span: expression.span(),
    }
}

fn lower_call_expression(call: &TypedExpressionKind) -> HirExpressionKind {
    match call {
        TypedExpressionKind::DirectCall {
            function,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Direct {
                function: *function,
            },
            arguments: arguments.iter().map(lower_expression).collect(),
        },
        TypedExpressionKind::IndirectCall { callee, arguments } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Indirect {
                callee: Box::new(lower_expression(callee)),
            },
            arguments: arguments.iter().map(lower_expression).collect(),
        },
        TypedExpressionKind::DirectMethodCall {
            method,
            receiver,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::DirectMethod { method: *method },
            arguments: receiver
                .iter()
                .map(|receiver| lower_expression(receiver))
                .chain(arguments.iter().map(lower_expression))
                .collect(),
        },
        _ => unreachable!("call lowering accepts only typed call expressions"),
    }
}

fn lower_field_value(field: &TypedFieldValue) -> HirFieldValue {
    HirFieldValue {
        field: field.field(),
        value: lower_expression(field.value()),
        span: field.span(),
    }
}

fn lower_table_entry(entry: &TypedTableEntry) -> HirTableEntry {
    HirTableEntry {
        key: lower_expression(entry.key()),
        value: lower_expression(entry.value()),
        span: entry.span(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirVerificationError {
    MissingCanonicalType,
    InvalidType {
        type_id: TypeId,
        span: SourceSpan,
    },
    DuplicateLocal(LocalId),
    DuplicateField(FieldId),
    UnknownLocal {
        local: LocalId,
        span: SourceSpan,
    },
    UnknownParameter {
        parameter: ValueParameterId,
        span: SourceSpan,
    },
    UnknownFunction {
        function: SymbolId,
        span: SourceSpan,
    },
    UnknownMethod {
        method: MethodId,
        span: SourceSpan,
    },
    InvalidCollectionType {
        type_id: TypeId,
        span: SourceSpan,
    },
    InvalidCallableType {
        type_id: TypeId,
        span: SourceSpan,
    },
    ExpressionTypeMismatch {
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidUnaryOperator {
        operator: TypedUnaryOperator,
        operand: TypeId,
        result: TypeId,
        span: SourceSpan,
    },
    InvalidBinaryOperator {
        operator: TypedBinaryOperator,
        left: TypeId,
        right: TypeId,
        result: TypeId,
        span: SourceSpan,
    },
    WrongReturnArity {
        expected: usize,
        found: usize,
        span: SourceSpan,
    },
    InvalidConditionType {
        found: TypeId,
        span: SourceSpan,
    },
    DuplicateSymbol(SymbolId),
    DuplicateClass(ClassId),
    DuplicateDeclaredField(FieldId),
    DuplicateUnionCase {
        union: SymbolId,
        case: UnionCaseId,
    },
    InvalidDeclarationType {
        symbol: SymbolId,
        type_id: TypeId,
        span: SourceSpan,
    },
    UnknownRecord {
        record: SymbolId,
        span: SourceSpan,
    },
    UnknownClass {
        class: ClassId,
        span: SourceSpan,
    },
    WrongClassDefinition {
        class: ClassId,
        expected: SymbolId,
        found: SymbolId,
        span: SourceSpan,
    },
    UnknownField {
        field: FieldId,
        span: SourceSpan,
    },
    WrongFieldOwner {
        field: FieldId,
        found: TypeId,
        span: SourceSpan,
    },
    ImmutableFieldSet {
        field: FieldId,
        span: SourceSpan,
    },
    MissingDeclaredField {
        field: FieldId,
        span: SourceSpan,
    },
    UnknownUnion {
        union: SymbolId,
        span: SourceSpan,
    },
    UnknownUnionCase {
        union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    UnionCaseArgumentTypeMismatch {
        union: SymbolId,
        case: UnionCaseId,
        index: usize,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidCallSignature {
        expected_arguments: usize,
        found_arguments: usize,
        expected_results: usize,
        found_results: usize,
        span: SourceSpan,
    },
    CallArgumentTypeMismatch {
        index: usize,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    CallResultTypeMismatch {
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidFunctionReferenceType {
        function: SymbolId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidMethodSignature {
        method: MethodId,
        span: SourceSpan,
    },
    MissingMethodBody {
        method: MethodId,
        span: SourceSpan,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct HirCallableSignature {
    parameters: Vec<TypeId>,
    results: Vec<TypeId>,
}

impl HirCallableSignature {
    fn from_function(function: &HirFunction) -> Self {
        Self {
            parameters: function
                .parameters()
                .iter()
                .map(HirParameter::type_id)
                .collect(),
            results: function.results().to_vec(),
        }
    }
}

#[derive(Clone)]
struct HirAggregateSchema {
    type_id: TypeId,
    fields: BTreeMap<FieldId, TypeId>,
}

#[derive(Clone)]
struct HirUnionSchema {
    type_id: TypeId,
    cases: BTreeMap<UnionCaseId, Vec<TypeId>>,
}

#[derive(Clone)]
struct HirClassSchema {
    definition: SymbolId,
    type_id: TypeId,
    fields: BTreeMap<FieldId, TypeId>,
}

#[derive(Clone)]
struct HirDeclaredField {
    owners: BTreeSet<TypeId>,
    field_type: TypeId,
    mutable: bool,
}

struct HirDeclaredMethod {
    class: ClassId,
    definition: SymbolId,
    signature: HirCallableSignature,
    span: SourceSpan,
}

struct HirSchema {
    functions: BTreeMap<SymbolId, HirCallableSignature>,
    methods: BTreeMap<MethodId, HirCallableSignature>,
    declared_methods: BTreeMap<MethodId, HirDeclaredMethod>,
    records: BTreeMap<SymbolId, HirAggregateSchema>,
    unions: BTreeMap<SymbolId, HirUnionSchema>,
    classes: BTreeMap<ClassId, HirClassSchema>,
    fields: BTreeMap<FieldId, HirDeclaredField>,
}

impl HirSchema {
    fn collect(
        bubble: &HirBubble,
        arena: &TypeArena,
        errors: &mut Vec<HirVerificationError>,
    ) -> Self {
        let mut schema = Self {
            functions: BTreeMap::new(),
            methods: BTreeMap::new(),
            declared_methods: BTreeMap::new(),
            records: BTreeMap::new(),
            unions: BTreeMap::new(),
            classes: BTreeMap::new(),
            fields: BTreeMap::new(),
        };
        let mut symbols = BTreeSet::new();
        for declaration in bubble.declarations() {
            if !symbols.insert(declaration.symbol()) {
                errors.push(HirVerificationError::DuplicateSymbol(declaration.symbol()));
            }
            schema.collect_declaration(declaration, arena, errors);
        }
        for function in bubble.functions() {
            if !symbols.insert(function.symbol()) {
                errors.push(HirVerificationError::DuplicateSymbol(function.symbol()));
            }
            schema.functions.insert(
                function.symbol(),
                HirCallableSignature::from_function(function),
            );
        }
        schema.collect_method_bodies(bubble.methods(), errors);
        schema
    }

    fn collect_declaration(
        &mut self,
        declaration: &HirDeclaration,
        arena: &TypeArena,
        errors: &mut Vec<HirVerificationError>,
    ) {
        match declaration.kind() {
            HirDeclarationKind::Record(record) => {
                for field in &record.fields {
                    verify_schema_type(arena, field.field_type, field.span, errors);
                }
                let semantic_fields: Vec<_> = record
                    .fields
                    .iter()
                    .map(|field| (field.name.clone(), field.field_type))
                    .collect();
                if arena.get(record.type_id) != Some(&SemanticType::Record(semantic_fields)) {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: record.type_id,
                        span: declaration.span(),
                    });
                }
                let fields = self.collect_fields(record.type_id, &record.fields, false, errors);
                self.records.insert(
                    declaration.symbol(),
                    HirAggregateSchema {
                        type_id: record.type_id,
                        fields,
                    },
                );
            }
            HirDeclarationKind::Union(union) => {
                if arena.get(union.type_id)
                    != Some(&SemanticType::TaggedUnion {
                        definition: declaration.symbol(),
                    })
                {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: union.type_id,
                        span: declaration.span(),
                    });
                }
                let mut cases = BTreeMap::new();
                for case in &union.cases {
                    for parameter in &case.parameters {
                        verify_schema_type(arena, parameter.type_id, parameter.span, errors);
                    }
                    let parameters = case.parameters.iter().map(HirNamedType::type_id).collect();
                    if cases.insert(case.case, parameters).is_some() {
                        errors.push(HirVerificationError::DuplicateUnionCase {
                            union: declaration.symbol(),
                            case: case.case,
                        });
                    }
                }
                self.unions.insert(
                    declaration.symbol(),
                    HirUnionSchema {
                        type_id: union.type_id,
                        cases,
                    },
                );
            }
            HirDeclarationKind::Class(class) => {
                for field in &class.fields {
                    verify_schema_type(arena, field.field_type, field.span, errors);
                }
                for method in &class.methods {
                    for parameter in &method.parameters {
                        verify_schema_type(arena, parameter.type_id, parameter.span, errors);
                    }
                    for result in &method.results {
                        verify_schema_type(arena, *result, method.span, errors);
                    }
                }
                if arena.get(class.type_id)
                    != Some(&SemanticType::Class {
                        class: class.class,
                        arguments: Vec::new(),
                    })
                {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: class.type_id,
                        span: declaration.span(),
                    });
                }
                let fields = self.collect_class_fields(class, errors);
                if self
                    .classes
                    .insert(
                        class.class,
                        HirClassSchema {
                            definition: declaration.symbol(),
                            type_id: class.type_id,
                            fields,
                        },
                    )
                    .is_some()
                {
                    errors.push(HirVerificationError::DuplicateClass(class.class));
                }
                self.collect_declared_methods(declaration.symbol(), class, errors);
            }
            HirDeclarationKind::Attribute(attribute) => {
                for parameter in &attribute.parameters {
                    if !arena.is_valid_hir_type(parameter.parameter_type) {
                        errors.push(HirVerificationError::InvalidType {
                            type_id: parameter.parameter_type,
                            span: parameter.span,
                        });
                    }
                }
            }
        }
    }

    fn collect_fields(
        &mut self,
        owner: TypeId,
        fields: &[HirRecordField],
        mutable: bool,
        errors: &mut Vec<HirVerificationError>,
    ) -> BTreeMap<FieldId, TypeId> {
        let mut declared = BTreeMap::new();
        for field in fields {
            self.collect_field(owner, field.field, field.field_type, mutable, errors);
            if declared.insert(field.field, field.field_type).is_some() {
                errors.push(HirVerificationError::DuplicateDeclaredField(field.field));
            }
        }
        declared
    }

    fn collect_class_fields(
        &mut self,
        class: &HirClassDeclaration,
        errors: &mut Vec<HirVerificationError>,
    ) -> BTreeMap<FieldId, TypeId> {
        let mut declared = BTreeMap::new();
        for field in &class.fields {
            self.collect_field(class.type_id, field.field, field.field_type, true, errors);
            if declared.insert(field.field, field.field_type).is_some() {
                errors.push(HirVerificationError::DuplicateDeclaredField(field.field));
            }
        }
        declared
    }

    fn collect_field(
        &mut self,
        owner: TypeId,
        field: FieldId,
        field_type: TypeId,
        mutable: bool,
        errors: &mut Vec<HirVerificationError>,
    ) {
        if let Some(existing) = self.fields.get_mut(&field) {
            if existing.field_type != field_type || existing.mutable != mutable {
                errors.push(HirVerificationError::DuplicateDeclaredField(field));
            } else {
                existing.owners.insert(owner);
            }
            return;
        }
        self.fields.insert(
            field,
            HirDeclaredField {
                owners: BTreeSet::from([owner]),
                field_type,
                mutable,
            },
        );
    }

    fn collect_declared_methods(
        &mut self,
        definition: SymbolId,
        class: &HirClassDeclaration,
        errors: &mut Vec<HirVerificationError>,
    ) {
        for method in &class.methods {
            let mut parameters = Vec::new();
            if method.dispatch == ClassMethodDispatch::Receiver {
                parameters.push(class.type_id);
            }
            parameters.extend(method.parameters.iter().map(HirNamedType::type_id));
            let declared = HirDeclaredMethod {
                class: class.class,
                definition,
                signature: HirCallableSignature {
                    parameters,
                    results: method.results.clone(),
                },
                span: method.span,
            };
            if self
                .methods
                .insert(method.method, declared.signature.clone())
                .is_some()
            {
                errors.push(HirVerificationError::UnknownMethod {
                    method: method.method,
                    span: method.span,
                });
            }
            self.declared_methods.insert(method.method, declared);
        }
    }

    fn collect_method_bodies(
        &mut self,
        methods: &[HirMethod],
        errors: &mut Vec<HirVerificationError>,
    ) {
        let mut bodies = BTreeSet::new();
        for method in methods {
            bodies.insert(method.method());
            let span = method_span(method);
            let Some(declared) = self.declared_methods.get(&method.method()) else {
                errors.push(HirVerificationError::UnknownMethod {
                    method: method.method(),
                    span,
                });
                continue;
            };
            if method.class() != declared.class {
                errors.push(HirVerificationError::UnknownClass {
                    class: method.class(),
                    span,
                });
            }
            if method.definition() != declared.definition {
                errors.push(HirVerificationError::WrongClassDefinition {
                    class: method.class(),
                    expected: declared.definition,
                    found: method.definition(),
                    span,
                });
            }
            if HirCallableSignature::from_function(method.function()) != declared.signature {
                errors.push(HirVerificationError::InvalidMethodSignature {
                    method: method.method(),
                    span,
                });
            }
        }
        for (method, declared) in &self.declared_methods {
            if !bodies.contains(method) {
                errors.push(HirVerificationError::MissingMethodBody {
                    method: *method,
                    span: declared.span,
                });
            }
        }
    }
}

fn verify_schema_type(
    arena: &TypeArena,
    type_id: TypeId,
    span: SourceSpan,
    errors: &mut Vec<HirVerificationError>,
) {
    if !arena.is_valid_hir_type(type_id) {
        errors.push(HirVerificationError::InvalidType { type_id, span });
    }
}

/// Verifies a complete backend-neutral HIR Bubble, including declaration and
/// member schemas plus exact direct, method, and indirect callable signatures.
///
/// # Errors
///
/// Returns invariant violations in deterministic declaration and body order.
pub fn verify_hir_bubble(
    bubble: &HirBubble,
    arena: &TypeArena,
) -> Result<(), Vec<HirVerificationError>> {
    let mut errors = Vec::new();
    let schema = HirSchema::collect(bubble, arena, &mut errors);
    let known_functions: BTreeSet<_> = schema.functions.keys().copied().collect();
    let known_methods: BTreeSet<_> = schema.methods.keys().copied().collect();
    for function in bubble.functions() {
        if let Err(mut function_errors) = verify_hir_callable_with_schema(
            function,
            arena,
            &known_functions,
            &known_methods,
            Some(&schema),
        ) {
            errors.append(&mut function_errors);
        }
    }
    for method in bubble.methods() {
        if let Err(mut method_errors) = verify_hir_callable_with_schema(
            method.function(),
            arena,
            &known_functions,
            &known_methods,
            Some(&schema),
        ) {
            errors.append(&mut method_errors);
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

/// Verifies backend-neutral HIR typing, local identity, and dispatch targets.
///
/// # Errors
///
/// Returns invariant violations in deterministic traversal order.
pub fn verify_hir_function(
    function: &HirFunction,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
) -> Result<(), Vec<HirVerificationError>> {
    verify_hir_callable(function, arena, known_functions, &BTreeSet::new())
}

fn verify_hir_callable(
    function: &HirFunction,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    known_methods: &BTreeSet<MethodId>,
) -> Result<(), Vec<HirVerificationError>> {
    verify_hir_callable_with_schema(function, arena, known_functions, known_methods, None)
}

fn verify_hir_callable_with_schema(
    function: &HirFunction,
    arena: &TypeArena,
    known_functions: &BTreeSet<SymbolId>,
    known_methods: &BTreeSet<MethodId>,
    schema: Option<&HirSchema>,
) -> Result<(), Vec<HirVerificationError>> {
    let mut verifier = Verifier {
        arena,
        known_functions,
        known_methods,
        schema,
        parameter_types: function
            .parameters
            .iter()
            .map(|parameter| parameter.type_id)
            .collect(),
        results: function.results.clone(),
        local_types: BTreeMap::new(),
        errors: Vec::new(),
    };
    for parameter in &function.parameters {
        verifier.verify_type(parameter.type_id, parameter.span);
    }
    for result in &function.results {
        if !arena.is_valid_hir_type(*result) {
            verifier.errors.push(HirVerificationError::InvalidType {
                type_id: *result,
                span: function
                    .parameters
                    .first()
                    .map_or_else(empty_span, |parameter| parameter.span),
            });
        }
    }
    for attribute in &function.attributes {
        for argument in attribute.arguments() {
            verifier.verify_type(argument.value_type(), argument.origin());
        }
    }
    verifier.verify_statements(&function.body, &BTreeSet::new());
    if verifier.errors.is_empty() {
        Ok(())
    } else {
        Err(verifier.errors)
    }
}

struct Verifier<'arena> {
    arena: &'arena TypeArena,
    known_functions: &'arena BTreeSet<SymbolId>,
    known_methods: &'arena BTreeSet<MethodId>,
    schema: Option<&'arena HirSchema>,
    parameter_types: Vec<TypeId>,
    results: Vec<TypeId>,
    local_types: BTreeMap<LocalId, TypeId>,
    errors: Vec<HirVerificationError>,
}

impl Verifier<'_> {
    fn verify_statements(&mut self, statements: &[HirStatement], visible: &BTreeSet<LocalId>) {
        let mut visible = visible.clone();
        for statement in statements {
            match statement.kind() {
                HirStatementKind::Local {
                    local,
                    local_type,
                    initializer,
                    ..
                } => {
                    self.verify_type(*local_type, statement.span());
                    self.verify_expression(initializer, &visible);
                    self.verify_expression_type(*local_type, initializer);
                    if self.local_types.insert(*local, *local_type).is_some() {
                        self.errors
                            .push(HirVerificationError::DuplicateLocal(*local));
                    }
                    visible.insert(*local);
                }
                HirStatementKind::Return { values } => {
                    for value in values {
                        self.verify_expression(value, &visible);
                    }
                    self.verify_return(values, statement.span());
                }
                HirStatementKind::If {
                    condition,
                    then_body,
                    else_body,
                } => {
                    self.verify_expression(condition, &visible);
                    self.verify_condition(condition);
                    self.verify_statements(then_body, &visible);
                    self.verify_statements(else_body, &visible);
                }
                HirStatementKind::While { condition, body } => {
                    self.verify_expression(condition, &visible);
                    self.verify_condition(condition);
                    self.verify_statements(body, &visible);
                }
                HirStatementKind::FieldSet { base, field, value } => {
                    self.verify_expression(base, &visible);
                    self.verify_expression(value, &visible);
                    self.verify_field_set(*field, base, value, statement.span());
                }
                HirStatementKind::Call(call) => {
                    self.verify_call(
                        call.dispatch(),
                        call.arguments(),
                        None,
                        call.span(),
                        &visible,
                    );
                }
                HirStatementKind::Expression(expression) => {
                    self.verify_expression(expression, &visible);
                }
            }
        }
    }

    fn verify_expression(&mut self, expression: &HirExpression, visible: &BTreeSet<LocalId>) {
        self.verify_type(expression.type_id(), expression.span());
        match expression.kind() {
            HirExpressionKind::Local(local) => {
                if !visible.contains(local) {
                    self.errors.push(HirVerificationError::UnknownLocal {
                        local: *local,
                        span: expression.span(),
                    });
                } else if let Some(expected) = self.local_types.get(local).copied() {
                    self.verify_expression_type(expected, expression);
                }
            }
            HirExpressionKind::Parameter(parameter) => {
                let parameter_type = usize::try_from(parameter.raw())
                    .ok()
                    .and_then(|raw| self.parameter_types.get(raw))
                    .copied();
                if let Some(expected) = parameter_type {
                    self.verify_expression_type(expected, expression);
                } else {
                    self.errors.push(HirVerificationError::UnknownParameter {
                        parameter: *parameter,
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::Function(function) => {
                self.verify_function(*function, expression.span());
                self.verify_function_reference(*function, expression);
            }
            HirExpressionKind::Field { .. }
            | HirExpressionKind::Record { .. }
            | HirExpressionKind::ClassConstruct { .. }
            | HirExpressionKind::RecordUpdate { .. }
            | HirExpressionKind::UnionCase { .. } => {
                self.verify_schema_expression(expression, visible);
            }
            HirExpressionKind::ArrayGet { array, index } => {
                self.verify_array_get(array, index, visible);
            }
            HirExpressionKind::Array(elements) => {
                self.verify_array(expression, elements, visible);
            }
            HirExpressionKind::Table(entries) => {
                self.verify_table(expression, entries, visible);
            }
            HirExpressionKind::Tuple(elements) => {
                for element in elements {
                    self.verify_expression(element, visible);
                }
                self.verify_tuple(expression, elements);
            }
            HirExpressionKind::Unary { operator, operand } => {
                self.verify_expression(operand, visible);
                self.verify_unary_operator(expression, *operator, operand);
            }
            HirExpressionKind::Binary {
                operator,
                left,
                right,
            } => {
                self.verify_expression(left, visible);
                self.verify_expression(right, visible);
                self.verify_binary_operator(expression, *operator, left, right);
            }
            HirExpressionKind::Call {
                dispatch,
                arguments,
            } => {
                self.verify_call(
                    dispatch,
                    arguments,
                    Some(expression.type_id()),
                    expression.span(),
                    visible,
                );
            }
            HirExpressionKind::Integer(_) | HirExpressionKind::Float(_) => {
                self.verify_numeric_literal(expression);
            }
            HirExpressionKind::String(_)
            | HirExpressionKind::Boolean(_)
            | HirExpressionKind::Nil => self.verify_primitive_literal(expression),
        }
    }

    fn verify_schema_expression(
        &mut self,
        expression: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        match expression.kind() {
            HirExpressionKind::Field { base, field } => {
                self.verify_expression(base, visible);
                self.verify_field_get(*field, base, expression);
            }
            HirExpressionKind::Record { record, fields } => {
                self.verify_fields(fields, visible);
                self.verify_record(*record, fields, true, expression);
            }
            HirExpressionKind::ClassConstruct {
                class,
                definition,
                fields,
            } => {
                self.verify_fields(fields, visible);
                self.verify_class(*class, *definition, fields, expression);
            }
            HirExpressionKind::RecordUpdate {
                record,
                base,
                fields,
            } => {
                self.verify_expression(base, visible);
                self.verify_fields(fields, visible);
                self.verify_record_update(*record, base, fields, expression);
            }
            HirExpressionKind::UnionCase {
                union,
                case,
                arguments,
            } => {
                for argument in arguments {
                    self.verify_expression(argument, visible);
                }
                self.verify_union_case(*union, *case, arguments, expression);
            }
            _ => unreachable!("schema expression verifier accepts only schema-owned expressions"),
        }
    }

    fn verify_return(&mut self, values: &[HirExpression], span: SourceSpan) {
        if values.len() != self.results.len() {
            self.errors.push(HirVerificationError::WrongReturnArity {
                expected: self.results.len(),
                found: values.len(),
                span,
            });
            return;
        }
        for (value, expected) in values.iter().zip(self.results.clone()) {
            self.verify_expression_type(expected, value);
        }
    }

    fn verify_condition(&mut self, condition: &HirExpression) {
        if self.arena.source_type("Boolean") != Some(condition.type_id()) {
            self.errors
                .push(HirVerificationError::InvalidConditionType {
                    found: condition.type_id(),
                    span: condition.span(),
                });
        }
    }

    fn verify_tuple(&mut self, expression: &HirExpression, elements: &[HirExpression]) {
        let Some(SemanticType::Tuple(element_types)) =
            self.arena.get(expression.type_id()).cloned()
        else {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
            return;
        };
        if element_types.len() != elements.len() {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
            return;
        }
        for (element, expected) in elements.iter().zip(element_types) {
            self.verify_expression_type(expected, element);
        }
    }

    fn verify_primitive_literal(&mut self, expression: &HirExpression) {
        let valid = matches!(
            (expression.kind(), self.arena.get(expression.type_id())),
            (
                HirExpressionKind::String(_),
                Some(SemanticType::Primitive(PrimitiveType::String))
            ) | (
                HirExpressionKind::Boolean(_),
                Some(SemanticType::Primitive(PrimitiveType::Boolean))
            ) | (
                HirExpressionKind::Nil,
                Some(SemanticType::Primitive(PrimitiveType::Nil))
            )
        );
        if !valid {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
        }
    }

    fn verify_unary_operator(
        &mut self,
        expression: &HirExpression,
        operator: TypedUnaryOperator,
        operand: &HirExpression,
    ) {
        if !valid_hir_unary_operator(
            operator,
            operand.type_id(),
            expression.type_id(),
            self.arena,
        ) {
            self.errors
                .push(HirVerificationError::InvalidUnaryOperator {
                    operator,
                    operand: operand.type_id(),
                    result: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_binary_operator(
        &mut self,
        expression: &HirExpression,
        operator: TypedBinaryOperator,
        left: &HirExpression,
        right: &HirExpression,
    ) {
        if !valid_hir_binary_operator(
            operator,
            left.type_id(),
            right.type_id(),
            expression.type_id(),
            self.arena,
        ) {
            self.errors
                .push(HirVerificationError::InvalidBinaryOperator {
                    operator,
                    left: left.type_id(),
                    right: right.type_id(),
                    result: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_numeric_literal(&mut self, expression: &HirExpression) {
        let matches = match expression.kind() {
            HirExpressionKind::Integer(value) => matches!(
                self.arena.get(expression.type_id()),
                Some(SemanticType::Primitive(PrimitiveType::Integer(kind)))
                    if *kind == value.kind()
            ),
            HirExpressionKind::Float(value) => matches!(
                (value.kind(), self.arena.get(expression.type_id())),
                (
                    pop_types::FloatKind::Float32,
                    Some(SemanticType::Primitive(PrimitiveType::Float32))
                ) | (
                    pop_types::FloatKind::Float64,
                    Some(SemanticType::Primitive(PrimitiveType::Float64))
                )
            ),
            _ => unreachable!("numeric literal verifier accepts only numeric literals"),
        };
        if !matches {
            self.errors.push(HirVerificationError::InvalidType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
        }
    }

    fn verify_call(
        &mut self,
        dispatch: &HirCallDispatch,
        arguments: &[HirExpression],
        result: Option<TypeId>,
        span: SourceSpan,
        visible: &BTreeSet<LocalId>,
    ) {
        let signature = match dispatch {
            HirCallDispatch::Direct { function } => {
                self.verify_function(*function, span);
                self.schema
                    .and_then(|schema| schema.functions.get(function))
                    .cloned()
            }
            HirCallDispatch::DirectMethod { method } => {
                if !self.known_methods.contains(method) {
                    self.errors.push(HirVerificationError::UnknownMethod {
                        method: *method,
                        span,
                    });
                }
                self.schema
                    .and_then(|schema| schema.methods.get(method))
                    .cloned()
            }
            HirCallDispatch::Indirect { callee } => {
                self.verify_expression(callee, visible);
                if let Some(SemanticType::Function {
                    parameters,
                    results,
                }) = self.arena.get(callee.type_id()).cloned()
                {
                    Some(HirCallableSignature {
                        parameters,
                        results,
                    })
                } else {
                    self.errors.push(HirVerificationError::InvalidCallableType {
                        type_id: callee.type_id(),
                        span: callee.span(),
                    });
                    None
                }
            }
        };
        for argument in arguments {
            self.verify_expression(argument, visible);
        }
        if self.schema.is_some()
            && let Some(signature) = signature
        {
            self.verify_call_signature(&signature, arguments, result, span);
        }
    }

    fn verify_call_signature(
        &mut self,
        signature: &HirCallableSignature,
        arguments: &[HirExpression],
        result: Option<TypeId>,
        span: SourceSpan,
    ) {
        for (index, (argument, expected)) in arguments.iter().zip(&signature.parameters).enumerate()
        {
            if argument.type_id() != *expected {
                self.errors
                    .push(HirVerificationError::CallArgumentTypeMismatch {
                        index,
                        expected: *expected,
                        found: argument.type_id(),
                        span: argument.span(),
                    });
            }
        }
        let found_results = usize::from(result.is_some());
        if arguments.len() != signature.parameters.len() || signature.results.len() != found_results
        {
            self.errors
                .push(HirVerificationError::InvalidCallSignature {
                    expected_arguments: signature.parameters.len(),
                    found_arguments: arguments.len(),
                    expected_results: signature.results.len(),
                    found_results,
                    span,
                });
        }
        if let ([expected], Some(found)) = (signature.results.as_slice(), result)
            && *expected != found
        {
            self.errors
                .push(HirVerificationError::CallResultTypeMismatch {
                    expected: *expected,
                    found,
                    span,
                });
        }
    }

    fn verify_function_reference(&mut self, function: SymbolId, expression: &HirExpression) {
        let Some(signature) = self
            .schema
            .and_then(|schema| schema.functions.get(&function))
            .cloned()
        else {
            return;
        };
        let expected = SemanticType::Function {
            parameters: signature.parameters,
            results: signature.results,
        };
        if self.arena.get(expression.type_id()) != Some(&expected) {
            self.errors
                .push(HirVerificationError::InvalidFunctionReferenceType {
                    function,
                    found: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_record(
        &mut self,
        record: SymbolId,
        fields: &[HirFieldValue],
        require_complete: bool,
        expression: &HirExpression,
    ) {
        let Some(record_schema) = self
            .schema
            .and_then(|schema| schema.records.get(&record))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownRecord {
                    record,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_expression_type(record_schema.type_id, expression);
        self.verify_declared_fields(
            &record_schema.fields,
            fields,
            require_complete,
            expression.span(),
        );
    }

    fn verify_record_update(
        &mut self,
        record: SymbolId,
        base: &HirExpression,
        fields: &[HirFieldValue],
        expression: &HirExpression,
    ) {
        let Some(record_schema) = self
            .schema
            .and_then(|schema| schema.records.get(&record))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownRecord {
                    record,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_expression_type(record_schema.type_id, base);
        self.verify_expression_type(record_schema.type_id, expression);
        self.verify_declared_fields(&record_schema.fields, fields, false, expression.span());
    }

    fn verify_class(
        &mut self,
        class: ClassId,
        definition: SymbolId,
        fields: &[HirFieldValue],
        expression: &HirExpression,
    ) {
        let Some(class_schema) = self
            .schema
            .and_then(|schema| schema.classes.get(&class))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownClass {
                    class,
                    span: expression.span(),
                });
            }
            return;
        };
        if definition != class_schema.definition {
            self.errors
                .push(HirVerificationError::WrongClassDefinition {
                    class,
                    expected: class_schema.definition,
                    found: definition,
                    span: expression.span(),
                });
        }
        self.verify_expression_type(class_schema.type_id, expression);
        self.verify_declared_fields(&class_schema.fields, fields, true, expression.span());
    }

    fn verify_declared_fields(
        &mut self,
        declared: &BTreeMap<FieldId, TypeId>,
        fields: &[HirFieldValue],
        require_complete: bool,
        span: SourceSpan,
    ) {
        let mut seen = BTreeSet::new();
        for field in fields {
            seen.insert(field.field());
            let Some(expected) = declared.get(&field.field()).copied() else {
                self.errors.push(HirVerificationError::UnknownField {
                    field: field.field(),
                    span: field.span(),
                });
                continue;
            };
            self.verify_expression_type(expected, field.value());
        }
        if require_complete {
            for field in declared.keys() {
                if !seen.contains(field) {
                    self.errors
                        .push(HirVerificationError::MissingDeclaredField {
                            field: *field,
                            span,
                        });
                }
            }
        }
    }

    fn verify_field_get(
        &mut self,
        field: FieldId,
        base: &HirExpression,
        expression: &HirExpression,
    ) {
        let Some(declared) = self
            .schema
            .and_then(|schema| schema.fields.get(&field))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownField {
                    field,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_field_owner(field, base, &declared, expression.span());
        self.verify_expression_type(declared.field_type, expression);
    }

    fn verify_field_set(
        &mut self,
        field: FieldId,
        base: &HirExpression,
        value: &HirExpression,
        span: SourceSpan,
    ) {
        let Some(declared) = self
            .schema
            .and_then(|schema| schema.fields.get(&field))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors
                    .push(HirVerificationError::UnknownField { field, span });
            }
            return;
        };
        self.verify_field_owner(field, base, &declared, span);
        self.verify_expression_type(declared.field_type, value);
        if !declared.mutable {
            self.errors
                .push(HirVerificationError::ImmutableFieldSet { field, span });
        }
    }

    fn verify_field_owner(
        &mut self,
        field: FieldId,
        base: &HirExpression,
        declared: &HirDeclaredField,
        span: SourceSpan,
    ) {
        if !declared.owners.contains(&base.type_id()) {
            self.errors.push(HirVerificationError::WrongFieldOwner {
                field,
                found: base.type_id(),
                span,
            });
        }
    }

    fn verify_union_case(
        &mut self,
        union: SymbolId,
        case: UnionCaseId,
        arguments: &[HirExpression],
        expression: &HirExpression,
    ) {
        let Some(union_schema) = self
            .schema
            .and_then(|schema| schema.unions.get(&union))
            .cloned()
        else {
            if self.schema.is_some() {
                self.errors.push(HirVerificationError::UnknownUnion {
                    union,
                    span: expression.span(),
                });
            }
            return;
        };
        self.verify_expression_type(union_schema.type_id, expression);
        let Some(parameters) = union_schema.cases.get(&case) else {
            self.errors.push(HirVerificationError::UnknownUnionCase {
                union,
                case,
                span: expression.span(),
            });
            return;
        };
        if parameters.len() != arguments.len() {
            self.errors
                .push(HirVerificationError::InvalidCallSignature {
                    expected_arguments: parameters.len(),
                    found_arguments: arguments.len(),
                    expected_results: 1,
                    found_results: 1,
                    span: expression.span(),
                });
        }
        for (index, (argument, expected)) in arguments.iter().zip(parameters).enumerate() {
            if argument.type_id() != *expected {
                self.errors
                    .push(HirVerificationError::UnionCaseArgumentTypeMismatch {
                        union,
                        case,
                        index,
                        expected: *expected,
                        found: argument.type_id(),
                        span: argument.span(),
                    });
            }
        }
    }

    fn verify_array_get(
        &mut self,
        array: &HirExpression,
        index: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        self.verify_expression(array, visible);
        self.verify_expression(index, visible);
        if !matches!(
            self.arena.get(array.type_id()),
            Some(SemanticType::Array(_))
        ) {
            self.errors
                .push(HirVerificationError::InvalidCollectionType {
                    type_id: array.type_id(),
                    span: array.span(),
                });
        }
        if let Some(integer) = self.arena.source_type("Int") {
            self.verify_expression_type(integer, index);
        }
    }

    fn verify_array(
        &mut self,
        expression: &HirExpression,
        elements: &[HirExpression],
        visible: &BTreeSet<LocalId>,
    ) {
        let element_type = if let Some(SemanticType::Array(element_type)) =
            self.arena.get(expression.type_id()).cloned()
        {
            Some(element_type)
        } else {
            self.errors
                .push(HirVerificationError::InvalidCollectionType {
                    type_id: expression.type_id(),
                    span: expression.span(),
                });
            None
        };
        for element in elements {
            self.verify_expression(element, visible);
            if let Some(element_type) = element_type {
                self.verify_expression_type(element_type, element);
            }
        }
    }

    fn verify_table(
        &mut self,
        expression: &HirExpression,
        entries: &[HirTableEntry],
        visible: &BTreeSet<LocalId>,
    ) {
        let types = if let Some(SemanticType::Table { key, value }) =
            self.arena.get(expression.type_id()).cloned()
        {
            Some((key, value))
        } else {
            self.errors
                .push(HirVerificationError::InvalidCollectionType {
                    type_id: expression.type_id(),
                    span: expression.span(),
                });
            None
        };
        for entry in entries {
            self.verify_expression(entry.key(), visible);
            self.verify_expression(entry.value(), visible);
            if let Some((key, value)) = types {
                self.verify_expression_type(key, entry.key());
                self.verify_expression_type(value, entry.value());
            }
        }
    }

    fn verify_type(&mut self, type_id: TypeId, span: SourceSpan) {
        if !self.arena.is_valid_hir_type(type_id) {
            self.errors
                .push(HirVerificationError::InvalidType { type_id, span });
        }
    }

    fn verify_expression_type(&mut self, expected: TypeId, expression: &HirExpression) {
        if expression.type_id() != expected {
            self.errors
                .push(HirVerificationError::ExpressionTypeMismatch {
                    expected,
                    found: expression.type_id(),
                    span: expression.span(),
                });
        }
    }

    fn verify_fields(&mut self, fields: &[HirFieldValue], visible: &BTreeSet<LocalId>) {
        let mut seen = BTreeSet::new();
        for field in fields {
            if !seen.insert(field.field()) {
                self.errors
                    .push(HirVerificationError::DuplicateField(field.field()));
            }
            self.verify_expression(field.value(), visible);
        }
    }

    fn verify_function(&mut self, function: SymbolId, span: SourceSpan) {
        if !self.known_functions.contains(&function) {
            self.errors
                .push(HirVerificationError::UnknownFunction { function, span });
        }
    }
}

fn valid_hir_unary_operator(
    operator: TypedUnaryOperator,
    operand: TypeId,
    result: TypeId,
    arena: &TypeArena,
) -> bool {
    match operator {
        TypedUnaryOperator::Not => {
            arena.source_type("Boolean") == Some(operand) && operand == result
        }
        TypedUnaryOperator::Negate => {
            operand == result
                && (matches!(
                    arena.get(operand),
                    Some(SemanticType::Primitive(PrimitiveType::Integer(kind)))
                        if kind.is_signed()
                ) || is_hir_float(arena, operand))
        }
    }
}

fn valid_hir_binary_operator(
    operator: TypedBinaryOperator,
    left: TypeId,
    right: TypeId,
    result: TypeId,
    arena: &TypeArena,
) -> bool {
    let boolean = arena.source_type("Boolean");
    match operator {
        TypedBinaryOperator::Or | TypedBinaryOperator::And => {
            boolean == Some(left) && left == right && left == result
        }
        TypedBinaryOperator::Equal | TypedBinaryOperator::NotEqual => {
            left == right && boolean == Some(result) && hir_supports_default_equality(arena, left)
        }
        TypedBinaryOperator::LessThan | TypedBinaryOperator::GreaterThan => {
            left == right && boolean == Some(result) && is_hir_numeric(arena, left)
        }
        TypedBinaryOperator::Add
        | TypedBinaryOperator::Subtract
        | TypedBinaryOperator::Multiply
        | TypedBinaryOperator::Divide => {
            left == right && left == result && is_hir_numeric(arena, left)
        }
        TypedBinaryOperator::Remainder => {
            left == right && left == result && is_hir_integer(arena, left)
        }
    }
}

fn is_hir_numeric(arena: &TypeArena, type_id: TypeId) -> bool {
    is_hir_integer(arena, type_id) || is_hir_float(arena, type_id)
}

fn is_hir_integer(arena: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        arena.get(type_id),
        Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
    )
}

fn is_hir_float(arena: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        arena.get(type_id),
        Some(SemanticType::Primitive(
            PrimitiveType::Float32 | PrimitiveType::Float64
        ))
    )
}

fn hir_supports_default_equality(arena: &TypeArena, type_id: TypeId) -> bool {
    match arena.get(type_id) {
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
            .all(|element| hir_supports_default_equality(arena, *element)),
        Some(SemanticType::Record(fields)) => fields
            .iter()
            .all(|(_, field_type)| hir_supports_default_equality(arena, *field_type)),
        _ => false,
    }
}

fn empty_span() -> SourceSpan {
    SourceSpan::new(
        pop_foundation::FileId::from_raw(0),
        pop_foundation::TextRange::empty(pop_foundation::TextSize::from_u32(0)),
    )
}

fn method_span(method: &HirMethod) -> SourceSpan {
    method
        .parameters()
        .first()
        .map(HirParameter::span)
        .or_else(|| method.body().first().map(HirStatement::span))
        .unwrap_or_else(empty_span)
}

fn dump_declaration(output: &mut String, declaration: &HirDeclaration, arena: &TypeArena) {
    let _ = write!(
        output,
        "declaration s{} {} m{} b{} ",
        declaration.symbol.raw(),
        visibility_text(declaration.visibility),
        declaration.module.raw(),
        declaration.bubble.raw()
    );
    match &declaration.kind {
        HirDeclarationKind::Record(record) => {
            let _ = write!(
                output,
                "record {}:{}",
                declaration.name,
                type_text(record.type_id, arena)
            );
        }
        HirDeclarationKind::Union(union) => {
            let _ = write!(
                output,
                "union {}:{}",
                declaration.name,
                type_text(union.type_id, arena)
            );
        }
        HirDeclarationKind::Class(class) => {
            let _ = write!(
                output,
                "class {} c{}:{} {}",
                declaration.name,
                class.class.raw(),
                type_text(class.type_id, arena),
                if class.is_open { "open" } else { "sealed" }
            );
        }
        HirDeclarationKind::Attribute(attribute) => {
            let _ = write!(
                output,
                "attribute {} a{}",
                declaration.name,
                attribute.attribute.raw()
            );
        }
    }
    output.push('\n');
}

fn dump_function(output: &mut String, function: &HirFunction, arena: &TypeArena) {
    for attribute in &function.attributes {
        let _ = write!(
            output,
            "attribute a{} s{}(",
            attribute.attribute.raw(),
            attribute.definition.raw()
        );
        for (index, argument) in attribute.arguments.iter().enumerate() {
            if index != 0 {
                output.push_str(", ");
            }
            let _ = write!(
                output,
                "{}:{}=",
                argument.name,
                type_text(argument.value_type, arena)
            );
            dump_attribute_value(output, &argument.value);
        }
        output.push_str(")\n");
    }
    let _ = write!(
        output,
        "function s{} f{} {} m{} b{} {}(",
        function.symbol.raw(),
        function.function.raw(),
        visibility_text(function.visibility),
        function.module.raw(),
        function.bubble.raw(),
        function.name
    );
    for (index, parameter) in function.parameters.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(
            output,
            "p{}:{}:{}",
            parameter.parameter.raw(),
            parameter.name,
            type_text(parameter.type_id, arena)
        );
    }
    output.push_str(") -> (");
    for (index, result) in function.results.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        output.push_str(&type_text(*result, arena));
    }
    output.push_str(")\n");
    dump_statements(output, &function.body, arena, 1);
}

fn dump_method(output: &mut String, method: &HirMethod, arena: &TypeArena) {
    let _ = writeln!(
        output,
        "method m{} class c{} definition s{}",
        method.method.raw(),
        method.class.raw(),
        method.definition.raw()
    );
    dump_function(output, &method.function, arena);
}

fn dump_attribute_value(output: &mut String, value: &AttributeConstant) {
    match value {
        AttributeConstant::Nil => output.push_str("nil"),
        AttributeConstant::Boolean(value) => {
            output.push_str(if *value { "true" } else { "false" });
        }
        AttributeConstant::Integer(value) => {
            let _ = write!(output, "{value}");
        }
        AttributeConstant::Float(value) => dump_float_value(output, *value),
        AttributeConstant::String(value) => {
            output.push('"');
            output.push_str(value);
            output.push('"');
        }
        AttributeConstant::Tuple(values) => {
            output.push('(');
            for (index, value) in values.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                dump_attribute_value(output, value);
            }
            output.push(')');
        }
    }
}

fn dump_statements(
    output: &mut String,
    statements: &[HirStatement],
    arena: &TypeArena,
    depth: usize,
) {
    for statement in statements {
        let indentation = "  ".repeat(depth);
        output.push_str(&indentation);
        match statement.kind() {
            HirStatementKind::Local {
                local,
                name,
                local_type,
                initializer,
            } => {
                let _ = write!(
                    output,
                    "local l{} {}:{} = ",
                    local.raw(),
                    name,
                    type_text(*local_type, arena)
                );
                dump_expression(output, initializer, arena);
                output.push('\n');
            }
            HirStatementKind::Return { values } => {
                output.push_str("return");
                for value in values {
                    output.push(' ');
                    dump_expression(output, value, arena);
                }
                output.push('\n');
            }
            HirStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                output.push_str("if ");
                dump_expression(output, condition, arena);
                output.push('\n');
                dump_statements(output, then_body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("else\n");
                dump_statements(output, else_body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::While { condition, body } => {
                output.push_str("while ");
                dump_expression(output, condition, arena);
                output.push('\n');
                dump_statements(output, body, arena, depth + 1);
                output.push_str(&indentation);
                output.push_str("end\n");
            }
            HirStatementKind::FieldSet { base, field, value } => {
                output.push_str("field.set ");
                dump_expression(output, base, arena);
                let _ = write!(output, ".field#{} = ", field.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::Call(call) => {
                output.push_str("do ");
                dump_call(output, call.dispatch(), call.arguments(), arena);
                output.push('\n');
            }
            HirStatementKind::Expression(expression) => {
                dump_expression(output, expression, arena);
                output.push('\n');
            }
        }
    }
}

fn dump_expression(output: &mut String, expression: &HirExpression, arena: &TypeArena) {
    match expression.kind() {
        HirExpressionKind::Integer(value) => {
            let _ = write!(output, "{value}");
        }
        HirExpressionKind::Float(value) => dump_float_value(output, *value),
        HirExpressionKind::String(value) => output.push_str(value),
        HirExpressionKind::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
        HirExpressionKind::Nil => output.push_str("nil"),
        HirExpressionKind::Local(local) => {
            let _ = write!(output, "l{}", local.raw());
        }
        HirExpressionKind::Parameter(parameter) => {
            let _ = write!(output, "p{}", parameter.raw());
        }
        HirExpressionKind::Function(function) => {
            let _ = write!(output, "function s{}", function.raw());
        }
        HirExpressionKind::Field { base, field } => {
            dump_expression(output, base, arena);
            let _ = write!(output, ".field#{}", field.raw());
        }
        HirExpressionKind::ArrayGet { array, index } => {
            dump_array_get(output, array, index, arena);
        }
        HirExpressionKind::Record { record, fields } => {
            let _ = write!(output, "record s{} ", record.raw());
            dump_fields(output, fields, arena);
        }
        HirExpressionKind::ClassConstruct {
            class,
            definition,
            fields,
        } => {
            dump_class(output, *class, *definition, fields, arena);
        }
        HirExpressionKind::RecordUpdate {
            record,
            base,
            fields,
        } => {
            let _ = write!(output, "record.update s{} ", record.raw());
            dump_expression(output, base, arena);
            output.push(' ');
            dump_fields(output, fields, arena);
        }
        HirExpressionKind::Array(elements) => {
            dump_array(output, elements, arena);
        }
        HirExpressionKind::Table(entries) => {
            dump_table(output, entries, arena);
        }
        HirExpressionKind::UnionCase {
            union,
            case,
            arguments,
        } => {
            dump_union_case(output, *union, *case, arguments, arena);
        }
        HirExpressionKind::Tuple(elements) => {
            output.push('(');
            for (index, element) in elements.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                dump_expression(output, element, arena);
            }
            output.push(')');
        }
        HirExpressionKind::Unary { operator, operand } => {
            output.push_str(unary_text(*operator));
            output.push(' ');
            dump_expression(output, operand, arena);
        }
        HirExpressionKind::Binary {
            operator,
            left,
            right,
        } => {
            output.push('(');
            dump_expression(output, left, arena);
            output.push(' ');
            output.push_str(binary_text(*operator));
            output.push(' ');
            dump_expression(output, right, arena);
            output.push(')');
        }
        HirExpressionKind::Call {
            dispatch,
            arguments,
        } => {
            dump_call(output, dispatch, arguments, arena);
        }
    }
    let _ = write!(output, ":{}", type_text(expression.type_id(), arena));
}

fn dump_float_value(output: &mut String, value: FloatValue) {
    let _ = write!(
        output,
        "{}(0x{:x})",
        match value.kind() {
            pop_types::FloatKind::Float32 => "float32",
            pop_types::FloatKind::Float64 => "float64",
        },
        value.bits()
    );
}

fn dump_call(
    output: &mut String,
    dispatch: &HirCallDispatch,
    arguments: &[HirExpression],
    arena: &TypeArena,
) {
    match dispatch {
        HirCallDispatch::Direct { function } => {
            let _ = write!(output, "call.direct s{}(", function.raw());
        }
        HirCallDispatch::DirectMethod { method } => {
            let _ = write!(output, "call.method m{}(", method.raw());
        }
        HirCallDispatch::Indirect { callee } => {
            output.push_str("call.indirect ");
            dump_expression(output, callee, arena);
            output.push('(');
        }
    }
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, argument, arena);
    }
    output.push(')');
}

fn dump_class(
    output: &mut String,
    class: ClassId,
    definition: SymbolId,
    fields: &[HirFieldValue],
    arena: &TypeArena,
) {
    let _ = write!(output, "class c{} s{} ", class.raw(), definition.raw());
    dump_fields(output, fields, arena);
}

fn dump_union_case(
    output: &mut String,
    union: SymbolId,
    case: UnionCaseId,
    arguments: &[HirExpression],
    arena: &TypeArena,
) {
    let _ = write!(output, "union.case s{} case#{}(", union.raw(), case.raw());
    for (index, argument) in arguments.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, argument, arena);
    }
    output.push(')');
}

fn dump_array_get(
    output: &mut String,
    array: &HirExpression,
    index: &HirExpression,
    arena: &TypeArena,
) {
    output.push_str("array.get ");
    dump_expression(output, array, arena);
    output.push('[');
    dump_expression(output, index, arena);
    output.push(']');
}

fn dump_array(output: &mut String, elements: &[HirExpression], arena: &TypeArena) {
    output.push_str("array[");
    for (index, element) in elements.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, element, arena);
    }
    output.push(']');
}

fn dump_table(output: &mut String, entries: &[HirTableEntry], arena: &TypeArena) {
    output.push_str("table{");
    for (index, entry) in entries.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        dump_expression(output, entry.key(), arena);
        output.push_str(" => ");
        dump_expression(output, entry.value(), arena);
    }
    output.push('}');
}

fn dump_fields(output: &mut String, fields: &[HirFieldValue], arena: &TypeArena) {
    output.push('{');
    for (index, field) in fields.iter().enumerate() {
        if index != 0 {
            output.push_str(", ");
        }
        let _ = write!(output, "field#{} = ", field.field().raw());
        dump_expression(output, field.value(), arena);
    }
    output.push('}');
}

fn type_text(type_id: TypeId, arena: &TypeArena) -> String {
    if arena.get(type_id).is_some() {
        format!("t{}", type_id.raw())
    } else {
        format!("invalid-t{}", type_id.raw())
    }
}

const fn visibility_text(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "public",
        Visibility::Internal => "internal",
        Visibility::Private => "private",
    }
}

const fn unary_text(operator: TypedUnaryOperator) -> &'static str {
    match operator {
        TypedUnaryOperator::Not => "not",
        TypedUnaryOperator::Negate => "-",
    }
}

const fn binary_text(operator: TypedBinaryOperator) -> &'static str {
    match operator {
        TypedBinaryOperator::Or => "or",
        TypedBinaryOperator::And => "and",
        TypedBinaryOperator::Equal => "==",
        TypedBinaryOperator::NotEqual => "~=",
        TypedBinaryOperator::LessThan => "<",
        TypedBinaryOperator::GreaterThan => ">",
        TypedBinaryOperator::Add => "+",
        TypedBinaryOperator::Subtract => "-",
        TypedBinaryOperator::Multiply => "*",
        TypedBinaryOperator::Divide => "/",
        TypedBinaryOperator::Remainder => "%",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pop_foundation::{FileId, TextRange, TextSize};
    use pop_types::{IntegerKind, IntegerValue, SemanticType};

    #[test]
    fn verifier_rejects_collection_elements_with_inconsistent_types() {
        let mut arena = TypeArena::new();
        let string = arena.source_type("String").expect("String");
        let integer = arena.source_type("Int").expect("Int");
        let array = arena
            .intern(SemanticType::Array(string))
            .expect("array type");
        let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));
        let function = HirFunction {
            function: FunctionId::from_raw(0),
            symbol: SymbolId::from_raw(0),
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "invalid".to_owned(),
            parameters: Vec::new(),
            results: Vec::new(),
            body: vec![HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Array(vec![HirExpression {
                        kind: HirExpressionKind::Integer(
                            IntegerValue::parse_decimal("1", IntegerKind::Int64).expect("integer"),
                        ),
                        type_id: integer,
                        span,
                    }]),
                    type_id: array,
                    span,
                }),
                span,
            }],
            attributes: Vec::new(),
        };

        assert_eq!(
            verify_hir_function(&function, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::ExpressionTypeMismatch {
                expected: string,
                found: integer,
                span,
            }])
        );
    }

    #[test]
    fn verifier_rejects_array_access_on_a_non_array_base() {
        let mut arena = TypeArena::new();
        let string = arena.source_type("String").expect("String");
        let integer = arena.source_type("Int").expect("Int");
        let optional_string = arena.optional(string).expect("optional string");
        let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));
        let function = HirFunction {
            function: FunctionId::from_raw(0),
            symbol: SymbolId::from_raw(0),
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "invalid".to_owned(),
            parameters: Vec::new(),
            results: Vec::new(),
            body: vec![HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::ArrayGet {
                        array: Box::new(HirExpression {
                            kind: HirExpressionKind::String("\"value\"".to_owned()),
                            type_id: string,
                            span,
                        }),
                        index: Box::new(HirExpression {
                            kind: HirExpressionKind::Integer(
                                IntegerValue::parse_decimal("1", IntegerKind::Int64)
                                    .expect("integer"),
                            ),
                            type_id: integer,
                            span,
                        }),
                    },
                    type_id: optional_string,
                    span,
                }),
                span,
            }],
            attributes: Vec::new(),
        };

        assert_eq!(
            verify_hir_function(&function, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::InvalidCollectionType {
                type_id: string,
                span,
            }])
        );
    }

    #[test]
    fn verifier_rejects_numeric_operator_type_disagreement() {
        let arena = TypeArena::new();
        let int8 = arena.source_type("Int8").expect("Int8");
        let uint8 = arena.source_type("UInt8").expect("UInt8");
        let boolean = arena.source_type("Boolean").expect("Boolean");
        let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

        let mixed = HirExpression {
            kind: HirExpressionKind::Binary {
                operator: TypedBinaryOperator::Add,
                left: Box::new(integer_expression("1", IntegerKind::Int8, int8, span)),
                right: Box::new(integer_expression("1", IntegerKind::UInt8, uint8, span)),
            },
            type_id: int8,
            span,
        };
        assert_eq!(
            verify_expression_statement(mixed, &arena),
            Err(vec![HirVerificationError::InvalidBinaryOperator {
                operator: TypedBinaryOperator::Add,
                left: int8,
                right: uint8,
                result: int8,
                span,
            }])
        );

        let wrong_comparison_result = HirExpression {
            kind: HirExpressionKind::Binary {
                operator: TypedBinaryOperator::LessThan,
                left: Box::new(integer_expression("1", IntegerKind::Int8, int8, span)),
                right: Box::new(integer_expression("2", IntegerKind::Int8, int8, span)),
            },
            type_id: int8,
            span,
        };
        assert_eq!(
            verify_expression_statement(wrong_comparison_result, &arena),
            Err(vec![HirVerificationError::InvalidBinaryOperator {
                operator: TypedBinaryOperator::LessThan,
                left: int8,
                right: int8,
                result: int8,
                span,
            }])
        );

        let unsigned_negation = HirExpression {
            kind: HirExpressionKind::Unary {
                operator: TypedUnaryOperator::Negate,
                operand: Box::new(integer_expression("1", IntegerKind::UInt8, uint8, span)),
            },
            type_id: uint8,
            span,
        };
        assert_eq!(
            verify_expression_statement(unsigned_negation, &arena),
            Err(vec![HirVerificationError::InvalidUnaryOperator {
                operator: TypedUnaryOperator::Negate,
                operand: uint8,
                result: uint8,
                span,
            }])
        );

        let numeric_boolean = HirExpression {
            kind: HirExpressionKind::Binary {
                operator: TypedBinaryOperator::And,
                left: Box::new(integer_expression("1", IntegerKind::Int8, int8, span)),
                right: Box::new(integer_expression("2", IntegerKind::Int8, int8, span)),
            },
            type_id: boolean,
            span,
        };
        assert_eq!(
            verify_expression_statement(numeric_boolean, &arena),
            Err(vec![HirVerificationError::InvalidBinaryOperator {
                operator: TypedBinaryOperator::And,
                left: int8,
                right: int8,
                result: boolean,
                span,
            }])
        );
    }

    #[test]
    fn verifier_rejects_local_and_return_type_disagreement() {
        let arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

        let local_mismatch = hir_function(
            vec![],
            vec![],
            vec![HirStatement {
                kind: HirStatementKind::Local {
                    local: LocalId::from_raw(0),
                    name: "value".to_owned(),
                    local_type: integer,
                    initializer: string_expression(string, span),
                },
                span,
            }],
        );
        assert_eq!(
            verify_hir_function(&local_mismatch, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::ExpressionTypeMismatch {
                expected: integer,
                found: string,
                span,
            }])
        );

        let wrong_return = hir_function(
            vec![],
            vec![integer],
            vec![HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![string_expression(string, span)],
                },
                span,
            }],
        );
        assert_eq!(
            verify_hir_function(&wrong_return, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::ExpressionTypeMismatch {
                expected: integer,
                found: string,
                span,
            }])
        );

        let missing_return_value = hir_function(
            vec![],
            vec![integer],
            vec![HirStatement {
                kind: HirStatementKind::Return { values: Vec::new() },
                span,
            }],
        );
        assert_eq!(
            verify_hir_function(&missing_return_value, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::WrongReturnArity {
                expected: 1,
                found: 0,
                span,
            }])
        );
    }

    #[test]
    fn verifier_rejects_condition_and_parameter_type_disagreement() {
        let arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

        let numeric_condition = hir_function(
            vec![],
            vec![],
            vec![HirStatement {
                kind: HirStatementKind::If {
                    condition: integer_expression("1", IntegerKind::Int64, integer, span),
                    then_body: Vec::new(),
                    else_body: Vec::new(),
                },
                span,
            }],
        );
        assert_eq!(
            verify_hir_function(&numeric_condition, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::InvalidConditionType {
                found: integer,
                span,
            }])
        );

        let wrong_parameter_type = hir_function(
            vec![HirParameter {
                parameter: ValueParameterId::from_raw(0),
                name: "value".to_owned(),
                type_id: integer,
                span,
            }],
            vec![],
            vec![HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Parameter(ValueParameterId::from_raw(0)),
                    type_id: string,
                    span,
                }),
                span,
            }],
        );
        assert_eq!(
            verify_hir_function(&wrong_parameter_type, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::ExpressionTypeMismatch {
                expected: integer,
                found: string,
                span,
            }])
        );
    }

    #[test]
    fn verifier_rejects_literal_and_tuple_type_disagreement() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let tuple = arena
            .intern(SemanticType::Tuple(vec![integer, string]))
            .expect("tuple");
        let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));

        let wrong_tuple_element = hir_function(
            vec![],
            vec![],
            vec![HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Tuple(vec![
                        integer_expression("1", IntegerKind::Int64, integer, span),
                        integer_expression("2", IntegerKind::Int64, integer, span),
                    ]),
                    type_id: tuple,
                    span,
                }),
                span,
            }],
        );
        assert_eq!(
            verify_hir_function(&wrong_tuple_element, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::ExpressionTypeMismatch {
                expected: string,
                found: integer,
                span,
            }])
        );

        let wrong_literal_type = hir_function(
            vec![],
            vec![],
            vec![HirStatement {
                kind: HirStatementKind::Expression(string_expression(integer, span)),
                span,
            }],
        );
        assert_eq!(
            verify_hir_function(&wrong_literal_type, &arena, &BTreeSet::new()),
            Err(vec![HirVerificationError::InvalidType {
                type_id: integer,
                span,
            }])
        );
    }

    #[test]
    fn bubble_verifier_rejects_direct_call_argument_and_result_spoofing() {
        let arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let span = test_span();
        let target_function = hir_function_with_symbol(
            SymbolId::from_raw(1),
            vec![hir_parameter(0, "value", integer, span)],
            vec![integer],
            vec![HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![parameter_expression(0, integer, span)],
                },
                span,
            }],
        );
        let invoking_function = hir_function_with_symbol(
            SymbolId::from_raw(2),
            Vec::new(),
            vec![string],
            vec![
                HirStatement {
                    kind: HirStatementKind::Call(HirCall {
                        dispatch: HirCallDispatch::Direct {
                            function: SymbolId::from_raw(1),
                        },
                        arguments: Vec::new(),
                        span,
                    }),
                    span,
                },
                HirStatement {
                    kind: HirStatementKind::Return {
                        values: vec![HirExpression {
                            kind: HirExpressionKind::Call {
                                dispatch: HirCallDispatch::Direct {
                                    function: SymbolId::from_raw(1),
                                },
                                arguments: vec![string_expression(string, span)],
                            },
                            type_id: string,
                            span,
                        }],
                    },
                    span,
                },
            ],
        );
        let bubble = test_bubble(
            Vec::new(),
            vec![target_function, invoking_function],
            Vec::new(),
        );

        assert!(matches!(
            verify_hir_bubble(&bubble, &arena),
            Err(errors)
                if errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::InvalidCallSignature {
                        expected_arguments: 1,
                        found_arguments: 0,
                        expected_results: 1,
                        found_results: 0,
                        ..
                    }
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CallArgumentTypeMismatch {
                        index: 0,
                        expected,
                        found,
                        ..
                    } if *expected == integer && *found == string
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CallResultTypeMismatch {
                        expected,
                        found,
                        ..
                    } if *expected == integer && *found == string
                ))
        ));
    }

    #[test]
    fn bubble_verifier_rejects_indirect_call_argument_and_result_spoofing() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let callable = arena
            .intern(SemanticType::Function {
                parameters: vec![integer],
                results: vec![integer],
            })
            .expect("function type");
        let span = test_span();
        let caller = hir_function_with_symbol(
            SymbolId::from_raw(0),
            vec![hir_parameter(0, "operation", callable, span)],
            vec![string],
            vec![
                HirStatement {
                    kind: HirStatementKind::Call(HirCall {
                        dispatch: HirCallDispatch::Indirect {
                            callee: Box::new(parameter_expression(0, callable, span)),
                        },
                        arguments: Vec::new(),
                        span,
                    }),
                    span,
                },
                HirStatement {
                    kind: HirStatementKind::Return {
                        values: vec![HirExpression {
                            kind: HirExpressionKind::Call {
                                dispatch: HirCallDispatch::Indirect {
                                    callee: Box::new(parameter_expression(0, callable, span)),
                                },
                                arguments: vec![string_expression(string, span)],
                            },
                            type_id: string,
                            span,
                        }],
                    },
                    span,
                },
            ],
        );
        let bubble = test_bubble(Vec::new(), vec![caller], Vec::new());

        assert!(matches!(
            verify_hir_bubble(&bubble, &arena),
            Err(errors)
                if errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::InvalidCallSignature {
                        expected_arguments: 1,
                        found_arguments: 0,
                        expected_results: 1,
                        found_results: 0,
                        ..
                    }
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CallArgumentTypeMismatch {
                        index: 0,
                        expected,
                        found,
                        ..
                    } if *expected == integer && *found == string
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CallResultTypeMismatch {
                        expected,
                        found,
                        ..
                    } if *expected == integer && *found == string
                ))
        ));
    }

    #[test]
    fn bubble_verifier_rejects_spoofed_function_reference_type() {
        let arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let span = test_span();
        let callee = hir_function_with_symbol(
            SymbolId::from_raw(1),
            vec![hir_parameter(0, "value", integer, span)],
            vec![integer],
            vec![HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![parameter_expression(0, integer, span)],
                },
                span,
            }],
        );
        let observer = hir_function_with_symbol(
            SymbolId::from_raw(2),
            Vec::new(),
            Vec::new(),
            vec![HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Function(SymbolId::from_raw(1)),
                    type_id: string,
                    span,
                }),
                span,
            }],
        );
        let bubble = test_bubble(Vec::new(), vec![callee, observer], Vec::new());

        assert!(matches!(
            verify_hir_bubble(&bubble, &arena),
            Err(errors) if errors.iter().any(|error| matches!(
                error,
                HirVerificationError::InvalidFunctionReferenceType {
                    function,
                    found,
                    ..
                } if *function == SymbolId::from_raw(1) && *found == string
            ))
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn bubble_verifier_checks_receiver_method_signatures_against_class_schema() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let class = ClassId::from_raw(0);
        let class_type = arena
            .intern(SemanticType::Class {
                class,
                arguments: Vec::new(),
            })
            .expect("class type");
        let span = test_span();
        let definition = SymbolId::from_raw(10);
        let method = MethodId::from_raw(0);
        let declaration = HirDeclaration {
            symbol: definition,
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Public,
            name: "Counter".to_owned(),
            kind: HirDeclarationKind::Class(HirClassDeclaration {
                class,
                type_id: class_type,
                is_open: false,
                fields: Vec::new(),
                methods: vec![HirClassMethod {
                    method,
                    visibility: Visibility::Public,
                    name: "apply".to_owned(),
                    dispatch: ClassMethodDispatch::Receiver,
                    parameters: vec![HirNamedType {
                        name: "value".to_owned(),
                        type_id: integer,
                        span,
                    }],
                    results: vec![integer],
                    span,
                }],
            }),
            span,
        };
        let method_body = HirMethod {
            method,
            class,
            definition,
            function: hir_function_with_symbol(
                definition,
                vec![
                    hir_parameter(0, "self", class_type, span),
                    hir_parameter(1, "value", integer, span),
                ],
                vec![integer],
                vec![HirStatement {
                    kind: HirStatementKind::Return {
                        values: vec![parameter_expression(1, integer, span)],
                    },
                    span,
                }],
            ),
        };
        let caller = hir_function_with_symbol(
            SymbolId::from_raw(20),
            Vec::new(),
            vec![string],
            vec![
                HirStatement {
                    kind: HirStatementKind::Call(HirCall {
                        dispatch: HirCallDispatch::DirectMethod { method },
                        arguments: vec![string_expression(string, span)],
                        span,
                    }),
                    span,
                },
                HirStatement {
                    kind: HirStatementKind::Return {
                        values: vec![HirExpression {
                            kind: HirExpressionKind::Call {
                                dispatch: HirCallDispatch::DirectMethod { method },
                                arguments: vec![
                                    string_expression(string, span),
                                    string_expression(string, span),
                                ],
                            },
                            type_id: string,
                            span,
                        }],
                    },
                    span,
                },
            ],
        );
        let bubble = test_bubble(vec![declaration], vec![caller], vec![method_body]);

        assert!(matches!(
            verify_hir_bubble(&bubble, &arena),
            Err(errors)
                if errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::InvalidCallSignature {
                        expected_arguments: 2,
                        found_arguments: 1,
                        expected_results: 1,
                        found_results: 0,
                        ..
                    }
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CallArgumentTypeMismatch {
                        index: 0,
                        expected,
                        found,
                        ..
                    } if *expected == class_type && *found == string
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CallArgumentTypeMismatch {
                        index: 1,
                        expected,
                        found,
                        ..
                    } if *expected == integer && *found == string
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CallResultTypeMismatch {
                        expected,
                        found,
                        ..
                    } if *expected == integer && *found == string
                ))
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn bubble_verifier_checks_declaration_field_and_union_case_schema() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let record_type = arena
            .intern(SemanticType::Record(vec![("value".to_owned(), integer)]))
            .expect("record type");
        let union_symbol = SymbolId::from_raw(11);
        let union_type = arena
            .intern(SemanticType::TaggedUnion {
                definition: union_symbol,
            })
            .expect("union type");
        let span = test_span();
        let field = FieldId::from_raw(0);
        let case = UnionCaseId::from_raw(0);
        let record = HirDeclaration {
            symbol: SymbolId::from_raw(10),
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "Data".to_owned(),
            kind: HirDeclarationKind::Record(HirRecordDeclaration {
                type_id: record_type,
                fields: vec![HirRecordField {
                    field,
                    name: "value".to_owned(),
                    field_type: integer,
                    default: None,
                    span,
                }],
            }),
            span,
        };
        let union = HirDeclaration {
            symbol: union_symbol,
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "Choice".to_owned(),
            kind: HirDeclarationKind::Union(HirUnionDeclaration {
                type_id: union_type,
                cases: vec![HirUnionCase {
                    case,
                    name: "Value".to_owned(),
                    parameters: vec![HirNamedType {
                        name: "value".to_owned(),
                        type_id: integer,
                        span,
                    }],
                    span,
                }],
            }),
            span,
        };
        let invalid_union = HirDeclaration {
            symbol: SymbolId::from_raw(12),
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "InvalidChoice".to_owned(),
            kind: HirDeclarationKind::Union(HirUnionDeclaration {
                type_id: record_type,
                cases: Vec::new(),
            }),
            span,
        };
        let function = hir_function_with_symbol(
            SymbolId::from_raw(20),
            Vec::new(),
            Vec::new(),
            vec![
                HirStatement {
                    kind: HirStatementKind::Expression(HirExpression {
                        kind: HirExpressionKind::Record {
                            record: SymbolId::from_raw(10),
                            fields: Vec::new(),
                        },
                        type_id: record_type,
                        span,
                    }),
                    span,
                },
                HirStatement {
                    kind: HirStatementKind::Expression(HirExpression {
                        kind: HirExpressionKind::Field {
                            base: Box::new(string_expression(string, span)),
                            field,
                        },
                        type_id: integer,
                        span,
                    }),
                    span,
                },
                HirStatement {
                    kind: HirStatementKind::Expression(HirExpression {
                        kind: HirExpressionKind::UnionCase {
                            union: union_symbol,
                            case,
                            arguments: vec![string_expression(string, span)],
                        },
                        type_id: union_type,
                        span,
                    }),
                    span,
                },
                HirStatement {
                    kind: HirStatementKind::Expression(HirExpression {
                        kind: HirExpressionKind::UnionCase {
                            union: union_symbol,
                            case: UnionCaseId::from_raw(99),
                            arguments: Vec::new(),
                        },
                        type_id: union_type,
                        span,
                    }),
                    span,
                },
            ],
        );
        let bubble = test_bubble(
            vec![record, union, invalid_union],
            vec![function],
            Vec::new(),
        );

        assert!(matches!(
            verify_hir_bubble(&bubble, &arena),
            Err(errors)
                if errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::InvalidDeclarationType { symbol, type_id, .. }
                        if *symbol == SymbolId::from_raw(12) && *type_id == record_type
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::MissingDeclaredField { field: missing, .. }
                        if *missing == field
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::WrongFieldOwner { field: wrong, found, .. }
                        if *wrong == field && *found == string
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::UnionCaseArgumentTypeMismatch {
                        union,
                        case: found_case,
                        index: 0,
                        expected,
                        found,
                        ..
                    } if *union == union_symbol
                        && *found_case == case
                        && *expected == integer
                        && *found == string
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::UnknownUnionCase { union, case, .. }
                        if *union == union_symbol && *case == UnionCaseId::from_raw(99)
                ))
        ));
    }

    fn hir_function(
        parameters: Vec<HirParameter>,
        results: Vec<TypeId>,
        body: Vec<HirStatement>,
    ) -> HirFunction {
        hir_function_with_symbol(SymbolId::from_raw(0), parameters, results, body)
    }

    fn hir_function_with_symbol(
        symbol: SymbolId,
        parameters: Vec<HirParameter>,
        results: Vec<TypeId>,
        body: Vec<HirStatement>,
    ) -> HirFunction {
        HirFunction {
            function: FunctionId::from_raw(symbol.raw()),
            symbol,
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "invalid".to_owned(),
            parameters,
            results,
            body,
            attributes: Vec::new(),
        }
    }

    fn hir_parameter(raw: u32, name: &str, type_id: TypeId, span: SourceSpan) -> HirParameter {
        HirParameter {
            parameter: ValueParameterId::from_raw(raw),
            name: name.to_owned(),
            type_id,
            span,
        }
    }

    fn parameter_expression(raw: u32, type_id: TypeId, span: SourceSpan) -> HirExpression {
        HirExpression {
            kind: HirExpressionKind::Parameter(ValueParameterId::from_raw(raw)),
            type_id,
            span,
        }
    }

    fn test_bubble(
        declarations: Vec<HirDeclaration>,
        functions: Vec<HirFunction>,
        methods: Vec<HirMethod>,
    ) -> HirBubble {
        HirBubble::new_with_declarations_and_methods(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            declarations,
            functions,
            methods,
        )
        .expect("structurally assembled test Bubble")
    }

    fn test_span() -> SourceSpan {
        SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)))
    }

    fn string_expression(type_id: TypeId, span: SourceSpan) -> HirExpression {
        HirExpression {
            kind: HirExpressionKind::String("\"value\"".to_owned()),
            type_id,
            span,
        }
    }

    fn integer_expression(
        text: &str,
        kind: IntegerKind,
        type_id: TypeId,
        span: SourceSpan,
    ) -> HirExpression {
        HirExpression {
            kind: HirExpressionKind::Integer(
                IntegerValue::parse_decimal(text, kind).expect("integer"),
            ),
            type_id,
            span,
        }
    }

    fn verify_expression_statement(
        expression: HirExpression,
        arena: &TypeArena,
    ) -> Result<(), Vec<HirVerificationError>> {
        let span = expression.span();
        let function = HirFunction {
            function: FunctionId::from_raw(0),
            symbol: SymbolId::from_raw(0),
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "invalid".to_owned(),
            parameters: Vec::new(),
            results: Vec::new(),
            body: vec![HirStatement {
                kind: HirStatementKind::Expression(expression),
                span,
            }],
            attributes: Vec::new(),
        };
        verify_hir_function(&function, arena, &BTreeSet::new())
    }
}
