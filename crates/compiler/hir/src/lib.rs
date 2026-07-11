//! Typed, resolved, backend-neutral high-level IR.
#![allow(clippy::too_many_lines)]

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

use pop_foundation::{
    AttributeId, BindingId, BubbleId, CaptureId, ClassId, FieldId, FunctionId, InterfaceId,
    InterfaceMethodId, LocalId, MethodId, ModuleId, NamespaceId, NestedFunctionId, SourceSpan,
    SymbolId, TypeId, UnionCaseId, ValueParameterId,
};
use pop_resolve::Visibility;
use pop_types::{
    AttributeConstant, AttributeDefinition, CaptureMode, CaptureSource, ClassDefinition,
    ClassFieldDefault, ClassInterfaceImplementation, ClassMethodDefinition, ClassMethodDispatch,
    FieldDefault, FloatValue, IntegerValue, InterfaceDefinition, PrimitiveType, RecordDefinition,
    ResolvedAttribute, ResolvedFunctionSignature, SemanticType, TypeArena, TypedBinaryOperator,
    TypedBody, TypedCall, TypedCallDispatch, TypedCapture, TypedClosure, TypedExpression,
    TypedExpressionKind, TypedFieldValue, TypedMatchArm, TypedMatchBinding, TypedStatement,
    TypedStatementKind, TypedTableEntry, TypedUnaryOperator, UnionDefinition,
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
                interfaces: definition
                    .interfaces()
                    .iter()
                    .map(lower_interface_implementation)
                    .collect(),
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

    /// Retains one accepted nominal interface with its canonical member slots.
    #[must_use]
    pub fn interface(
        module: ModuleId,
        bubble: BubbleId,
        visibility: Visibility,
        name: impl Into<String>,
        definition: &InterfaceDefinition,
    ) -> Self {
        Self {
            symbol: definition.symbol(),
            module,
            bubble,
            visibility,
            name: name.into(),
            kind: HirDeclarationKind::Interface(HirInterfaceDeclaration {
                interface: definition.interface(),
                type_id: definition.type_id(),
                methods: definition
                    .methods()
                    .iter()
                    .map(|method| HirInterfaceMethod {
                        method: method.method(),
                        slot: method.slot(),
                        name: method.name().to_owned(),
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
    pub const fn as_class(&self) -> Option<&HirClassDeclaration> {
        if let HirDeclarationKind::Class(class) = &self.kind {
            Some(class)
        } else {
            None
        }
    }

    #[must_use]
    pub const fn as_interface(&self) -> Option<&HirInterfaceDeclaration> {
        if let HirDeclarationKind::Interface(interface) = &self.kind {
            Some(interface)
        } else {
            None
        }
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
    Interface(HirInterfaceDeclaration),
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
    interfaces: Vec<HirInterfaceImplementation>,
    fields: Vec<HirClassField>,
    methods: Vec<HirClassMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirInterfaceDeclaration {
    interface: InterfaceId,
    type_id: TypeId,
    methods: Vec<HirInterfaceMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirInterfaceMethod {
    method: InterfaceMethodId,
    slot: u32,
    name: String,
    parameters: Vec<HirNamedType>,
    results: Vec<TypeId>,
    span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirInterfaceImplementation {
    interface: InterfaceId,
    interface_type: TypeId,
    methods: Vec<HirInterfaceMethodImplementation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HirInterfaceMethodImplementation {
    interface_method: InterfaceMethodId,
    slot: u32,
    class_method: MethodId,
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
    pub fn interfaces(&self) -> &[HirInterfaceImplementation] {
        &self.interfaces
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

impl HirInterfaceDeclaration {
    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn methods(&self) -> &[HirInterfaceMethod] {
        &self.methods
    }
}

impl HirInterfaceMethod {
    #[must_use]
    pub const fn method(&self) -> InterfaceMethodId {
        self.method
    }

    #[must_use]
    pub const fn slot(&self) -> u32 {
        self.slot
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
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl HirInterfaceImplementation {
    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn interface_type(&self) -> TypeId {
        self.interface_type
    }

    #[must_use]
    pub fn methods(&self) -> &[HirInterfaceMethodImplementation] {
        &self.methods
    }
}

impl HirInterfaceMethodImplementation {
    #[must_use]
    pub const fn interface_method(&self) -> InterfaceMethodId {
        self.interface_method
    }

    #[must_use]
    pub const fn slot(&self) -> u32 {
        self.slot
    }

    #[must_use]
    pub const fn class_method(&self) -> MethodId {
        self.class_method
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
    effects: pop_types::EffectSummary,
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
    pub const fn effects(&self) -> pop_types::EffectSummary {
        self.effects
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
    binding: BindingId,
    parameter: ValueParameterId,
    name: String,
    type_id: TypeId,
    span: SourceSpan,
}

impl HirParameter {
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
        binding: BindingId,
        local: LocalId,
        name: String,
        local_type: TypeId,
        initializer: HirExpression,
    },
    LocalSet {
        local: LocalId,
        value: HirExpression,
    },
    ParameterSet {
        parameter: ValueParameterId,
        value: HirExpression,
    },
    CaptureSet {
        capture: CaptureId,
        value: HirExpression,
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
    Match {
        scrutinee: HirExpression,
        union: SymbolId,
        arms: Vec<HirMatchArm>,
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
pub struct HirMatchArm {
    union: SymbolId,
    case: UnionCaseId,
    bindings: Vec<HirMatchBinding>,
    body: Vec<HirStatement>,
    span: SourceSpan,
}

impl HirMatchArm {
    #[must_use]
    pub const fn union(&self) -> SymbolId {
        self.union
    }

    #[must_use]
    pub const fn case(&self) -> UnionCaseId {
        self.case
    }

    #[must_use]
    pub fn bindings(&self) -> &[HirMatchBinding] {
        &self.bindings
    }

    #[must_use]
    pub fn body(&self) -> &[HirStatement] {
        &self.body
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirMatchBinding {
    binding: Option<BindingId>,
    local: Option<LocalId>,
    name: String,
    type_id: TypeId,
    span: SourceSpan,
}

impl HirMatchBinding {
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

    #[must_use]
    pub fn is_ignored(&self) -> bool {
        self.name == "_" && self.binding.is_none() && self.local.is_none()
    }
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
    Closure(HirClosure),
    Local(LocalId),
    Parameter(ValueParameterId),
    Capture(CaptureId),
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
    InterfaceUpcast {
        value: Box<HirExpression>,
        interface: InterfaceId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClosure {
    function: NestedFunctionId,
    parameters: Vec<HirClosureParameter>,
    results: Vec<TypeId>,
    captures: Vec<HirCapture>,
    body: Vec<HirStatement>,
    span: SourceSpan,
    effects: pop_types::EffectSummary,
}

impl HirClosure {
    #[must_use]
    pub const fn function(&self) -> NestedFunctionId {
        self.function
    }

    #[must_use]
    pub fn parameters(&self) -> &[HirClosureParameter] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub fn captures(&self) -> &[HirCapture] {
        &self.captures
    }

    #[must_use]
    pub fn body(&self) -> &[HirStatement] {
        &self.body
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn effects(&self) -> pop_types::EffectSummary {
        self.effects
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClosureParameter {
    binding: BindingId,
    parameter: ValueParameterId,
    name: String,
    type_id: TypeId,
    span: SourceSpan,
}

impl HirClosureParameter {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirCaptureMode {
    Value,
    Cell,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirCaptureSource {
    Local(LocalId),
    Parameter(ValueParameterId),
    Capture(CaptureId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HirCapture {
    capture: CaptureId,
    binding: BindingId,
    source: HirCaptureSource,
    type_id: TypeId,
    mode: HirCaptureMode,
}

impl HirCapture {
    #[must_use]
    pub const fn capture(&self) -> CaptureId {
        self.capture
    }

    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    #[must_use]
    pub const fn source(&self) -> HirCaptureSource {
        self.source
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub const fn mode(&self) -> HirCaptureMode {
        self.mode
    }
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
    Direct {
        function: SymbolId,
    },
    DirectMethod {
        method: MethodId,
    },
    InterfaceMethod {
        interface: InterfaceId,
        method: InterfaceMethodId,
        slot: u32,
    },
    Indirect {
        callee: Box<HirExpression>,
    },
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
    interfaces: &'a [InterfaceDefinition],
}

impl<'a> HirKnownCallables<'a> {
    #[must_use]
    pub const fn new(functions: &'a BTreeSet<SymbolId>, methods: &'a BTreeSet<MethodId>) -> Self {
        Self {
            functions,
            methods,
            interfaces: &[],
        }
    }

    /// Adds nominal interface member schemas used to resolve per-interface
    /// dispatch slots while lowering typed calls.
    #[must_use]
    pub const fn with_interfaces(mut self, interfaces: &'a [InterfaceDefinition]) -> Self {
        self.interfaces = interfaces;
        self
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
) -> Result<HirFunction, Vec<HirBuildError>> {
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
) -> Result<HirFunction, Vec<HirBuildError>> {
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
) -> Result<HirFunction, Vec<HirBuildError>> {
    build_hir_function_with_known_callables_and_attributes(
        context,
        signature,
        body,
        arena,
        HirKnownCallables::new(known_functions, known_methods),
        attributes,
    )
}

/// Constructs a function with complete direct, class, and nominal-interface
/// callable schemas. Interface schemas are required for interface calls because
/// `InterfaceMethodId` is global while dispatch slots are per interface.
///
/// # Errors
///
/// Returns deterministic build or verification failures. In particular,
/// compile-time-only attribute queries and interface calls without a known
/// canonical slot never enter runtime HIR.
pub fn build_hir_function_with_known_callables_and_attributes(
    context: HirFunctionContext,
    signature: &ResolvedFunctionSignature,
    body: &TypedBody,
    arena: &TypeArena,
    known: HirKnownCallables<'_>,
    attributes: &[ResolvedAttribute],
) -> Result<HirFunction, Vec<HirBuildError>> {
    if let Some(span) = first_compile_time_only_statement(body.statements()) {
        return Err(vec![HirVerificationError::CompileTimeOnlyExpression {
            span,
        }]);
    }
    let interface_slots = collect_interface_slots(known.interfaces);
    if let Some((interface, method, span)) =
        first_unknown_interface_call(body.statements(), &interface_slots)
    {
        return Err(vec![HirVerificationError::UnknownInterfaceMethod {
            interface,
            method,
            span,
        }]);
    }
    let parameters: Option<Vec<_>> = signature
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, parameter)| {
            Some(HirParameter {
                binding: BindingId::from_raw(u32::try_from(index).ok()?),
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
        body: body
            .statements()
            .iter()
            .map(|statement| lower_statement(statement, &interface_slots))
            .collect(),
        attributes: attributes.iter().map(lower_attribute).collect(),
        effects: pop_types::EffectSummary::empty(),
    };
    verify_hir_callable(&function, arena, known.functions, known.methods)?;
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
) -> Result<HirMethod, Vec<HirBuildError>> {
    let function = build_hir_function_with_known_callables_and_attributes(
        context,
        signature,
        body,
        arena,
        known,
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

type HirInterfaceSlotMap = BTreeMap<(InterfaceId, InterfaceMethodId), u32>;

fn lower_statement(
    statement: &TypedStatement,
    interface_slots: &HirInterfaceSlotMap,
) -> HirStatement {
    let kind = match statement.kind() {
        TypedStatementKind::Local {
            binding,
            local,
            name,
            local_type,
            initializer,
        } => HirStatementKind::Local {
            binding: *binding,
            local: *local,
            name: name.clone(),
            local_type: *local_type,
            initializer: lower_expression(initializer, interface_slots),
        },
        TypedStatementKind::LocalSet { local, value } => HirStatementKind::LocalSet {
            local: *local,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::ParameterSet { parameter, value } => HirStatementKind::ParameterSet {
            parameter: *parameter,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::CaptureSet { capture, value } => HirStatementKind::CaptureSet {
            capture: *capture,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::Return { values } => HirStatementKind::Return {
            values: values
                .iter()
                .map(|value| lower_expression(value, interface_slots))
                .collect(),
        },
        TypedStatementKind::If {
            condition,
            then_body,
            else_body,
        } => HirStatementKind::If {
            condition: lower_expression(condition, interface_slots),
            then_body: then_body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
            else_body: else_body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::While { condition, body } => HirStatementKind::While {
            condition: lower_expression(condition, interface_slots),
            body: body
                .iter()
                .map(|statement| lower_statement(statement, interface_slots))
                .collect(),
        },
        TypedStatementKind::Match {
            scrutinee,
            union,
            arms,
        } => HirStatementKind::Match {
            scrutinee: lower_expression(scrutinee, interface_slots),
            union: *union,
            arms: arms
                .iter()
                .map(|arm| lower_match_arm(arm, interface_slots))
                .collect(),
        },
        TypedStatementKind::FieldSet { base, field, value } => HirStatementKind::FieldSet {
            base: lower_expression(base, interface_slots),
            field: *field,
            value: lower_expression(value, interface_slots),
        },
        TypedStatementKind::Call(call) => HirStatementKind::Call(lower_call(call, interface_slots)),
        TypedStatementKind::Expression(expression) => {
            HirStatementKind::Expression(lower_expression(expression, interface_slots))
        }
    };
    HirStatement {
        kind,
        span: statement.span(),
    }
}

fn lower_call(call: &TypedCall, interface_slots: &HirInterfaceSlotMap) -> HirCall {
    let dispatch = match call.dispatch() {
        TypedCallDispatch::Direct { function } => HirCallDispatch::Direct {
            function: *function,
        },
        TypedCallDispatch::DirectMethod { method, receiver } => {
            return HirCall {
                dispatch: HirCallDispatch::DirectMethod { method: *method },
                arguments: receiver
                    .iter()
                    .map(|receiver| lower_expression(receiver, interface_slots))
                    .chain(
                        call.arguments()
                            .iter()
                            .map(|argument| lower_expression(argument, interface_slots)),
                    )
                    .collect(),
                span: call.span(),
            };
        }
        TypedCallDispatch::InterfaceMethod {
            interface,
            method,
            receiver,
        } => {
            return HirCall {
                dispatch: HirCallDispatch::InterfaceMethod {
                    interface: *interface,
                    method: *method,
                    slot: interface_slots[&(*interface, *method)],
                },
                arguments: std::iter::once(lower_expression(receiver, interface_slots))
                    .chain(
                        call.arguments()
                            .iter()
                            .map(|argument| lower_expression(argument, interface_slots)),
                    )
                    .collect(),
                span: call.span(),
            };
        }
        TypedCallDispatch::Indirect { callee } => HirCallDispatch::Indirect {
            callee: Box::new(lower_expression(callee, interface_slots)),
        },
    };
    HirCall {
        dispatch,
        arguments: call
            .arguments()
            .iter()
            .map(|argument| lower_expression(argument, interface_slots))
            .collect(),
        span: call.span(),
    }
}

#[allow(clippy::too_many_lines)]
fn lower_expression(
    expression: &TypedExpression,
    interface_slots: &HirInterfaceSlotMap,
) -> HirExpression {
    let kind = match expression.kind() {
        TypedExpressionKind::Integer(value) => HirExpressionKind::Integer(*value),
        TypedExpressionKind::Float(value) => HirExpressionKind::Float(*value),
        TypedExpressionKind::String(value) => HirExpressionKind::String(value.clone()),
        TypedExpressionKind::Boolean(value) => HirExpressionKind::Boolean(*value),
        TypedExpressionKind::Nil => HirExpressionKind::Nil,
        TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. } => {
            unreachable!("compile-time-only attribute queries are rejected before runtime HIR")
        }
        TypedExpressionKind::Closure(closure) => {
            HirExpressionKind::Closure(lower_closure(closure, interface_slots))
        }
        TypedExpressionKind::Local(local) => HirExpressionKind::Local(*local),
        TypedExpressionKind::Parameter(parameter) => HirExpressionKind::Parameter(*parameter),
        TypedExpressionKind::Capture(capture) => HirExpressionKind::Capture(*capture),
        TypedExpressionKind::Function(function) => HirExpressionKind::Function(*function),
        TypedExpressionKind::Field { base, field } => HirExpressionKind::Field {
            base: Box::new(lower_expression(base, interface_slots)),
            field: *field,
        },
        TypedExpressionKind::ArrayGet { array, index } => HirExpressionKind::ArrayGet {
            array: Box::new(lower_expression(array, interface_slots)),
            index: Box::new(lower_expression(index, interface_slots)),
        },
        TypedExpressionKind::Record { record, fields } => HirExpressionKind::Record {
            record: *record,
            fields: fields
                .iter()
                .map(|field| lower_field_value(field, interface_slots))
                .collect(),
        },
        TypedExpressionKind::ClassConstruct {
            class,
            definition,
            fields,
        } => HirExpressionKind::ClassConstruct {
            class: *class,
            definition: *definition,
            fields: fields
                .iter()
                .map(|field| lower_field_value(field, interface_slots))
                .collect(),
        },
        TypedExpressionKind::RecordUpdate {
            record,
            base,
            fields,
        } => HirExpressionKind::RecordUpdate {
            record: *record,
            base: Box::new(lower_expression(base, interface_slots)),
            fields: fields
                .iter()
                .map(|field| lower_field_value(field, interface_slots))
                .collect(),
        },
        TypedExpressionKind::Array(elements) => HirExpressionKind::Array(
            elements
                .iter()
                .map(|element| lower_expression(element, interface_slots))
                .collect(),
        ),
        TypedExpressionKind::Table(entries) => HirExpressionKind::Table(
            entries
                .iter()
                .map(|entry| lower_table_entry(entry, interface_slots))
                .collect(),
        ),
        TypedExpressionKind::UnionCase {
            union,
            case,
            arguments,
        } => HirExpressionKind::UnionCase {
            union: *union,
            case: *case,
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::Tuple(elements) => HirExpressionKind::Tuple(
            elements
                .iter()
                .map(|element| lower_expression(element, interface_slots))
                .collect(),
        ),
        TypedExpressionKind::Unary { operator, operand } => HirExpressionKind::Unary {
            operator: *operator,
            operand: Box::new(lower_expression(operand, interface_slots)),
        },
        TypedExpressionKind::Binary {
            operator,
            left,
            right,
        } => HirExpressionKind::Binary {
            operator: *operator,
            left: Box::new(lower_expression(left, interface_slots)),
            right: Box::new(lower_expression(right, interface_slots)),
        },
        call @ (TypedExpressionKind::DirectCall { .. }
        | TypedExpressionKind::IndirectCall { .. }
        | TypedExpressionKind::DirectMethodCall { .. }
        | TypedExpressionKind::InterfaceMethodCall { .. }) => {
            lower_call_expression(call, interface_slots)
        }
        TypedExpressionKind::InterfaceUpcast { value, interface } => {
            HirExpressionKind::InterfaceUpcast {
                value: Box::new(lower_expression(value, interface_slots)),
                interface: *interface,
            }
        }
    };
    HirExpression {
        kind,
        type_id: expression.type_id(),
        span: expression.span(),
    }
}

fn lower_call_expression(
    call: &TypedExpressionKind,
    interface_slots: &HirInterfaceSlotMap,
) -> HirExpressionKind {
    match call {
        TypedExpressionKind::DirectCall {
            function,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Direct {
                function: *function,
            },
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::IndirectCall { callee, arguments } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::Indirect {
                callee: Box::new(lower_expression(callee, interface_slots)),
            },
            arguments: arguments
                .iter()
                .map(|argument| lower_expression(argument, interface_slots))
                .collect(),
        },
        TypedExpressionKind::DirectMethodCall {
            method,
            receiver,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::DirectMethod { method: *method },
            arguments: receiver
                .iter()
                .map(|receiver| lower_expression(receiver, interface_slots))
                .chain(
                    arguments
                        .iter()
                        .map(|argument| lower_expression(argument, interface_slots)),
                )
                .collect(),
        },
        TypedExpressionKind::InterfaceMethodCall {
            interface,
            method,
            receiver,
            arguments,
        } => HirExpressionKind::Call {
            dispatch: HirCallDispatch::InterfaceMethod {
                interface: *interface,
                method: *method,
                slot: interface_slots[&(*interface, *method)],
            },
            arguments: std::iter::once(lower_expression(receiver, interface_slots))
                .chain(
                    arguments
                        .iter()
                        .map(|argument| lower_expression(argument, interface_slots)),
                )
                .collect(),
        },
        _ => unreachable!("call lowering accepts only typed call expressions"),
    }
}

fn lower_closure(closure: &TypedClosure, interface_slots: &HirInterfaceSlotMap) -> HirClosure {
    HirClosure {
        function: closure.function(),
        parameters: closure
            .parameters()
            .iter()
            .map(|parameter| HirClosureParameter {
                binding: parameter.binding(),
                parameter: parameter.parameter(),
                name: parameter.name().to_owned(),
                type_id: parameter.type_id(),
                span: parameter.span(),
            })
            .collect(),
        results: closure.results().to_vec(),
        captures: closure.captures().iter().map(lower_capture).collect(),
        body: closure
            .body()
            .statements()
            .iter()
            .map(|statement| lower_statement(statement, interface_slots))
            .collect(),
        span: closure.span(),
        effects: pop_types::EffectSummary::empty(),
    }
}

fn lower_capture(capture: &TypedCapture) -> HirCapture {
    HirCapture {
        capture: capture.capture(),
        binding: capture.binding(),
        source: match capture.source() {
            CaptureSource::Local(local) => HirCaptureSource::Local(local),
            CaptureSource::Parameter(parameter) => HirCaptureSource::Parameter(parameter),
            CaptureSource::Capture(capture) => HirCaptureSource::Capture(capture),
        },
        type_id: capture.type_id(),
        mode: match capture.mode() {
            CaptureMode::Value => HirCaptureMode::Value,
            CaptureMode::Cell => HirCaptureMode::Cell,
        },
    }
}

fn lower_match_arm(arm: &TypedMatchArm, interface_slots: &HirInterfaceSlotMap) -> HirMatchArm {
    HirMatchArm {
        union: arm.union(),
        case: arm.case(),
        bindings: arm.bindings().iter().map(lower_match_binding).collect(),
        body: arm
            .body()
            .iter()
            .map(|statement| lower_statement(statement, interface_slots))
            .collect(),
        span: arm.span(),
    }
}

fn lower_match_binding(binding: &TypedMatchBinding) -> HirMatchBinding {
    HirMatchBinding {
        binding: binding.binding(),
        local: binding.local(),
        name: binding.name().to_owned(),
        type_id: binding.type_id(),
        span: binding.span(),
    }
}

fn lower_interface_implementation(
    implementation: &ClassInterfaceImplementation,
) -> HirInterfaceImplementation {
    HirInterfaceImplementation {
        interface: implementation.interface(),
        interface_type: implementation.interface_type(),
        methods: implementation
            .methods()
            .iter()
            .map(|method| HirInterfaceMethodImplementation {
                interface_method: method.interface_method(),
                slot: method.slot(),
                class_method: method.class_method(),
            })
            .collect(),
    }
}

fn collect_interface_slots(interfaces: &[InterfaceDefinition]) -> HirInterfaceSlotMap {
    interfaces
        .iter()
        .flat_map(|interface| {
            interface
                .methods()
                .iter()
                .map(move |method| ((interface.interface(), method.method()), method.slot()))
        })
        .collect()
}

fn first_unknown_interface_call(
    statements: &[TypedStatement],
    slots: &HirInterfaceSlotMap,
) -> Option<(InterfaceId, InterfaceMethodId, SourceSpan)> {
    for statement in statements {
        let found = match statement.kind() {
            TypedStatementKind::Local { initializer, .. } => {
                first_unknown_interface_expression(initializer, slots)
            }
            TypedStatementKind::LocalSet { value, .. }
            | TypedStatementKind::ParameterSet { value, .. }
            | TypedStatementKind::CaptureSet { value, .. }
            | TypedStatementKind::Expression(value) => {
                first_unknown_interface_expression(value, slots)
            }
            TypedStatementKind::Return { values } => values
                .iter()
                .find_map(|value| first_unknown_interface_expression(value, slots)),
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => first_unknown_interface_expression(condition, slots)
                .or_else(|| first_unknown_interface_call(then_body, slots))
                .or_else(|| first_unknown_interface_call(else_body, slots)),
            TypedStatementKind::While { condition, body } => {
                first_unknown_interface_expression(condition, slots)
                    .or_else(|| first_unknown_interface_call(body, slots))
            }
            TypedStatementKind::Match {
                scrutinee, arms, ..
            } => first_unknown_interface_expression(scrutinee, slots).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_unknown_interface_call(arm.body(), slots))
            }),
            TypedStatementKind::FieldSet { base, value, .. } => {
                first_unknown_interface_expression(base, slots)
                    .or_else(|| first_unknown_interface_expression(value, slots))
            }
            TypedStatementKind::Call(call) => {
                if let TypedCallDispatch::InterfaceMethod {
                    interface, method, ..
                } = call.dispatch()
                    && !slots.contains_key(&(*interface, *method))
                {
                    Some((*interface, *method, call.span()))
                } else {
                    let receiver = match call.dispatch() {
                        TypedCallDispatch::Direct { .. } => None,
                        TypedCallDispatch::DirectMethod { receiver, .. } => receiver
                            .as_deref()
                            .and_then(|value| first_unknown_interface_expression(value, slots)),
                        TypedCallDispatch::InterfaceMethod { receiver, .. } => {
                            first_unknown_interface_expression(receiver, slots)
                        }
                        TypedCallDispatch::Indirect { callee } => {
                            first_unknown_interface_expression(callee, slots)
                        }
                    };
                    receiver.or_else(|| {
                        call.arguments().iter().find_map(|argument| {
                            first_unknown_interface_expression(argument, slots)
                        })
                    })
                }
            }
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn first_unknown_interface_expression(
    expression: &TypedExpression,
    slots: &HirInterfaceSlotMap,
) -> Option<(InterfaceId, InterfaceMethodId, SourceSpan)> {
    match expression.kind() {
        TypedExpressionKind::InterfaceMethodCall {
            interface,
            method,
            receiver,
            arguments,
        } => {
            if !slots.contains_key(&(*interface, *method)) {
                return Some((*interface, *method, expression.span()));
            }
            first_unknown_interface_expression(receiver, slots).or_else(|| {
                arguments
                    .iter()
                    .find_map(|argument| first_unknown_interface_expression(argument, slots))
            })
        }
        TypedExpressionKind::Closure(closure) => {
            first_unknown_interface_call(closure.body().statements(), slots)
        }
        TypedExpressionKind::Field { base, .. } => first_unknown_interface_expression(base, slots),
        TypedExpressionKind::ClassConstruct { fields, .. }
        | TypedExpressionKind::Record { fields, .. } => fields
            .iter()
            .find_map(|field| first_unknown_interface_expression(field.value(), slots)),
        TypedExpressionKind::ArrayGet { array, index } => {
            first_unknown_interface_expression(array, slots)
                .or_else(|| first_unknown_interface_expression(index, slots))
        }
        TypedExpressionKind::RecordUpdate { base, fields, .. } => {
            first_unknown_interface_expression(base, slots).or_else(|| {
                fields
                    .iter()
                    .find_map(|field| first_unknown_interface_expression(field.value(), slots))
            })
        }
        TypedExpressionKind::Array(elements) | TypedExpressionKind::Tuple(elements) => elements
            .iter()
            .find_map(|element| first_unknown_interface_expression(element, slots)),
        TypedExpressionKind::Table(entries) => entries.iter().find_map(|entry| {
            first_unknown_interface_expression(entry.key(), slots)
                .or_else(|| first_unknown_interface_expression(entry.value(), slots))
        }),
        TypedExpressionKind::UnionCase { arguments, .. }
        | TypedExpressionKind::DirectCall { arguments, .. } => arguments
            .iter()
            .find_map(|argument| first_unknown_interface_expression(argument, slots)),
        TypedExpressionKind::Unary { operand, .. } => {
            first_unknown_interface_expression(operand, slots)
        }
        TypedExpressionKind::Binary { left, right, .. } => {
            first_unknown_interface_expression(left, slots)
                .or_else(|| first_unknown_interface_expression(right, slots))
        }
        TypedExpressionKind::IndirectCall { callee, arguments } => {
            first_unknown_interface_expression(callee, slots).or_else(|| {
                arguments
                    .iter()
                    .find_map(|argument| first_unknown_interface_expression(argument, slots))
            })
        }
        TypedExpressionKind::DirectMethodCall {
            receiver,
            arguments,
            ..
        } => receiver
            .as_deref()
            .and_then(|value| first_unknown_interface_expression(value, slots))
            .or_else(|| {
                arguments
                    .iter()
                    .find_map(|argument| first_unknown_interface_expression(argument, slots))
            }),
        TypedExpressionKind::InterfaceUpcast { value, .. } => {
            first_unknown_interface_expression(value, slots)
        }
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
        | TypedExpressionKind::Function(_) => None,
    }
}

fn first_compile_time_only_statement(statements: &[TypedStatement]) -> Option<SourceSpan> {
    for statement in statements {
        let found = match statement.kind() {
            TypedStatementKind::Local { initializer, .. } => {
                first_compile_time_only_expression(initializer)
            }
            TypedStatementKind::LocalSet { value, .. }
            | TypedStatementKind::ParameterSet { value, .. }
            | TypedStatementKind::CaptureSet { value, .. }
            | TypedStatementKind::Expression(value) => first_compile_time_only_expression(value),
            TypedStatementKind::Return { values } => {
                values.iter().find_map(first_compile_time_only_expression)
            }
            TypedStatementKind::If {
                condition,
                then_body,
                else_body,
            } => first_compile_time_only_expression(condition)
                .or_else(|| first_compile_time_only_statement(then_body))
                .or_else(|| first_compile_time_only_statement(else_body)),
            TypedStatementKind::While { condition, body } => {
                first_compile_time_only_expression(condition)
                    .or_else(|| first_compile_time_only_statement(body))
            }
            TypedStatementKind::Match {
                scrutinee, arms, ..
            } => first_compile_time_only_expression(scrutinee).or_else(|| {
                arms.iter()
                    .find_map(|arm| first_compile_time_only_statement(arm.body()))
            }),
            TypedStatementKind::FieldSet { base, value, .. } => {
                first_compile_time_only_expression(base)
                    .or_else(|| first_compile_time_only_expression(value))
            }
            TypedStatementKind::Call(call) => first_compile_time_only_call(call),
        };
        if found.is_some() {
            return found;
        }
    }
    None
}

fn first_compile_time_only_call(call: &TypedCall) -> Option<SourceSpan> {
    let callee = match call.dispatch() {
        TypedCallDispatch::Direct { .. } => None,
        TypedCallDispatch::DirectMethod { receiver, .. } => receiver
            .as_deref()
            .and_then(first_compile_time_only_expression),
        TypedCallDispatch::InterfaceMethod { receiver, .. } => {
            first_compile_time_only_expression(receiver)
        }
        TypedCallDispatch::Indirect { callee } => first_compile_time_only_expression(callee),
    };
    callee.or_else(|| {
        call.arguments()
            .iter()
            .find_map(first_compile_time_only_expression)
    })
}

fn first_compile_time_only_expression(expression: &TypedExpression) -> Option<SourceSpan> {
    match expression.kind() {
        TypedExpressionKind::AttributeQuery { .. }
        | TypedExpressionKind::HasAttributeQuery { .. } => Some(expression.span()),
        TypedExpressionKind::Closure(closure) => {
            first_compile_time_only_statement(closure.body().statements())
        }
        TypedExpressionKind::Field { base, .. } => first_compile_time_only_expression(base),
        TypedExpressionKind::ClassConstruct { fields, .. }
        | TypedExpressionKind::Record { fields, .. } => fields
            .iter()
            .find_map(|field| first_compile_time_only_expression(field.value())),
        TypedExpressionKind::ArrayGet { array, index } => first_compile_time_only_expression(array)
            .or_else(|| first_compile_time_only_expression(index)),
        TypedExpressionKind::RecordUpdate { base, fields, .. } => {
            first_compile_time_only_expression(base).or_else(|| {
                fields
                    .iter()
                    .find_map(|field| first_compile_time_only_expression(field.value()))
            })
        }
        TypedExpressionKind::Array(elements) | TypedExpressionKind::Tuple(elements) => {
            elements.iter().find_map(first_compile_time_only_expression)
        }
        TypedExpressionKind::Table(entries) => entries.iter().find_map(|entry| {
            first_compile_time_only_expression(entry.key())
                .or_else(|| first_compile_time_only_expression(entry.value()))
        }),
        TypedExpressionKind::UnionCase { arguments, .. }
        | TypedExpressionKind::DirectCall { arguments, .. } => arguments
            .iter()
            .find_map(first_compile_time_only_expression),
        TypedExpressionKind::Unary { operand, .. } => first_compile_time_only_expression(operand),
        TypedExpressionKind::Binary { left, right, .. } => first_compile_time_only_expression(left)
            .or_else(|| first_compile_time_only_expression(right)),
        TypedExpressionKind::IndirectCall { callee, arguments } => {
            first_compile_time_only_expression(callee).or_else(|| {
                arguments
                    .iter()
                    .find_map(first_compile_time_only_expression)
            })
        }
        TypedExpressionKind::DirectMethodCall {
            receiver,
            arguments,
            ..
        } => receiver
            .as_deref()
            .and_then(first_compile_time_only_expression)
            .or_else(|| {
                arguments
                    .iter()
                    .find_map(first_compile_time_only_expression)
            }),
        TypedExpressionKind::InterfaceMethodCall {
            receiver,
            arguments,
            ..
        } => first_compile_time_only_expression(receiver).or_else(|| {
            arguments
                .iter()
                .find_map(first_compile_time_only_expression)
        }),
        TypedExpressionKind::InterfaceUpcast { value, .. } => {
            first_compile_time_only_expression(value)
        }
        TypedExpressionKind::Integer(_)
        | TypedExpressionKind::Float(_)
        | TypedExpressionKind::String(_)
        | TypedExpressionKind::Boolean(_)
        | TypedExpressionKind::Nil
        | TypedExpressionKind::Local(_)
        | TypedExpressionKind::Parameter(_)
        | TypedExpressionKind::Capture(_)
        | TypedExpressionKind::Function(_) => None,
    }
}

fn lower_field_value(
    field: &TypedFieldValue,
    interface_slots: &HirInterfaceSlotMap,
) -> HirFieldValue {
    HirFieldValue {
        field: field.field(),
        value: lower_expression(field.value(), interface_slots),
        span: field.span(),
    }
}

fn lower_table_entry(
    entry: &TypedTableEntry,
    interface_slots: &HirInterfaceSlotMap,
) -> HirTableEntry {
    HirTableEntry {
        key: lower_expression(entry.key(), interface_slots),
        value: lower_expression(entry.value(), interface_slots),
        span: entry.span(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HirVerificationError {
    CompileTimeOnlyExpression {
        span: SourceSpan,
    },
    MissingCanonicalType,
    InvalidType {
        type_id: TypeId,
        span: SourceSpan,
    },
    DuplicateLocal(LocalId),
    DuplicateBinding(BindingId),
    DuplicateCapture(CaptureId),
    DuplicateCapturedBinding(BindingId),
    UnknownCapture {
        capture: CaptureId,
        span: SourceSpan,
    },
    InvalidCaptureSource {
        capture: CaptureId,
        binding: BindingId,
        span: SourceSpan,
    },
    CaptureTypeMismatch {
        capture: CaptureId,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    CaptureModeMismatch {
        capture: CaptureId,
        span: SourceSpan,
    },
    DuplicateNestedFunction(NestedFunctionId),
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
    DuplicateInterface(InterfaceId),
    DuplicateInterfaceMethod(InterfaceMethodId),
    DuplicateInterfaceImplementation(InterfaceId),
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
    UnknownInterface {
        interface: InterfaceId,
        span: SourceSpan,
    },
    WrongInterfaceType {
        interface: InterfaceId,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    UnknownInterfaceMethod {
        interface: InterfaceId,
        method: InterfaceMethodId,
        span: SourceSpan,
    },
    WrongInterfaceMethodSlot {
        interface: InterfaceId,
        method: InterfaceMethodId,
        expected: u32,
        found: u32,
        span: SourceSpan,
    },
    MissingInterfaceMethodMapping {
        class: ClassId,
        interface: InterfaceId,
        method: InterfaceMethodId,
        span: SourceSpan,
    },
    InterfaceMethodMappingMismatch {
        class: ClassId,
        interface: InterfaceId,
        method: InterfaceMethodId,
        class_method: MethodId,
        span: SourceSpan,
    },
    InvalidInterfaceUpcast {
        interface: InterfaceId,
        source: TypeId,
        target: TypeId,
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
    MatchScrutineeTypeMismatch {
        union: SymbolId,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    DuplicateMatchCase {
        union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    MissingMatchCase {
        union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    ForeignMatchCase {
        expected_union: SymbolId,
        found_union: SymbolId,
        case: UnionCaseId,
        span: SourceSpan,
    },
    MatchPayloadArityMismatch {
        union: SymbolId,
        case: UnionCaseId,
        expected: usize,
        found: usize,
        span: SourceSpan,
    },
    MatchPayloadTypeMismatch {
        union: SymbolId,
        case: UnionCaseId,
        index: usize,
        expected: TypeId,
        found: TypeId,
        span: SourceSpan,
    },
    InvalidIgnoredMatchBinding {
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

/// Construction and verification share one closed deterministic failure set.
/// Build-only variants prevent compile-time handles or unresolved interface
/// slots from ever becoming HIR nodes; the remaining variants are independently
/// rechecked whenever a complete Bubble is published.
pub type HirBuildError = HirVerificationError;

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
    interfaces: BTreeMap<InterfaceId, HirInterfaceImplementation>,
}

#[derive(Clone)]
struct HirInterfaceSchema {
    type_id: TypeId,
    methods: BTreeMap<InterfaceMethodId, HirInterfaceMethodSchema>,
}

#[derive(Clone)]
struct HirInterfaceMethodSchema {
    slot: u32,
    signature: HirCallableSignature,
    span: SourceSpan,
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
    visibility: Visibility,
    dispatch: ClassMethodDispatch,
    span: SourceSpan,
}

struct HirSchema {
    functions: BTreeMap<SymbolId, HirCallableSignature>,
    methods: BTreeMap<MethodId, HirCallableSignature>,
    declared_methods: BTreeMap<MethodId, HirDeclaredMethod>,
    records: BTreeMap<SymbolId, HirAggregateSchema>,
    unions: BTreeMap<SymbolId, HirUnionSchema>,
    classes: BTreeMap<ClassId, HirClassSchema>,
    interfaces: BTreeMap<InterfaceId, HirInterfaceSchema>,
    interface_methods: BTreeMap<InterfaceMethodId, InterfaceId>,
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
            interfaces: BTreeMap::new(),
            interface_methods: BTreeMap::new(),
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
        schema.verify_class_interfaces(errors);
        schema.collect_method_bodies(bubble.methods(), errors);
        schema
    }

    #[allow(clippy::too_many_lines)]
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
                let mut interfaces = BTreeMap::new();
                for implementation in &class.interfaces {
                    if interfaces
                        .insert(implementation.interface, implementation.clone())
                        .is_some()
                    {
                        errors.push(HirVerificationError::DuplicateInterfaceImplementation(
                            implementation.interface,
                        ));
                    }
                }
                if self
                    .classes
                    .insert(
                        class.class,
                        HirClassSchema {
                            definition: declaration.symbol(),
                            type_id: class.type_id,
                            fields,
                            interfaces,
                        },
                    )
                    .is_some()
                {
                    errors.push(HirVerificationError::DuplicateClass(class.class));
                }
                self.collect_declared_methods(declaration.symbol(), class, errors);
            }
            HirDeclarationKind::Interface(interface) => {
                if arena.get(interface.type_id)
                    != Some(&SemanticType::Interface {
                        interface: interface.interface,
                        arguments: Vec::new(),
                    })
                {
                    errors.push(HirVerificationError::InvalidDeclarationType {
                        symbol: declaration.symbol(),
                        type_id: interface.type_id,
                        span: declaration.span(),
                    });
                }
                let mut methods = BTreeMap::new();
                for (expected_slot, method) in interface.methods.iter().enumerate() {
                    for parameter in &method.parameters {
                        verify_schema_type(arena, parameter.type_id, parameter.span, errors);
                    }
                    for result in &method.results {
                        verify_schema_type(arena, *result, method.span, errors);
                    }
                    if method.slot != u32::try_from(expected_slot).unwrap_or(u32::MAX) {
                        errors.push(HirVerificationError::WrongInterfaceMethodSlot {
                            interface: interface.interface,
                            method: method.method,
                            expected: u32::try_from(expected_slot).unwrap_or(u32::MAX),
                            found: method.slot,
                            span: method.span,
                        });
                    }
                    if self
                        .interface_methods
                        .insert(method.method, interface.interface)
                        .is_some()
                        || methods
                            .insert(
                                method.method,
                                HirInterfaceMethodSchema {
                                    slot: method.slot,
                                    signature: HirCallableSignature {
                                        parameters: method
                                            .parameters
                                            .iter()
                                            .map(HirNamedType::type_id)
                                            .collect(),
                                        results: method.results.clone(),
                                    },
                                    span: method.span,
                                },
                            )
                            .is_some()
                    {
                        errors.push(HirVerificationError::DuplicateInterfaceMethod(
                            method.method,
                        ));
                    }
                }
                if self
                    .interfaces
                    .insert(
                        interface.interface,
                        HirInterfaceSchema {
                            type_id: interface.type_id,
                            methods,
                        },
                    )
                    .is_some()
                {
                    errors.push(HirVerificationError::DuplicateInterface(
                        interface.interface,
                    ));
                }
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
                visibility: method.visibility,
                dispatch: method.dispatch,
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

    fn verify_class_interfaces(&self, errors: &mut Vec<HirVerificationError>) {
        for (class_id, class) in &self.classes {
            let mut seen = BTreeSet::new();
            for implementation in class.interfaces.values() {
                if !seen.insert(implementation.interface) {
                    errors.push(HirVerificationError::DuplicateInterfaceImplementation(
                        implementation.interface,
                    ));
                }
                let Some(interface) = self.interfaces.get(&implementation.interface) else {
                    errors.push(HirVerificationError::UnknownInterface {
                        interface: implementation.interface,
                        span: empty_span(),
                    });
                    continue;
                };
                if implementation.interface_type != interface.type_id {
                    errors.push(HirVerificationError::WrongInterfaceType {
                        interface: implementation.interface,
                        expected: interface.type_id,
                        found: implementation.interface_type,
                        span: empty_span(),
                    });
                }
                let mut mapped = BTreeSet::new();
                for mapping in &implementation.methods {
                    if !mapped.insert(mapping.interface_method) {
                        errors.push(HirVerificationError::DuplicateInterfaceMethod(
                            mapping.interface_method,
                        ));
                        continue;
                    }
                    let Some(required) = interface.methods.get(&mapping.interface_method) else {
                        errors.push(HirVerificationError::UnknownInterfaceMethod {
                            interface: implementation.interface,
                            method: mapping.interface_method,
                            span: empty_span(),
                        });
                        continue;
                    };
                    if mapping.slot != required.slot {
                        errors.push(HirVerificationError::WrongInterfaceMethodSlot {
                            interface: implementation.interface,
                            method: mapping.interface_method,
                            expected: required.slot,
                            found: mapping.slot,
                            span: required.span,
                        });
                    }
                    let valid_class_method = self
                        .declared_methods
                        .get(&mapping.class_method)
                        .is_some_and(|method| {
                            method.class == *class_id
                                && method.visibility == Visibility::Public
                                && method.dispatch == ClassMethodDispatch::Receiver
                                && method.signature.parameters.first() == Some(&class.type_id)
                                && method.signature.parameters[1..] == required.signature.parameters
                                && method.signature.results == required.signature.results
                        });
                    if !valid_class_method {
                        errors.push(HirVerificationError::InterfaceMethodMappingMismatch {
                            class: *class_id,
                            interface: implementation.interface,
                            method: mapping.interface_method,
                            class_method: mapping.class_method,
                            span: required.span,
                        });
                    }
                }
                for required in interface.methods.keys() {
                    if !mapped.contains(required) {
                        errors.push(HirVerificationError::MissingInterfaceMethodMapping {
                            class: *class_id,
                            interface: implementation.interface,
                            method: *required,
                            span: interface.methods[required].span,
                        });
                    }
                }
            }
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
    let parameter_bindings: Vec<_> = function
        .parameters
        .iter()
        .map(|parameter| parameter.binding)
        .collect();
    let cell_bindings = collect_cell_bindings(
        &function.body,
        &parameter_bindings,
        &BTreeMap::new(),
        &BTreeMap::new(),
    );
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
        parameter_bindings,
        results: function.results.clone(),
        local_types: BTreeMap::new(),
        local_bindings: BTreeMap::new(),
        capture_types: BTreeMap::new(),
        capture_bindings: BTreeMap::new(),
        capture_modes: BTreeMap::new(),
        bindings: BTreeSet::new(),
        nested_functions: BTreeSet::new(),
        cell_bindings,
        errors: Vec::new(),
    };
    for parameter in &function.parameters {
        verifier.verify_type(parameter.type_id, parameter.span);
        if !verifier.bindings.insert(parameter.binding) {
            verifier
                .errors
                .push(HirVerificationError::DuplicateBinding(parameter.binding));
        }
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
    parameter_bindings: Vec<BindingId>,
    results: Vec<TypeId>,
    local_types: BTreeMap<LocalId, TypeId>,
    local_bindings: BTreeMap<LocalId, BindingId>,
    capture_types: BTreeMap<CaptureId, TypeId>,
    capture_bindings: BTreeMap<CaptureId, BindingId>,
    capture_modes: BTreeMap<CaptureId, HirCaptureMode>,
    bindings: BTreeSet<BindingId>,
    nested_functions: BTreeSet<NestedFunctionId>,
    cell_bindings: BTreeSet<BindingId>,
    errors: Vec<HirVerificationError>,
}

impl Verifier<'_> {
    #[allow(clippy::too_many_lines)]
    fn verify_statements(&mut self, statements: &[HirStatement], visible: &BTreeSet<LocalId>) {
        let mut visible = visible.clone();
        for statement in statements {
            match statement.kind() {
                HirStatementKind::Local {
                    binding,
                    local,
                    local_type,
                    initializer,
                    ..
                } => {
                    self.verify_type(*local_type, statement.span());
                    let recursive_closure = matches!(initializer.kind(), HirExpressionKind::Closure(closure)
                    if closure.captures.iter().any(|capture| {
                        capture.binding == *binding
                            && capture.source == HirCaptureSource::Local(*local)
                    }));
                    let mut initializer_visible = visible.clone();
                    if recursive_closure {
                        initializer_visible.insert(*local);
                    }
                    if self.local_types.insert(*local, *local_type).is_some() {
                        self.errors
                            .push(HirVerificationError::DuplicateLocal(*local));
                    }
                    self.local_bindings.insert(*local, *binding);
                    if !self.bindings.insert(*binding) {
                        self.errors
                            .push(HirVerificationError::DuplicateBinding(*binding));
                    }
                    self.verify_expression(initializer, &initializer_visible);
                    self.verify_expression_type(*local_type, initializer);
                    visible.insert(*local);
                }
                HirStatementKind::LocalSet { local, value } => {
                    self.verify_expression(value, &visible);
                    if !visible.contains(local) {
                        self.errors.push(HirVerificationError::UnknownLocal {
                            local: *local,
                            span: statement.span(),
                        });
                    } else if let Some(expected) = self.local_types.get(local).copied() {
                        self.verify_expression_type(expected, value);
                    }
                }
                HirStatementKind::ParameterSet { parameter, value } => {
                    self.verify_expression(value, &visible);
                    if let Some(expected) = self.parameter_type(*parameter) {
                        self.verify_expression_type(expected, value);
                    } else {
                        self.errors.push(HirVerificationError::UnknownParameter {
                            parameter: *parameter,
                            span: statement.span(),
                        });
                    }
                }
                HirStatementKind::CaptureSet { capture, value } => {
                    self.verify_expression(value, &visible);
                    if let Some(expected) = self.capture_types.get(capture).copied() {
                        self.verify_expression_type(expected, value);
                        if self.capture_modes.get(capture) != Some(&HirCaptureMode::Cell) {
                            self.errors.push(HirVerificationError::CaptureModeMismatch {
                                capture: *capture,
                                span: statement.span(),
                            });
                        }
                    } else {
                        self.errors.push(HirVerificationError::UnknownCapture {
                            capture: *capture,
                            span: statement.span(),
                        });
                    }
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
                HirStatementKind::Match {
                    scrutinee,
                    union,
                    arms,
                } => self.verify_match(scrutinee, *union, arms, statement.span(), &visible),
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
                let parameter_type = self.parameter_type(*parameter);
                if let Some(expected) = parameter_type {
                    self.verify_expression_type(expected, expression);
                } else {
                    self.errors.push(HirVerificationError::UnknownParameter {
                        parameter: *parameter,
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::Capture(capture) => {
                if let Some(expected) = self.capture_types.get(capture).copied() {
                    self.verify_expression_type(expected, expression);
                } else {
                    self.errors.push(HirVerificationError::UnknownCapture {
                        capture: *capture,
                        span: expression.span(),
                    });
                }
            }
            HirExpressionKind::Closure(closure) => {
                self.verify_closure(closure, expression, visible);
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
            HirExpressionKind::InterfaceUpcast { value, interface } => {
                self.verify_expression(value, visible);
                self.verify_interface_upcast(*interface, value, expression);
            }
            HirExpressionKind::Integer(_) | HirExpressionKind::Float(_) => {
                self.verify_numeric_literal(expression);
            }
            HirExpressionKind::String(_)
            | HirExpressionKind::Boolean(_)
            | HirExpressionKind::Nil => self.verify_primitive_literal(expression),
        }
    }

    fn parameter_type(&self, parameter: ValueParameterId) -> Option<TypeId> {
        usize::try_from(parameter.raw())
            .ok()
            .and_then(|raw| self.parameter_types.get(raw))
            .copied()
    }

    fn parameter_binding(&self, parameter: ValueParameterId) -> Option<BindingId> {
        usize::try_from(parameter.raw())
            .ok()
            .and_then(|raw| self.parameter_bindings.get(raw))
            .copied()
    }

    #[allow(clippy::too_many_lines)]
    fn verify_closure(
        &mut self,
        closure: &HirClosure,
        expression: &HirExpression,
        visible: &BTreeSet<LocalId>,
    ) {
        if !self.nested_functions.insert(closure.function) {
            self.errors
                .push(HirVerificationError::DuplicateNestedFunction(
                    closure.function,
                ));
        }
        let expected_function = SemanticType::Function {
            parameters: closure
                .parameters
                .iter()
                .map(HirClosureParameter::type_id)
                .collect(),
            results: closure.results.clone(),
            effects: pop_types::EffectSummary::empty(),
        };
        if self.arena.get(expression.type_id()) != Some(&expected_function) {
            self.errors.push(HirVerificationError::InvalidCallableType {
                type_id: expression.type_id(),
                span: expression.span(),
            });
        }

        let mut capture_ids = BTreeSet::new();
        let mut captured_bindings = BTreeSet::new();
        let mut previous_binding = None;
        let mut nested_capture_types = BTreeMap::new();
        let mut nested_capture_bindings = BTreeMap::new();
        let mut nested_capture_modes = BTreeMap::new();
        for capture in &closure.captures {
            self.verify_type(capture.type_id, closure.span);
            if !capture_ids.insert(capture.capture) {
                self.errors
                    .push(HirVerificationError::DuplicateCapture(capture.capture));
            }
            if !captured_bindings.insert(capture.binding) {
                self.errors
                    .push(HirVerificationError::DuplicateCapturedBinding(
                        capture.binding,
                    ));
            }
            if previous_binding.is_some_and(|previous| previous >= capture.binding) {
                self.errors
                    .push(HirVerificationError::InvalidCaptureSource {
                        capture: capture.capture,
                        binding: capture.binding,
                        span: closure.span,
                    });
            }
            previous_binding = Some(capture.binding);
            let source = match capture.source {
                HirCaptureSource::Local(local) if visible.contains(&local) => self
                    .local_types
                    .get(&local)
                    .copied()
                    .zip(self.local_bindings.get(&local).copied())
                    .map(|(type_id, binding)| (type_id, binding, None)),
                HirCaptureSource::Parameter(parameter) => self
                    .parameter_type(parameter)
                    .zip(self.parameter_binding(parameter))
                    .map(|(type_id, binding)| (type_id, binding, None)),
                HirCaptureSource::Capture(source) => self
                    .capture_types
                    .get(&source)
                    .copied()
                    .zip(self.capture_bindings.get(&source).copied())
                    .map(|(type_id, binding)| {
                        (type_id, binding, self.capture_modes.get(&source).copied())
                    }),
                HirCaptureSource::Local(_) => None,
            };
            let Some((source_type, source_binding, source_mode)) = source else {
                self.errors
                    .push(HirVerificationError::InvalidCaptureSource {
                        capture: capture.capture,
                        binding: capture.binding,
                        span: closure.span,
                    });
                continue;
            };
            if source_binding != capture.binding {
                self.errors
                    .push(HirVerificationError::InvalidCaptureSource {
                        capture: capture.capture,
                        binding: capture.binding,
                        span: closure.span,
                    });
            }
            if source_type != capture.type_id {
                self.errors.push(HirVerificationError::CaptureTypeMismatch {
                    capture: capture.capture,
                    expected: source_type,
                    found: capture.type_id,
                    span: closure.span,
                });
            }
            let expected_mode = if source_mode == Some(HirCaptureMode::Cell)
                || self.cell_bindings.contains(&capture.binding)
            {
                HirCaptureMode::Cell
            } else {
                HirCaptureMode::Value
            };
            if capture.mode != expected_mode {
                self.errors.push(HirVerificationError::CaptureModeMismatch {
                    capture: capture.capture,
                    span: closure.span,
                });
            }
            nested_capture_types.insert(capture.capture, capture.type_id);
            nested_capture_bindings.insert(capture.capture, capture.binding);
            nested_capture_modes.insert(capture.capture, capture.mode);
        }

        let saved_parameter_types = std::mem::replace(
            &mut self.parameter_types,
            closure
                .parameters
                .iter()
                .map(|parameter| parameter.type_id)
                .collect(),
        );
        let saved_parameter_bindings = std::mem::replace(
            &mut self.parameter_bindings,
            closure
                .parameters
                .iter()
                .map(|parameter| parameter.binding)
                .collect(),
        );
        let saved_results = std::mem::replace(&mut self.results, closure.results.clone());
        let saved_capture_types = std::mem::replace(&mut self.capture_types, nested_capture_types);
        let saved_capture_bindings =
            std::mem::replace(&mut self.capture_bindings, nested_capture_bindings);
        let saved_capture_modes = std::mem::replace(&mut self.capture_modes, nested_capture_modes);
        let nested_parameter_bindings = self.parameter_bindings.clone();
        let nested_cell_bindings = collect_cell_bindings(
            &closure.body,
            &nested_parameter_bindings,
            &self.capture_bindings,
            &self.capture_modes,
        );
        let saved_cell_bindings = std::mem::replace(&mut self.cell_bindings, nested_cell_bindings);
        for parameter in &closure.parameters {
            self.verify_type(parameter.type_id, parameter.span);
            if !self.bindings.insert(parameter.binding) {
                self.errors
                    .push(HirVerificationError::DuplicateBinding(parameter.binding));
            }
        }
        self.verify_statements(&closure.body, &BTreeSet::new());
        self.parameter_types = saved_parameter_types;
        self.parameter_bindings = saved_parameter_bindings;
        self.results = saved_results;
        self.capture_types = saved_capture_types;
        self.capture_bindings = saved_capture_bindings;
        self.capture_modes = saved_capture_modes;
        self.cell_bindings = saved_cell_bindings;
    }

    #[allow(clippy::too_many_lines)]
    fn verify_match(
        &mut self,
        scrutinee: &HirExpression,
        union: SymbolId,
        arms: &[HirMatchArm],
        span: SourceSpan,
        visible: &BTreeSet<LocalId>,
    ) {
        self.verify_expression(scrutinee, visible);
        let union_schema = self
            .schema
            .and_then(|schema| schema.unions.get(&union))
            .cloned();
        if self.schema.is_some() && union_schema.is_none() {
            self.errors
                .push(HirVerificationError::UnknownUnion { union, span });
        }
        if let Some(schema) = &union_schema
            && scrutinee.type_id() != schema.type_id
        {
            self.errors
                .push(HirVerificationError::MatchScrutineeTypeMismatch {
                    union,
                    expected: schema.type_id,
                    found: scrutinee.type_id(),
                    span: scrutinee.span(),
                });
        }
        let mut seen = BTreeSet::new();
        for arm in arms {
            if arm.union != union {
                self.errors.push(HirVerificationError::ForeignMatchCase {
                    expected_union: union,
                    found_union: arm.union,
                    case: arm.case,
                    span: arm.span,
                });
            }
            if !seen.insert(arm.case) {
                self.errors.push(HirVerificationError::DuplicateMatchCase {
                    union,
                    case: arm.case,
                    span: arm.span,
                });
            }
            let expected = union_schema
                .as_ref()
                .and_then(|schema| schema.cases.get(&arm.case));
            if union_schema.is_some() && expected.is_none() {
                self.errors.push(HirVerificationError::UnknownUnionCase {
                    union,
                    case: arm.case,
                    span: arm.span,
                });
            }
            if let Some(expected) = expected
                && expected.len() != arm.bindings.len()
            {
                self.errors
                    .push(HirVerificationError::MatchPayloadArityMismatch {
                        union,
                        case: arm.case,
                        expected: expected.len(),
                        found: arm.bindings.len(),
                        span: arm.span,
                    });
            }
            let mut arm_visible = visible.clone();
            for (index, binding) in arm.bindings.iter().enumerate() {
                self.verify_type(binding.type_id, binding.span);
                if let Some(expected) = expected.and_then(|types| types.get(index))
                    && *expected != binding.type_id
                {
                    self.errors
                        .push(HirVerificationError::MatchPayloadTypeMismatch {
                            union,
                            case: arm.case,
                            index,
                            expected: *expected,
                            found: binding.type_id,
                            span: binding.span,
                        });
                }
                match (binding.binding, binding.local, binding.name.as_str()) {
                    (None, None, "_") => {}
                    (Some(binding_id), Some(local), name) if name != "_" => {
                        if self.local_types.insert(local, binding.type_id).is_some() {
                            self.errors
                                .push(HirVerificationError::DuplicateLocal(local));
                        }
                        self.local_bindings.insert(local, binding_id);
                        if !self.bindings.insert(binding_id) {
                            self.errors
                                .push(HirVerificationError::DuplicateBinding(binding_id));
                        }
                        arm_visible.insert(local);
                    }
                    _ => self
                        .errors
                        .push(HirVerificationError::InvalidIgnoredMatchBinding {
                            span: binding.span,
                        }),
                }
            }
            self.verify_statements(&arm.body, &arm_visible);
        }
        if let Some(schema) = union_schema {
            for case in schema.cases.keys() {
                if !seen.contains(case) {
                    self.errors.push(HirVerificationError::MissingMatchCase {
                        union,
                        case: *case,
                        span,
                    });
                }
            }
        }
    }

    fn verify_interface_upcast(
        &mut self,
        interface_id: InterfaceId,
        value: &HirExpression,
        expression: &HirExpression,
    ) {
        let Some(schema) = self.schema else {
            return;
        };
        let Some(interface) = schema.interfaces.get(&interface_id) else {
            self.errors.push(HirVerificationError::UnknownInterface {
                interface: interface_id,
                span: expression.span(),
            });
            return;
        };
        let source_class = schema
            .classes
            .values()
            .find(|class| class.type_id == value.type_id());
        let valid = source_class.is_some_and(|class| class.interfaces.contains_key(&interface_id))
            && expression.type_id() == interface.type_id;
        if !valid {
            self.errors
                .push(HirVerificationError::InvalidInterfaceUpcast {
                    interface: interface_id,
                    source: value.type_id(),
                    target: expression.type_id(),
                    span: expression.span(),
                });
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
            HirCallDispatch::InterfaceMethod {
                interface,
                method,
                slot,
            } => {
                let signature =
                    self.schema
                        .and_then(|schema| schema.interfaces.get(interface))
                        .and_then(|interface_schema| {
                            interface_schema.methods.get(method).map(|method_schema| {
                                (interface_schema.type_id, method_schema.clone())
                            })
                        });
                if let Some((receiver_type, method_schema)) = signature {
                    if method_schema.slot != *slot {
                        self.errors
                            .push(HirVerificationError::WrongInterfaceMethodSlot {
                                interface: *interface,
                                method: *method,
                                expected: method_schema.slot,
                                found: *slot,
                                span,
                            });
                    }
                    let mut parameters = vec![receiver_type];
                    parameters.extend(method_schema.signature.parameters);
                    Some(HirCallableSignature {
                        parameters,
                        results: method_schema.signature.results,
                    })
                } else {
                    if self.schema.is_some() {
                        self.errors
                            .push(HirVerificationError::UnknownInterfaceMethod {
                                interface: *interface,
                                method: *method,
                                span,
                            });
                    }
                    None
                }
            }
            HirCallDispatch::Indirect { callee } => {
                self.verify_expression(callee, visible);
                if let Some(SemanticType::Function {
                    parameters,
                    results,
                    ..
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
            effects: pop_types::EffectSummary::empty(),
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

fn collect_cell_bindings(
    statements: &[HirStatement],
    parameter_bindings: &[BindingId],
    capture_bindings: &BTreeMap<CaptureId, BindingId>,
    capture_modes: &BTreeMap<CaptureId, HirCaptureMode>,
) -> BTreeSet<BindingId> {
    let mut local_bindings = BTreeMap::new();
    collect_local_binding_map(statements, &mut local_bindings);
    let mut written = BTreeSet::new();
    for (capture, mode) in capture_modes {
        if *mode == HirCaptureMode::Cell
            && let Some(binding) = capture_bindings.get(capture)
        {
            written.insert(*binding);
        }
    }
    collect_written_bindings(
        statements,
        parameter_bindings,
        capture_bindings,
        &local_bindings,
        &mut written,
    );
    written
}

fn collect_local_binding_map(
    statements: &[HirStatement],
    local_bindings: &mut BTreeMap<LocalId, BindingId>,
) {
    for statement in statements {
        match statement.kind() {
            HirStatementKind::Local { local, binding, .. } => {
                local_bindings.insert(*local, *binding);
            }
            HirStatementKind::If {
                then_body,
                else_body,
                ..
            } => {
                collect_local_binding_map(then_body, local_bindings);
                collect_local_binding_map(else_body, local_bindings);
            }
            HirStatementKind::While { body, .. } => {
                collect_local_binding_map(body, local_bindings);
            }
            HirStatementKind::Match { arms, .. } => {
                for arm in arms {
                    for binding in &arm.bindings {
                        if let (Some(binding), Some(local)) = (binding.binding, binding.local) {
                            local_bindings.insert(local, binding);
                        }
                    }
                    collect_local_binding_map(&arm.body, local_bindings);
                }
            }
            HirStatementKind::LocalSet { .. }
            | HirStatementKind::ParameterSet { .. }
            | HirStatementKind::CaptureSet { .. }
            | HirStatementKind::Return { .. }
            | HirStatementKind::FieldSet { .. }
            | HirStatementKind::Call(_)
            | HirStatementKind::Expression(_) => {}
        }
    }
}

fn collect_written_bindings(
    statements: &[HirStatement],
    parameter_bindings: &[BindingId],
    capture_bindings: &BTreeMap<CaptureId, BindingId>,
    local_bindings: &BTreeMap<LocalId, BindingId>,
    written: &mut BTreeSet<BindingId>,
) {
    for statement in statements {
        match statement.kind() {
            HirStatementKind::Local { initializer, .. } => {
                collect_cell_captures(initializer, written);
            }
            HirStatementKind::LocalSet { local, value } => {
                if let Some(binding) = local_bindings.get(local) {
                    written.insert(*binding);
                }
                collect_cell_captures(value, written);
            }
            HirStatementKind::ParameterSet { parameter, value } => {
                if let Some(binding) = usize::try_from(parameter.raw())
                    .ok()
                    .and_then(|raw| parameter_bindings.get(raw))
                {
                    written.insert(*binding);
                }
                collect_cell_captures(value, written);
            }
            HirStatementKind::CaptureSet { capture, value } => {
                if let Some(binding) = capture_bindings.get(capture) {
                    written.insert(*binding);
                }
                collect_cell_captures(value, written);
            }
            HirStatementKind::Return { values } => {
                for value in values {
                    collect_cell_captures(value, written);
                }
            }
            HirStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_cell_captures(condition, written);
                collect_written_bindings(
                    then_body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
                collect_written_bindings(
                    else_body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::While { condition, body } => {
                collect_cell_captures(condition, written);
                collect_written_bindings(
                    body,
                    parameter_bindings,
                    capture_bindings,
                    local_bindings,
                    written,
                );
            }
            HirStatementKind::Match {
                scrutinee, arms, ..
            } => {
                collect_cell_captures(scrutinee, written);
                for arm in arms {
                    collect_written_bindings(
                        &arm.body,
                        parameter_bindings,
                        capture_bindings,
                        local_bindings,
                        written,
                    );
                }
            }
            HirStatementKind::FieldSet { base, value, .. } => {
                collect_cell_captures(base, written);
                collect_cell_captures(value, written);
            }
            HirStatementKind::Call(call) => {
                if let HirCallDispatch::Indirect { callee } = call.dispatch() {
                    collect_cell_captures(callee, written);
                }
                for argument in call.arguments() {
                    collect_cell_captures(argument, written);
                }
            }
            HirStatementKind::Expression(expression) => {
                collect_cell_captures(expression, written);
            }
        }
    }
}

fn collect_cell_captures(expression: &HirExpression, written: &mut BTreeSet<BindingId>) {
    match expression.kind() {
        HirExpressionKind::Closure(closure) => {
            for capture in &closure.captures {
                if capture.mode == HirCaptureMode::Cell {
                    written.insert(capture.binding);
                }
            }
        }
        HirExpressionKind::Field { base, .. } => collect_cell_captures(base, written),
        HirExpressionKind::ArrayGet { array, index } => {
            collect_cell_captures(array, written);
            collect_cell_captures(index, written);
        }
        HirExpressionKind::Record { fields, .. }
        | HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                collect_cell_captures(field.value(), written);
            }
        }
        HirExpressionKind::RecordUpdate { base, fields, .. } => {
            collect_cell_captures(base, written);
            for field in fields {
                collect_cell_captures(field.value(), written);
            }
        }
        HirExpressionKind::Array(elements) | HirExpressionKind::Tuple(elements) => {
            for element in elements {
                collect_cell_captures(element, written);
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                collect_cell_captures(entry.key(), written);
                collect_cell_captures(entry.value(), written);
            }
        }
        HirExpressionKind::UnionCase { arguments, .. }
        | HirExpressionKind::Call { arguments, .. } => {
            for argument in arguments {
                collect_cell_captures(argument, written);
            }
            if let HirExpressionKind::Call {
                dispatch: HirCallDispatch::Indirect { callee },
                ..
            } = expression.kind()
            {
                collect_cell_captures(callee, written);
            }
        }
        HirExpressionKind::Unary { operand, .. } => collect_cell_captures(operand, written),
        HirExpressionKind::Binary { left, right, .. } => {
            collect_cell_captures(left, written);
            collect_cell_captures(right, written);
        }
        HirExpressionKind::InterfaceUpcast { value, .. } => {
            collect_cell_captures(value, written);
        }
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_) => {}
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
            for implementation in &class.interfaces {
                let _ = write!(
                    output,
                    " implements i{}:{}",
                    implementation.interface.raw(),
                    type_text(implementation.interface_type, arena)
                );
                for mapping in &implementation.methods {
                    let _ = write!(
                        output,
                        " [im{} slot{} => m{}]",
                        mapping.interface_method.raw(),
                        mapping.slot,
                        mapping.class_method.raw()
                    );
                }
            }
        }
        HirDeclarationKind::Interface(interface) => {
            let _ = write!(
                output,
                "interface {} i{}:{}",
                declaration.name,
                interface.interface.raw(),
                type_text(interface.type_id, arena)
            );
            for method in &interface.methods {
                let _ = write!(
                    output,
                    " [im{} slot{} {}(",
                    method.method.raw(),
                    method.slot,
                    method.name
                );
                for (index, parameter) in method.parameters.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&type_text(parameter.type_id, arena));
                }
                output.push_str(") -> (");
                for (index, result) in method.results.iter().enumerate() {
                    if index != 0 {
                        output.push_str(", ");
                    }
                    output.push_str(&type_text(*result, arena));
                }
                output.push_str(")]");
            }
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

#[allow(clippy::too_many_lines)]
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
                binding,
                local,
                name,
                local_type,
                initializer,
            } => {
                let _ = write!(
                    output,
                    "local bind#{} l{} {}:{} = ",
                    binding.raw(),
                    local.raw(),
                    name,
                    type_text(*local_type, arena)
                );
                dump_expression(output, initializer, arena);
                output.push('\n');
            }
            HirStatementKind::LocalSet { local, value } => {
                let _ = write!(output, "local.set l{} = ", local.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::ParameterSet { parameter, value } => {
                let _ = write!(output, "parameter.set p{} = ", parameter.raw());
                dump_expression(output, value, arena);
                output.push('\n');
            }
            HirStatementKind::CaptureSet { capture, value } => {
                let _ = write!(output, "capture.set cap{} = ", capture.raw());
                dump_expression(output, value, arena);
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
            HirStatementKind::Match {
                scrutinee,
                union,
                arms,
            } => {
                let _ = write!(output, "match s{} ", union.raw());
                dump_expression(output, scrutinee, arena);
                output.push('\n');
                for arm in arms {
                    output.push_str(&indentation);
                    let _ = write!(output, "when case#{}(", arm.case.raw());
                    for (index, binding) in arm.bindings.iter().enumerate() {
                        if index != 0 {
                            output.push_str(", ");
                        }
                        if binding.is_ignored() {
                            let _ = write!(output, "_:{}", type_text(binding.type_id, arena));
                        } else if let (Some(binding_id), Some(local)) =
                            (binding.binding, binding.local)
                        {
                            let _ = write!(
                                output,
                                "bind#{} l{} {}:{}",
                                binding_id.raw(),
                                local.raw(),
                                binding.name,
                                type_text(binding.type_id, arena)
                            );
                        }
                    }
                    output.push_str(")\n");
                    dump_statements(output, &arm.body, arena, depth + 1);
                }
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

#[allow(clippy::too_many_lines)]
fn dump_expression(output: &mut String, expression: &HirExpression, arena: &TypeArena) {
    match expression.kind() {
        HirExpressionKind::Integer(value) => {
            let _ = write!(output, "{value}");
        }
        HirExpressionKind::Float(value) => dump_float_value(output, *value),
        HirExpressionKind::String(value) => output.push_str(value),
        HirExpressionKind::Boolean(value) => output.push_str(if *value { "true" } else { "false" }),
        HirExpressionKind::Nil => output.push_str("nil"),
        HirExpressionKind::Closure(closure) => {
            let _ = write!(output, "closure nested#{} [", closure.function.raw());
            for (index, capture) in closure.captures.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                let _ = write!(
                    output,
                    "capture.{} cap{} bind#{}=",
                    match capture.mode {
                        HirCaptureMode::Value => "value",
                        HirCaptureMode::Cell => "cell",
                    },
                    capture.capture.raw(),
                    capture.binding.raw()
                );
                match capture.source {
                    HirCaptureSource::Local(local) => {
                        let _ = write!(output, "l{}", local.raw());
                    }
                    HirCaptureSource::Parameter(parameter) => {
                        let _ = write!(output, "p{}", parameter.raw());
                    }
                    HirCaptureSource::Capture(source) => {
                        let _ = write!(output, "cap{}", source.raw());
                    }
                }
                let _ = write!(output, ":{}", type_text(capture.type_id, arena));
            }
            output.push_str("] (");
            for (index, parameter) in closure.parameters.iter().enumerate() {
                if index != 0 {
                    output.push_str(", ");
                }
                let _ = write!(
                    output,
                    "bind#{} p{} {}:{}",
                    parameter.binding.raw(),
                    parameter.parameter.raw(),
                    parameter.name,
                    type_text(parameter.type_id, arena)
                );
            }
            output.push_str(") {\n");
            dump_statements(output, &closure.body, arena, 1);
            output.push('}');
        }
        HirExpressionKind::Local(local) => {
            let _ = write!(output, "l{}", local.raw());
        }
        HirExpressionKind::Parameter(parameter) => {
            let _ = write!(output, "p{}", parameter.raw());
        }
        HirExpressionKind::Capture(capture) => {
            let _ = write!(output, "cap{}", capture.raw());
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
        HirExpressionKind::InterfaceUpcast { value, interface } => {
            let _ = write!(output, "convert.interface i{} ", interface.raw());
            dump_expression(output, value, arena);
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
        HirCallDispatch::InterfaceMethod {
            interface,
            method,
            slot,
        } => {
            let _ = write!(
                output,
                "call.interface i{} im{} slot{}(",
                interface.raw(),
                method.raw(),
                slot
            );
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
            effects: pop_types::EffectSummary::empty(),
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
            effects: pop_types::EffectSummary::empty(),
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
                    binding: BindingId::from_raw(0),
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
                binding: BindingId::from_raw(0),
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
                effects: pop_types::EffectSummary::empty(),
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
                interfaces: Vec::new(),
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

    #[test]
    fn closure_verifier_rejects_duplicate_mistyped_and_wrongly_owned_captures() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let closure_type = arena
            .intern(SemanticType::Function {
                parameters: Vec::new(),
                results: Vec::new(),
                effects: pop_types::EffectSummary::empty(),
            })
            .expect("closure type");
        let span = test_span();
        let capture = CaptureId::from_raw(0);
        let function = hir_function(
            vec![hir_parameter(0, "value", integer, span)],
            Vec::new(),
            vec![HirStatement {
                kind: HirStatementKind::Expression(HirExpression {
                    kind: HirExpressionKind::Closure(HirClosure {
                        function: NestedFunctionId::from_raw(0),
                        parameters: Vec::new(),
                        results: Vec::new(),
                        captures: vec![
                            HirCapture {
                                capture,
                                binding: BindingId::from_raw(0),
                                source: HirCaptureSource::Parameter(ValueParameterId::from_raw(0)),
                                type_id: string,
                                mode: HirCaptureMode::Value,
                            },
                            HirCapture {
                                capture,
                                binding: BindingId::from_raw(0),
                                source: HirCaptureSource::Local(LocalId::from_raw(99)),
                                type_id: integer,
                                mode: HirCaptureMode::Cell,
                            },
                        ],
                        body: vec![HirStatement {
                            kind: HirStatementKind::Expression(HirExpression {
                                kind: HirExpressionKind::Capture(CaptureId::from_raw(99)),
                                type_id: integer,
                                span,
                            }),
                            span,
                        }],
                        span,
                        effects: pop_types::EffectSummary::empty(),
                    }),
                    type_id: closure_type,
                    span,
                }),
                span,
            }],
        );

        assert!(matches!(
            verify_hir_function(&function, &arena, &BTreeSet::new()),
            Err(errors)
                if errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::CaptureTypeMismatch {
                        capture: found,
                        expected,
                        found: found_type,
                        ..
                    } if *found == capture && *expected == integer && *found_type == string
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::DuplicateCapture(found) if *found == capture
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::InvalidCaptureSource { capture: found, .. }
                        if *found == capture
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::UnknownCapture { capture: found, .. }
                        if *found == CaptureId::from_raw(99)
                ))
        ));
    }

    #[test]
    fn match_verifier_rejects_duplicate_missing_and_mistyped_case_tables() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let union_symbol = SymbolId::from_raw(10);
        let union_type = arena
            .intern(SemanticType::TaggedUnion {
                definition: union_symbol,
            })
            .expect("union type");
        let span = test_span();
        let first_case = UnionCaseId::from_raw(0);
        let second_case = UnionCaseId::from_raw(1);
        let union = HirDeclaration {
            symbol: union_symbol,
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Private,
            name: "ResultValue".to_owned(),
            kind: HirDeclarationKind::Union(HirUnionDeclaration {
                type_id: union_type,
                cases: vec![
                    HirUnionCase {
                        case: first_case,
                        name: "Value".to_owned(),
                        parameters: vec![HirNamedType {
                            name: "value".to_owned(),
                            type_id: integer,
                            span,
                        }],
                        span,
                    },
                    HirUnionCase {
                        case: second_case,
                        name: "Empty".to_owned(),
                        parameters: Vec::new(),
                        span,
                    },
                ],
            }),
            span,
        };
        let invalid_binding = |binding, local| HirMatchBinding {
            binding: Some(BindingId::from_raw(binding)),
            local: Some(LocalId::from_raw(local)),
            name: "payload".to_owned(),
            type_id: string,
            span,
        };
        let function = hir_function_with_symbol(
            SymbolId::from_raw(20),
            vec![hir_parameter(0, "result", union_type, span)],
            Vec::new(),
            vec![HirStatement {
                kind: HirStatementKind::Match {
                    scrutinee: parameter_expression(0, union_type, span),
                    union: union_symbol,
                    arms: vec![
                        HirMatchArm {
                            union: union_symbol,
                            case: first_case,
                            bindings: vec![invalid_binding(1, 0)],
                            body: Vec::new(),
                            span,
                        },
                        HirMatchArm {
                            union: union_symbol,
                            case: first_case,
                            bindings: vec![invalid_binding(2, 1)],
                            body: Vec::new(),
                            span,
                        },
                    ],
                },
                span,
            }],
        );
        let bubble = test_bubble(vec![union], vec![function], Vec::new());

        assert!(matches!(
            verify_hir_bubble(&bubble, &arena),
            Err(errors)
                if errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::DuplicateMatchCase { case, .. }
                        if *case == first_case
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::MissingMatchCase { case, .. }
                        if *case == second_case
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::MatchPayloadTypeMismatch {
                        case,
                        expected,
                        found,
                        ..
                    } if *case == first_case && *expected == integer && *found == string
                ))
        ));
    }

    #[test]
    #[allow(clippy::too_many_lines)]
    fn interface_verifier_rejects_wrong_slots_mappings_arguments_and_results() {
        let mut arena = TypeArena::new();
        let integer = arena.source_type("Int").expect("Int");
        let string = arena.source_type("String").expect("String");
        let interface_id = InterfaceId::from_raw(0);
        let interface_type = arena
            .intern(SemanticType::Interface {
                interface: interface_id,
                arguments: Vec::new(),
            })
            .expect("interface type");
        let class_id = ClassId::from_raw(0);
        let class_type = arena
            .intern(SemanticType::Class {
                class: class_id,
                arguments: Vec::new(),
            })
            .expect("class type");
        let span = test_span();
        let interface_method = InterfaceMethodId::from_raw(7);
        let class_method = MethodId::from_raw(3);
        let interface = HirDeclaration {
            symbol: SymbolId::from_raw(10),
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Public,
            name: "Reader".to_owned(),
            kind: HirDeclarationKind::Interface(HirInterfaceDeclaration {
                interface: interface_id,
                type_id: interface_type,
                methods: vec![HirInterfaceMethod {
                    method: interface_method,
                    slot: 0,
                    name: "read".to_owned(),
                    parameters: vec![HirNamedType {
                        name: "count".to_owned(),
                        type_id: integer,
                        span,
                    }],
                    results: vec![string],
                    span,
                }],
            }),
            span,
        };
        let class_symbol = SymbolId::from_raw(11);
        let class = HirDeclaration {
            symbol: class_symbol,
            module: ModuleId::from_raw(0),
            bubble: BubbleId::from_raw(0),
            visibility: Visibility::Public,
            name: "FileReader".to_owned(),
            kind: HirDeclarationKind::Class(HirClassDeclaration {
                class: class_id,
                type_id: class_type,
                is_open: false,
                interfaces: vec![HirInterfaceImplementation {
                    interface: interface_id,
                    interface_type,
                    methods: vec![HirInterfaceMethodImplementation {
                        interface_method,
                        slot: 9,
                        class_method,
                    }],
                }],
                fields: Vec::new(),
                methods: vec![HirClassMethod {
                    method: class_method,
                    visibility: Visibility::Public,
                    name: "read".to_owned(),
                    dispatch: ClassMethodDispatch::Receiver,
                    parameters: vec![HirNamedType {
                        name: "count".to_owned(),
                        type_id: string,
                        span,
                    }],
                    results: vec![integer],
                    span,
                }],
            }),
            span,
        };
        let method_body = HirMethod {
            method: class_method,
            class: class_id,
            definition: class_symbol,
            function: hir_function_with_symbol(
                class_symbol,
                vec![
                    hir_parameter(0, "self", class_type, span),
                    hir_parameter(1, "count", string, span),
                ],
                vec![integer],
                vec![HirStatement {
                    kind: HirStatementKind::Return {
                        values: vec![integer_expression("0", IntegerKind::Int64, integer, span)],
                    },
                    span,
                }],
            ),
        };
        let caller = hir_function_with_symbol(
            SymbolId::from_raw(20),
            vec![hir_parameter(0, "reader", interface_type, span)],
            vec![integer],
            vec![HirStatement {
                kind: HirStatementKind::Return {
                    values: vec![HirExpression {
                        kind: HirExpressionKind::Call {
                            dispatch: HirCallDispatch::InterfaceMethod {
                                interface: interface_id,
                                method: interface_method,
                                slot: 8,
                            },
                            arguments: vec![
                                parameter_expression(0, interface_type, span),
                                string_expression(string, span),
                            ],
                        },
                        type_id: integer,
                        span,
                    }],
                },
                span,
            }],
        );
        let bubble = test_bubble(vec![interface, class], vec![caller], vec![method_body]);

        assert!(matches!(
            verify_hir_bubble(&bubble, &arena),
            Err(errors)
                if errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::WrongInterfaceMethodSlot {
                        method,
                        expected: 0,
                        found: 8 | 9,
                        ..
                    } if *method == interface_method
                )) && errors.iter().any(|error| matches!(
                    error,
                    HirVerificationError::InterfaceMethodMappingMismatch {
                        method,
                        class_method: found,
                        ..
                    } if *method == interface_method && *found == class_method
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
                    } if *expected == string && *found == integer
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
            effects: pop_types::EffectSummary::empty(),
        }
    }

    fn hir_parameter(raw: u32, name: &str, type_id: TypeId, span: SourceSpan) -> HirParameter {
        HirParameter {
            binding: BindingId::from_raw(raw),
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
            effects: pop_types::EffectSummary::empty(),
        };
        verify_hir_function(&function, arena, &BTreeSet::new())
    }
}
