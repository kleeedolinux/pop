//! Typed, resolved, backend-neutral high-level IR implementation.
//!
//! This module currently owns the complete HIR contract. Focused lowering,
//! verification, and textual-format modules are split beside it so contributors
//! can follow the architecture pipeline without searching one crate-root file.
#![allow(clippy::too_many_lines)]

use std::fmt::Write;

use pop_foundation::{
    AttributeId, BindingId, BubbleId, CaptureId, ClassId, EnumCaseId, FieldId, FunctionId,
    InterfaceId, InterfaceMethodId, LocalId, MethodId, ModuleId, NamespaceId, NestedFunctionId,
    SourceSpan, StandardFunctionId, SymbolId, SymbolIdentity, TypeId, UnionCaseId,
    ValueParameterId,
};
use pop_resolve::Visibility;
use pop_types::{
    AttributeConstant, AttributeDefinition, ClassDefinition, ClassFieldDefault,
    ClassMethodDispatch, EnumDefinition, FieldDefault, FloatValue, IntegerValue,
    InterfaceDefinition, NumericConversionKind, RecordDefinition, StringFormatKind, TypeArena,
    TypedBinaryOperator, TypedCompoundOperator, TypedUnaryOperator, UnionDefinition,
};

use crate::lowering::lower_interface_implementation;
use crate::text::{dump_declaration, dump_function, dump_method};
use crate::verification::{HirVerificationError, verify_hir_bubble};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirBubble {
    pub(crate) bubble: BubbleId,
    pub(crate) namespace: NamespaceId,
    pub(crate) dependencies: Vec<BubbleId>,
    pub(crate) declarations: Vec<HirDeclaration>,
    pub(crate) functions: Vec<HirFunction>,
    pub(crate) methods: Vec<HirMethod>,
    pub(crate) public_symbols: Vec<SymbolId>,
    pub(crate) function_references: Vec<HirFunctionReference>,
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
            function_references: Vec::new(),
        })
    }

    /// Attaches verified direct-dependency function signatures.
    ///
    /// # Errors
    ///
    /// Rejects references outside the Bubble dependency set or duplicate
    /// Bubble-scoped identities.
    pub fn with_function_references(
        mut self,
        mut references: Vec<HirFunctionReference>,
    ) -> Result<Self, HirBubbleError> {
        references.sort_by_key(HirFunctionReference::identity);
        let mut previous = None;
        for reference in &references {
            if !self.dependencies.contains(&reference.identity.bubble()) {
                return Err(HirBubbleError::UnknownReferenceBubble(
                    reference.identity.bubble(),
                ));
            }
            if previous == Some(reference.identity) {
                return Err(HirBubbleError::DuplicateReference(reference.identity));
            }
            previous = Some(reference.identity);
        }
        self.function_references = references;
        Ok(self)
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

    #[must_use]
    pub fn function_references(&self) -> &[HirFunctionReference] {
        &self.function_references
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
    DuplicateReference(SymbolIdentity),
    UnknownReferenceBubble(BubbleId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirFunctionReference {
    pub(crate) identity: SymbolIdentity,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) effects: pop_types::EffectSummary,
}

impl HirFunctionReference {
    #[must_use]
    pub fn new(
        identity: SymbolIdentity,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        effects: pop_types::EffectSummary,
    ) -> Self {
        Self {
            identity,
            parameters,
            results,
            effects,
        }
    }

    #[must_use]
    pub const fn identity(&self) -> SymbolIdentity {
        self.identity
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub const fn effects(&self) -> pop_types::EffectSummary {
        self.effects
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirDeclaration {
    pub(crate) symbol: SymbolId,
    pub(crate) module: ModuleId,
    pub(crate) bubble: BubbleId,
    pub(crate) visibility: Visibility,
    pub(crate) name: String,
    pub(crate) kind: HirDeclarationKind,
    pub(crate) span: SourceSpan,
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
    pub fn enumeration(
        module: ModuleId,
        bubble: BubbleId,
        visibility: Visibility,
        name: impl Into<String>,
        definition: &EnumDefinition,
    ) -> Self {
        Self {
            symbol: definition.symbol(),
            module,
            bubble,
            visibility,
            name: name.into(),
            kind: HirDeclarationKind::Enum(HirEnumDeclaration {
                type_id: definition.type_id(),
                cases: definition
                    .cases()
                    .iter()
                    .map(|case| HirEnumCase {
                        case: case.case(),
                        name: case.name().to_owned(),
                        discriminant: case.discriminant(),
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
    Enum(HirEnumDeclaration),
    Class(HirClassDeclaration),
    Interface(HirInterfaceDeclaration),
    Attribute(HirAttributeDeclaration),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirEnumDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) cases: Vec<HirEnumCase>,
}

impl HirEnumDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[HirEnumCase] {
        &self.cases
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirEnumCase {
    pub(crate) case: EnumCaseId,
    pub(crate) name: String,
    pub(crate) discriminant: u32,
    pub(crate) span: SourceSpan,
}

impl HirEnumCase {
    #[must_use]
    pub const fn case(&self) -> EnumCaseId {
        self.case
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn discriminant(&self) -> u32 {
        self.discriminant
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirRecordDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) fields: Vec<HirRecordField>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirRecordField {
    pub(crate) field: FieldId,
    pub(crate) name: String,
    pub(crate) field_type: TypeId,
    pub(crate) default: Option<FieldDefault>,
    pub(crate) span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirUnionDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) cases: Vec<HirUnionCase>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirUnionCase {
    pub(crate) case: UnionCaseId,
    pub(crate) name: String,
    pub(crate) parameters: Vec<HirNamedType>,
    pub(crate) span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClassDeclaration {
    pub(crate) class: ClassId,
    pub(crate) type_id: TypeId,
    pub(crate) is_open: bool,
    pub(crate) interfaces: Vec<HirInterfaceImplementation>,
    pub(crate) fields: Vec<HirClassField>,
    pub(crate) methods: Vec<HirClassMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirInterfaceDeclaration {
    pub(crate) interface: InterfaceId,
    pub(crate) type_id: TypeId,
    pub(crate) methods: Vec<HirInterfaceMethod>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirInterfaceMethod {
    pub(crate) method: InterfaceMethodId,
    pub(crate) slot: u32,
    pub(crate) name: String,
    pub(crate) parameters: Vec<HirNamedType>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirInterfaceImplementation {
    pub(crate) interface: InterfaceId,
    pub(crate) interface_type: TypeId,
    pub(crate) methods: Vec<HirInterfaceMethodImplementation>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HirInterfaceMethodImplementation {
    pub(crate) interface_method: InterfaceMethodId,
    pub(crate) slot: u32,
    pub(crate) class_method: MethodId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClassField {
    pub(crate) field: FieldId,
    pub(crate) visibility: Visibility,
    pub(crate) name: String,
    pub(crate) field_type: TypeId,
    pub(crate) default: Option<ClassFieldDefault>,
    pub(crate) span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClassMethod {
    pub(crate) method: MethodId,
    pub(crate) visibility: Visibility,
    pub(crate) name: String,
    pub(crate) dispatch: ClassMethodDispatch,
    pub(crate) parameters: Vec<HirNamedType>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirAttributeDeclaration {
    pub(crate) attribute: AttributeId,
    pub(crate) parameters: Vec<HirAttributeParameter>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirAttributeParameter {
    pub(crate) name: String,
    pub(crate) parameter_type: TypeId,
    pub(crate) default: Option<AttributeConstant>,
    pub(crate) span: SourceSpan,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirNamedType {
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
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
    pub(crate) method: MethodId,
    pub(crate) class: ClassId,
    pub(crate) definition: SymbolId,
    pub(crate) function: HirFunction,
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
    pub(crate) function: FunctionId,
    pub(crate) symbol: SymbolId,
    pub(crate) module: ModuleId,
    pub(crate) bubble: BubbleId,
    pub(crate) visibility: Visibility,
    pub(crate) name: String,
    pub(crate) type_parameters: Vec<TypeId>,
    pub(crate) parameters: Vec<HirParameter>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) body: Vec<HirStatement>,
    pub(crate) attributes: Vec<HirAttribute>,
    pub(crate) effects: pop_types::EffectSummary,
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
    pub fn type_parameters(&self) -> &[TypeId] {
        &self.type_parameters
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

/// Produces one concrete HIR callable instance while retaining only static
/// type information. MIR lowering uses this as its initial full-specialization
/// strategy; no runtime type argument is introduced.
#[must_use]
pub struct HirDataSpecialization {
    symbols: std::collections::BTreeMap<TypeId, SymbolId>,
    fields: std::collections::BTreeMap<(TypeId, FieldId), FieldId>,
}

impl HirDataSpecialization {
    #[must_use]
    pub fn new(
        symbols: std::collections::BTreeMap<TypeId, SymbolId>,
        fields: std::collections::BTreeMap<(TypeId, FieldId), FieldId>,
    ) -> Self {
        Self { symbols, fields }
    }

    fn symbol(&self, type_id: TypeId) -> Option<SymbolId> {
        self.symbols.get(&type_id).copied()
    }

    fn field(&self, type_id: TypeId, field: FieldId) -> FieldId {
        self.fields.get(&(type_id, field)).copied().unwrap_or(field)
    }
}

#[must_use]
pub fn specialize_hir_function(
    function: &HirFunction,
    symbol: SymbolId,
    type_arguments: &[TypeId],
    instances: &std::collections::BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    data_instances: &HirDataSpecialization,
    arena: &pop_types::TypeArena,
) -> Option<HirFunction> {
    if function.type_parameters.len() != type_arguments.len() {
        return None;
    }
    let substitutions: std::collections::BTreeMap<_, _> = function
        .type_parameters
        .iter()
        .zip(type_arguments)
        .map(|(parameter, argument)| match arena.get(*parameter) {
            Some(pop_types::SemanticType::TypeParameter(parameter)) => {
                Some((*parameter, *argument))
            }
            _ => None,
        })
        .collect::<Option<_>>()?;
    let mut specialized = function.clone();
    specialized.symbol = symbol;
    specialized.function = FunctionId::from_raw(symbol.raw());
    specialized.type_parameters.clear();
    for parameter in &mut specialized.parameters {
        parameter.type_id = arena.substitute_existing(parameter.type_id, &substitutions)?;
    }
    for result in &mut specialized.results {
        *result = arena.substitute_existing(*result, &substitutions)?;
    }
    for statement in &mut specialized.body {
        specialize_statement(statement, &substitutions, instances, arena)?;
    }
    remap_aggregate_statements(&mut specialized.body, data_instances);
    Some(specialized)
}

fn remap_aggregate_statements(statements: &mut [HirStatement], instances: &HirDataSpecialization) {
    for statement in statements {
        match &mut statement.kind {
            HirStatementKind::Local { initializer, .. } => {
                remap_aggregate_expression(initializer, instances)
            }
            HirStatementKind::MultipleLocal { value, .. }
            | HirStatementKind::LocalSet { value, .. }
            | HirStatementKind::ParameterSet { value, .. }
            | HirStatementKind::CaptureSet { value, .. }
            | HirStatementKind::Expression(value) => remap_aggregate_expression(value, instances),
            HirStatementKind::Return { values } => {
                for value in values {
                    remap_aggregate_expression(value, instances);
                }
            }
            HirStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                remap_aggregate_expression(condition, instances);
                remap_aggregate_statements(then_body, instances);
                remap_aggregate_statements(else_body, instances);
            }
            HirStatementKind::OptionalIf {
                initializer,
                then_body,
                else_body,
                ..
            } => {
                remap_aggregate_expression(initializer, instances);
                remap_aggregate_statements(then_body, instances);
                remap_aggregate_statements(else_body, instances);
            }
            HirStatementKind::While { condition, body } => {
                remap_aggregate_expression(condition, instances);
                remap_aggregate_statements(body, instances);
            }
            HirStatementKind::OptionalWhile {
                initializer, body, ..
            } => {
                remap_aggregate_expression(initializer, instances);
                remap_aggregate_statements(body, instances);
            }
            HirStatementKind::RepeatUntil { body, condition } => {
                remap_aggregate_statements(body, instances);
                remap_aggregate_expression(condition, instances);
            }
            HirStatementKind::NumericFor {
                first,
                last,
                step,
                body,
                ..
            } => {
                remap_aggregate_expression(first, instances);
                remap_aggregate_expression(last, instances);
                remap_aggregate_expression(step, instances);
                remap_aggregate_statements(body, instances);
            }
            HirStatementKind::Break | HirStatementKind::Continue => {}
            HirStatementKind::Match {
                scrutinee,
                union,
                arms,
            } => {
                remap_aggregate_expression(scrutinee, instances);
                if let Some(instance) = instances.symbol(scrutinee.type_id()) {
                    *union = instance;
                }
                for arm in arms {
                    if let Some(instance) = instances.symbol(scrutinee.type_id()) {
                        arm.union = instance;
                    }
                    remap_aggregate_statements(&mut arm.body, instances);
                }
            }
            HirStatementKind::FieldSet { base, field, value } => {
                remap_aggregate_expression(base, instances);
                *field = instances.field(base.type_id(), *field);
                remap_aggregate_expression(value, instances);
            }
            HirStatementKind::CompoundFieldSet {
                base, field, value, ..
            } => {
                remap_aggregate_expression(base, instances);
                *field = instances.field(base.type_id(), *field);
                remap_aggregate_expression(value, instances);
            }
            HirStatementKind::ArraySet {
                array,
                index,
                value,
            }
            | HirStatementKind::CompoundArraySet {
                array,
                index,
                value,
                ..
            } => {
                remap_aggregate_expression(array, instances);
                remap_aggregate_expression(index, instances);
                remap_aggregate_expression(value, instances);
            }
            HirStatementKind::TableSet { table, key, value } => {
                remap_aggregate_expression(table, instances);
                remap_aggregate_expression(key, instances);
                remap_aggregate_expression(value, instances);
            }
            HirStatementKind::MultipleAssignment { targets, value } => {
                for target in targets {
                    match target {
                        HirAssignmentTarget::Local { .. } | HirAssignmentTarget::Capture { .. } => {
                        }
                        HirAssignmentTarget::Field { base, field, .. } => {
                            remap_aggregate_expression(base, instances);
                            *field = instances.field(base.type_id(), *field);
                        }
                        HirAssignmentTarget::Array { array, index, .. } => {
                            remap_aggregate_expression(array, instances);
                            remap_aggregate_expression(index, instances);
                        }
                        HirAssignmentTarget::Table { table, key, .. } => {
                            remap_aggregate_expression(table, instances);
                            remap_aggregate_expression(key, instances);
                        }
                    }
                }
                remap_aggregate_expression(value, instances);
            }
            HirStatementKind::Call(call) => {
                for argument in &mut call.arguments {
                    remap_aggregate_expression(argument, instances);
                }
            }
        }
    }
}

fn remap_aggregate_expression(expression: &mut HirExpression, instances: &HirDataSpecialization) {
    match &mut expression.kind {
        HirExpressionKind::Closure(closure) => {
            remap_aggregate_statements(&mut closure.body, instances)
        }
        HirExpressionKind::Field { base, field } => {
            remap_aggregate_expression(base, instances);
            *field = instances.field(base.type_id(), *field);
        }
        HirExpressionKind::TupleGet { tuple: base, .. }
        | HirExpressionKind::InterfaceUpcast { value: base, .. }
        | HirExpressionKind::NumericConvert { value: base, .. }
        | HirExpressionKind::StringFormat { value: base, .. }
        | HirExpressionKind::Unary { operand: base, .. }
        | HirExpressionKind::ArrayLength { array: base } => {
            remap_aggregate_expression(base, instances)
        }
        HirExpressionKind::TableGet { table, key } => {
            remap_aggregate_expression(table, instances);
            remap_aggregate_expression(key, instances);
        }
        HirExpressionKind::ArrayGet { array, index }
        | HirExpressionKind::ArrayGetChecked { array, index }
        | HirExpressionKind::Binary {
            left: array,
            right: index,
            ..
        }
        | HirExpressionKind::StringConcat {
            left: array,
            right: index,
        } => {
            remap_aggregate_expression(array, instances);
            remap_aggregate_expression(index, instances);
        }
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            remap_aggregate_expression(length, instances);
            remap_aggregate_expression(initial_value, instances);
        }
        HirExpressionKind::ArrayFill { array, value } => {
            remap_aggregate_expression(array, instances);
            remap_aggregate_expression(value, instances);
        }
        HirExpressionKind::OptionalDefault { optional, fallback } => {
            remap_aggregate_expression(optional, instances);
            remap_aggregate_expression(fallback, instances);
        }
        HirExpressionKind::OptionalPropagate { optional, .. }
        | HirExpressionKind::OptionalNarrow { optional } => {
            remap_aggregate_expression(optional, instances);
        }
        HirExpressionKind::Record { record, fields } => {
            if let Some(instance) = instances.symbol(expression.type_id) {
                *record = instance;
            }
            for field in fields {
                field.field = instances.field(expression.type_id, field.field);
                remap_aggregate_expression(&mut field.value, instances);
            }
        }
        HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                remap_aggregate_expression(&mut field.value, instances);
            }
        }
        HirExpressionKind::RecordUpdate {
            record,
            base,
            fields,
        } => {
            if let Some(instance) = instances.symbol(expression.type_id) {
                *record = instance;
            }
            remap_aggregate_expression(base, instances);
            for field in fields {
                field.field = instances.field(expression.type_id, field.field);
                remap_aggregate_expression(&mut field.value, instances);
            }
        }
        HirExpressionKind::UnionCase {
            union, arguments, ..
        } => {
            if let Some(instance) = instances.symbol(expression.type_id) {
                *union = instance;
            }
            for argument in arguments {
                remap_aggregate_expression(argument, instances);
            }
        }
        HirExpressionKind::Array(values) | HirExpressionKind::Tuple(values) => {
            for value in values {
                remap_aggregate_expression(value, instances);
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                remap_aggregate_expression(&mut entry.key, instances);
                remap_aggregate_expression(&mut entry.value, instances);
            }
        }
        HirExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            remap_aggregate_expression(condition, instances);
            remap_aggregate_expression(when_true, instances);
            remap_aggregate_expression(when_false, instances);
        }
        HirExpressionKind::Call { arguments, .. } => {
            for argument in arguments {
                remap_aggregate_expression(argument, instances);
            }
        }
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::EnumCase { .. } => {}
    }
}

#[must_use]
pub fn hir_generic_call_instances(function: &HirFunction) -> Vec<(SymbolId, Vec<TypeId>)> {
    let mut calls = Vec::new();
    collect_statement_calls(&function.body, &mut calls);
    calls.sort();
    calls.dedup();
    calls
}

fn collect_statement_calls(statements: &[HirStatement], calls: &mut Vec<(SymbolId, Vec<TypeId>)>) {
    for statement in statements {
        match statement.kind() {
            HirStatementKind::Local { initializer, .. } => {
                collect_expression_calls(initializer, calls)
            }
            HirStatementKind::MultipleLocal { value, .. }
            | HirStatementKind::LocalSet { value, .. }
            | HirStatementKind::ParameterSet { value, .. }
            | HirStatementKind::CaptureSet { value, .. }
            | HirStatementKind::Expression(value) => collect_expression_calls(value, calls),
            HirStatementKind::Return { values } => {
                for value in values {
                    collect_expression_calls(value, calls);
                }
            }
            HirStatementKind::If {
                condition,
                then_body,
                else_body,
            } => {
                collect_expression_calls(condition, calls);
                collect_statement_calls(then_body, calls);
                collect_statement_calls(else_body, calls);
            }
            HirStatementKind::OptionalIf {
                initializer,
                then_body,
                else_body,
                ..
            } => {
                collect_expression_calls(initializer, calls);
                collect_statement_calls(then_body, calls);
                collect_statement_calls(else_body, calls);
            }
            HirStatementKind::While { condition, body } => {
                collect_expression_calls(condition, calls);
                collect_statement_calls(body, calls);
            }
            HirStatementKind::OptionalWhile {
                initializer, body, ..
            } => {
                collect_expression_calls(initializer, calls);
                collect_statement_calls(body, calls);
            }
            HirStatementKind::RepeatUntil { body, condition } => {
                collect_statement_calls(body, calls);
                collect_expression_calls(condition, calls);
            }
            HirStatementKind::NumericFor {
                first,
                last,
                step,
                body,
                ..
            } => {
                collect_expression_calls(first, calls);
                collect_expression_calls(last, calls);
                collect_expression_calls(step, calls);
                collect_statement_calls(body, calls);
            }
            HirStatementKind::Break | HirStatementKind::Continue => {}
            HirStatementKind::Match {
                scrutinee, arms, ..
            } => {
                collect_expression_calls(scrutinee, calls);
                for arm in arms {
                    collect_statement_calls(arm.body(), calls);
                }
            }
            HirStatementKind::FieldSet { base, value, .. }
            | HirStatementKind::CompoundFieldSet { base, value, .. } => {
                collect_expression_calls(base, calls);
                collect_expression_calls(value, calls);
            }
            HirStatementKind::ArraySet {
                array,
                index,
                value,
            }
            | HirStatementKind::CompoundArraySet {
                array,
                index,
                value,
                ..
            } => {
                collect_expression_calls(array, calls);
                collect_expression_calls(index, calls);
                collect_expression_calls(value, calls);
            }
            HirStatementKind::TableSet { table, key, value } => {
                collect_expression_calls(table, calls);
                collect_expression_calls(key, calls);
                collect_expression_calls(value, calls);
            }
            HirStatementKind::MultipleAssignment { targets, value } => {
                for target in targets {
                    match target {
                        HirAssignmentTarget::Local { .. } | HirAssignmentTarget::Capture { .. } => {
                        }
                        HirAssignmentTarget::Field { base, .. } => {
                            collect_expression_calls(base, calls)
                        }
                        HirAssignmentTarget::Array { array, index, .. } => {
                            collect_expression_calls(array, calls);
                            collect_expression_calls(index, calls);
                        }
                        HirAssignmentTarget::Table { table, key, .. } => {
                            collect_expression_calls(table, calls);
                            collect_expression_calls(key, calls);
                        }
                    }
                }
                collect_expression_calls(value, calls);
            }
            HirStatementKind::Call(call) => {
                if let HirCallDispatch::Direct { function } = call.dispatch()
                    && !call.type_arguments().is_empty()
                {
                    calls.push((*function, call.type_arguments().to_vec()));
                }
                for argument in call.arguments() {
                    collect_expression_calls(argument, calls);
                }
            }
        }
    }
}

fn collect_expression_calls(expression: &HirExpression, calls: &mut Vec<(SymbolId, Vec<TypeId>)>) {
    match expression.kind() {
        HirExpressionKind::Closure(closure) => collect_statement_calls(closure.body(), calls),
        HirExpressionKind::Field { base, .. }
        | HirExpressionKind::TupleGet { tuple: base, .. }
        | HirExpressionKind::InterfaceUpcast { value: base, .. }
        | HirExpressionKind::NumericConvert { value: base, .. }
        | HirExpressionKind::StringFormat { value: base, .. }
        | HirExpressionKind::Unary { operand: base, .. }
        | HirExpressionKind::ArrayLength { array: base } => collect_expression_calls(base, calls),
        HirExpressionKind::TableGet { table, key } => {
            collect_expression_calls(table, calls);
            collect_expression_calls(key, calls);
        }
        HirExpressionKind::ArrayGet { array, index }
        | HirExpressionKind::ArrayGetChecked { array, index }
        | HirExpressionKind::Binary {
            left: array,
            right: index,
            ..
        }
        | HirExpressionKind::StringConcat {
            left: array,
            right: index,
        } => {
            collect_expression_calls(array, calls);
            collect_expression_calls(index, calls);
        }
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            collect_expression_calls(length, calls);
            collect_expression_calls(initial_value, calls);
        }
        HirExpressionKind::ArrayFill { array, value } => {
            collect_expression_calls(array, calls);
            collect_expression_calls(value, calls);
        }
        HirExpressionKind::OptionalDefault { optional, fallback } => {
            collect_expression_calls(optional, calls);
            collect_expression_calls(fallback, calls);
        }
        HirExpressionKind::OptionalPropagate { optional, .. }
        | HirExpressionKind::OptionalNarrow { optional } => {
            collect_expression_calls(optional, calls);
        }
        HirExpressionKind::Record { fields, .. }
        | HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                collect_expression_calls(field.value(), calls);
            }
        }
        HirExpressionKind::RecordUpdate { base, fields, .. } => {
            collect_expression_calls(base, calls);
            for field in fields {
                collect_expression_calls(field.value(), calls);
            }
        }
        HirExpressionKind::Array(values)
        | HirExpressionKind::Tuple(values)
        | HirExpressionKind::UnionCase {
            arguments: values, ..
        } => {
            for value in values {
                collect_expression_calls(value, calls);
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                collect_expression_calls(entry.key(), calls);
                collect_expression_calls(entry.value(), calls);
            }
        }
        HirExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            collect_expression_calls(condition, calls);
            collect_expression_calls(when_true, calls);
            collect_expression_calls(when_false, calls);
        }
        HirExpressionKind::Call {
            dispatch,
            type_arguments,
            arguments,
        } => {
            if let HirCallDispatch::Direct { function } = dispatch
                && !type_arguments.is_empty()
            {
                calls.push((*function, type_arguments.clone()));
            }
            for argument in arguments {
                collect_expression_calls(argument, calls);
            }
        }
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::EnumCase { .. } => {}
    }
}

fn specialize_type(
    type_id: &mut TypeId,
    substitutions: &std::collections::BTreeMap<pop_foundation::ParameterId, TypeId>,
    arena: &pop_types::TypeArena,
) -> Option<()> {
    *type_id = arena.substitute_existing(*type_id, substitutions)?;
    Some(())
}

fn specialize_statements(
    statements: &mut [HirStatement],
    substitutions: &std::collections::BTreeMap<pop_foundation::ParameterId, TypeId>,
    instances: &std::collections::BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    arena: &pop_types::TypeArena,
) -> Option<()> {
    for statement in statements {
        specialize_statement(statement, substitutions, instances, arena)?;
    }
    Some(())
}

fn specialize_statement(
    statement: &mut HirStatement,
    substitutions: &std::collections::BTreeMap<pop_foundation::ParameterId, TypeId>,
    instances: &std::collections::BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    arena: &pop_types::TypeArena,
) -> Option<()> {
    match &mut statement.kind {
        HirStatementKind::Local {
            local_type,
            initializer,
            ..
        } => {
            specialize_type(local_type, substitutions, arena)?;
            specialize_expression(initializer, substitutions, instances, arena)?;
        }
        HirStatementKind::MultipleLocal { bindings, value } => {
            for binding in bindings {
                specialize_type(&mut binding.local_type, substitutions, arena)?;
            }
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::LocalSet { value, .. }
        | HirStatementKind::ParameterSet { value, .. }
        | HirStatementKind::CaptureSet { value, .. }
        | HirStatementKind::Expression(value) => {
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::Return { values } => {
            for value in values {
                specialize_expression(value, substitutions, instances, arena)?;
            }
        }
        HirStatementKind::If {
            condition,
            then_body,
            else_body,
        } => {
            specialize_expression(condition, substitutions, instances, arena)?;
            specialize_statements(then_body, substitutions, instances, arena)?;
            specialize_statements(else_body, substitutions, instances, arena)?;
        }
        HirStatementKind::OptionalIf {
            inner_type,
            initializer,
            then_body,
            else_body,
            ..
        } => {
            specialize_type(inner_type, substitutions, arena)?;
            specialize_expression(initializer, substitutions, instances, arena)?;
            specialize_statements(then_body, substitutions, instances, arena)?;
            specialize_statements(else_body, substitutions, instances, arena)?;
        }
        HirStatementKind::While { condition, body } => {
            specialize_expression(condition, substitutions, instances, arena)?;
            specialize_statements(body, substitutions, instances, arena)?;
        }
        HirStatementKind::OptionalWhile {
            inner_type,
            initializer,
            body,
            ..
        } => {
            specialize_type(inner_type, substitutions, arena)?;
            specialize_expression(initializer, substitutions, instances, arena)?;
            specialize_statements(body, substitutions, instances, arena)?;
        }
        HirStatementKind::RepeatUntil { body, condition } => {
            specialize_statements(body, substitutions, instances, arena)?;
            specialize_expression(condition, substitutions, instances, arena)?;
        }
        HirStatementKind::NumericFor {
            integer_type,
            first,
            last,
            step,
            body,
            ..
        } => {
            specialize_type(integer_type, substitutions, arena)?;
            specialize_expression(first, substitutions, instances, arena)?;
            specialize_expression(last, substitutions, instances, arena)?;
            specialize_expression(step, substitutions, instances, arena)?;
            specialize_statements(body, substitutions, instances, arena)?;
        }
        HirStatementKind::Break | HirStatementKind::Continue => {}
        HirStatementKind::Match {
            scrutinee, arms, ..
        } => {
            specialize_expression(scrutinee, substitutions, instances, arena)?;
            for arm in arms {
                for binding in &mut arm.bindings {
                    specialize_type(&mut binding.type_id, substitutions, arena)?;
                }
                specialize_statements(&mut arm.body, substitutions, instances, arena)?;
            }
        }
        HirStatementKind::FieldSet { base, value, .. } => {
            specialize_expression(base, substitutions, instances, arena)?;
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::CompoundFieldSet {
            base,
            value_type,
            value,
            ..
        } => {
            specialize_type(value_type, substitutions, arena)?;
            specialize_expression(base, substitutions, instances, arena)?;
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::ArraySet {
            array,
            index,
            value,
        } => {
            specialize_expression(array, substitutions, instances, arena)?;
            specialize_expression(index, substitutions, instances, arena)?;
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::CompoundArraySet {
            array,
            index,
            element_type,
            value,
            ..
        } => {
            specialize_type(element_type, substitutions, arena)?;
            specialize_expression(array, substitutions, instances, arena)?;
            specialize_expression(index, substitutions, instances, arena)?;
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::TableSet { table, key, value } => {
            specialize_expression(table, substitutions, instances, arena)?;
            specialize_expression(key, substitutions, instances, arena)?;
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::MultipleAssignment { targets, value } => {
            for target in targets {
                specialize_assignment_target(target, substitutions, instances, arena)?;
            }
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirStatementKind::Call(call) => specialize_call(call, substitutions, instances, arena)?,
    }
    Some(())
}

fn specialize_assignment_target(
    target: &mut HirAssignmentTarget,
    substitutions: &std::collections::BTreeMap<pop_foundation::ParameterId, TypeId>,
    instances: &std::collections::BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    arena: &pop_types::TypeArena,
) -> Option<()> {
    match target {
        HirAssignmentTarget::Local { value_type, .. }
        | HirAssignmentTarget::Capture { value_type, .. } => {
            specialize_type(value_type, substitutions, arena)?;
        }
        HirAssignmentTarget::Field {
            base, value_type, ..
        } => {
            specialize_type(value_type, substitutions, arena)?;
            specialize_expression(base, substitutions, instances, arena)?;
        }
        HirAssignmentTarget::Array {
            array,
            index,
            element_type,
        } => {
            specialize_type(element_type, substitutions, arena)?;
            specialize_expression(array, substitutions, instances, arena)?;
            specialize_expression(index, substitutions, instances, arena)?;
        }
        HirAssignmentTarget::Table {
            table,
            key,
            value_type,
        } => {
            specialize_type(value_type, substitutions, arena)?;
            specialize_expression(table, substitutions, instances, arena)?;
            specialize_expression(key, substitutions, instances, arena)?;
        }
    }
    Some(())
}

fn specialize_call(
    call: &mut HirCall,
    substitutions: &std::collections::BTreeMap<pop_foundation::ParameterId, TypeId>,
    instances: &std::collections::BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    arena: &pop_types::TypeArena,
) -> Option<()> {
    for argument in &mut call.type_arguments {
        specialize_type(argument, substitutions, arena)?;
    }
    if let HirCallDispatch::Direct { function } = &mut call.dispatch
        && !call.type_arguments.is_empty()
        && let Some(instance) = instances.get(&(*function, call.type_arguments.clone()))
    {
        *function = *instance;
        call.type_arguments.clear();
    }
    if let HirCallDispatch::Indirect { callee } = &mut call.dispatch {
        specialize_expression(callee, substitutions, instances, arena)?;
    }
    for argument in &mut call.arguments {
        specialize_expression(argument, substitutions, instances, arena)?;
    }
    Some(())
}

fn specialize_expression(
    expression: &mut HirExpression,
    substitutions: &std::collections::BTreeMap<pop_foundation::ParameterId, TypeId>,
    instances: &std::collections::BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    arena: &pop_types::TypeArena,
) -> Option<()> {
    specialize_type(&mut expression.type_id, substitutions, arena)?;
    match &mut expression.kind {
        HirExpressionKind::Closure(closure) => {
            for parameter in &mut closure.parameters {
                specialize_type(&mut parameter.type_id, substitutions, arena)?;
            }
            for result in &mut closure.results {
                specialize_type(result, substitutions, arena)?;
            }
            for capture in &mut closure.captures {
                specialize_type(&mut capture.type_id, substitutions, arena)?;
            }
            specialize_statements(&mut closure.body, substitutions, instances, arena)?;
        }
        HirExpressionKind::Field { base, .. }
        | HirExpressionKind::TupleGet { tuple: base, .. }
        | HirExpressionKind::InterfaceUpcast { value: base, .. }
        | HirExpressionKind::NumericConvert { value: base, .. }
        | HirExpressionKind::StringFormat { value: base, .. }
        | HirExpressionKind::Unary { operand: base, .. }
        | HirExpressionKind::ArrayLength { array: base } => {
            specialize_expression(base, substitutions, instances, arena)?;
        }
        HirExpressionKind::TableGet { table, key } => {
            specialize_expression(table, substitutions, instances, arena)?;
            specialize_expression(key, substitutions, instances, arena)?;
        }
        HirExpressionKind::ArrayGet { array, index }
        | HirExpressionKind::ArrayGetChecked { array, index }
        | HirExpressionKind::Binary {
            left: array,
            right: index,
            ..
        }
        | HirExpressionKind::StringConcat {
            left: array,
            right: index,
        } => {
            specialize_expression(array, substitutions, instances, arena)?;
            specialize_expression(index, substitutions, instances, arena)?;
        }
        HirExpressionKind::ArrayCreate {
            length,
            initial_value,
        } => {
            specialize_expression(length, substitutions, instances, arena)?;
            specialize_expression(initial_value, substitutions, instances, arena)?;
        }
        HirExpressionKind::ArrayFill { array, value } => {
            specialize_expression(array, substitutions, instances, arena)?;
            specialize_expression(value, substitutions, instances, arena)?;
        }
        HirExpressionKind::OptionalDefault { optional, fallback } => {
            specialize_expression(optional, substitutions, instances, arena)?;
            specialize_expression(fallback, substitutions, instances, arena)?;
        }
        HirExpressionKind::OptionalPropagate {
            optional,
            enclosing_result,
        } => {
            specialize_type(enclosing_result, substitutions, arena)?;
            specialize_expression(optional, substitutions, instances, arena)?;
        }
        HirExpressionKind::OptionalNarrow { optional } => {
            specialize_expression(optional, substitutions, instances, arena)?;
        }
        HirExpressionKind::Record { fields, .. }
        | HirExpressionKind::ClassConstruct { fields, .. } => {
            for field in fields {
                specialize_expression(&mut field.value, substitutions, instances, arena)?;
            }
        }
        HirExpressionKind::RecordUpdate { base, fields, .. } => {
            specialize_expression(base, substitutions, instances, arena)?;
            for field in fields {
                specialize_expression(&mut field.value, substitutions, instances, arena)?;
            }
        }
        HirExpressionKind::Array(values)
        | HirExpressionKind::Tuple(values)
        | HirExpressionKind::UnionCase {
            arguments: values, ..
        } => {
            for value in values {
                specialize_expression(value, substitutions, instances, arena)?;
            }
        }
        HirExpressionKind::Table(entries) => {
            for entry in entries {
                specialize_expression(&mut entry.key, substitutions, instances, arena)?;
                specialize_expression(&mut entry.value, substitutions, instances, arena)?;
            }
        }
        HirExpressionKind::Conditional {
            condition,
            when_true,
            when_false,
        } => {
            specialize_expression(condition, substitutions, instances, arena)?;
            specialize_expression(when_true, substitutions, instances, arena)?;
            specialize_expression(when_false, substitutions, instances, arena)?;
        }
        HirExpressionKind::Call {
            dispatch,
            type_arguments,
            arguments,
        } => {
            for argument in type_arguments.iter_mut() {
                specialize_type(argument, substitutions, arena)?;
            }
            if let HirCallDispatch::Direct { function } = dispatch
                && !type_arguments.is_empty()
                && let Some(instance) = instances.get(&(*function, type_arguments.clone()))
            {
                *function = *instance;
                type_arguments.clear();
            }
            if let HirCallDispatch::Indirect { callee } = dispatch {
                specialize_expression(callee, substitutions, instances, arena)?;
            }
            for argument in arguments {
                specialize_expression(argument, substitutions, instances, arena)?;
            }
        }
        HirExpressionKind::Integer(_)
        | HirExpressionKind::Float(_)
        | HirExpressionKind::String(_)
        | HirExpressionKind::Boolean(_)
        | HirExpressionKind::Nil
        | HirExpressionKind::Local(_)
        | HirExpressionKind::Parameter(_)
        | HirExpressionKind::Capture(_)
        | HirExpressionKind::Function(_)
        | HirExpressionKind::EnumCase { .. } => {}
    }
    Some(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirAttribute {
    pub(crate) attribute: AttributeId,
    pub(crate) definition: SymbolId,
    pub(crate) arguments: Vec<HirAttributeArgument>,
    pub(crate) span: SourceSpan,
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
    pub(crate) name: String,
    pub(crate) value: AttributeConstant,
    pub(crate) value_type: TypeId,
    pub(crate) origin: SourceSpan,
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
    pub(crate) binding: BindingId,
    pub(crate) parameter: ValueParameterId,
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
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
    pub(crate) kind: HirStatementKind,
    pub(crate) span: SourceSpan,
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
    MultipleLocal {
        bindings: Vec<HirLocalBinding>,
        value: HirExpression,
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
    OptionalIf {
        binding: BindingId,
        local: LocalId,
        name: String,
        inner_type: TypeId,
        initializer: HirExpression,
        then_body: Vec<HirStatement>,
        else_body: Vec<HirStatement>,
    },
    While {
        condition: HirExpression,
        body: Vec<HirStatement>,
    },
    OptionalWhile {
        binding: BindingId,
        local: LocalId,
        name: String,
        inner_type: TypeId,
        initializer: HirExpression,
        body: Vec<HirStatement>,
    },
    RepeatUntil {
        body: Vec<HirStatement>,
        condition: HirExpression,
    },
    NumericFor {
        binding: BindingId,
        local: LocalId,
        name: String,
        integer_type: TypeId,
        first: HirExpression,
        last: HirExpression,
        step: HirExpression,
        body: Vec<HirStatement>,
    },
    Break,
    Continue,
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
    CompoundFieldSet {
        base: HirExpression,
        field: FieldId,
        value_type: TypeId,
        operator: TypedCompoundOperator,
        value: HirExpression,
    },
    ArraySet {
        array: HirExpression,
        index: HirExpression,
        value: HirExpression,
    },
    TableSet {
        table: HirExpression,
        key: HirExpression,
        value: HirExpression,
    },
    CompoundArraySet {
        array: HirExpression,
        index: HirExpression,
        element_type: TypeId,
        operator: TypedCompoundOperator,
        value: HirExpression,
    },
    MultipleAssignment {
        targets: Vec<HirAssignmentTarget>,
        value: HirExpression,
    },
    Call(HirCall),
    Expression(HirExpression),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirLocalBinding {
    pub(crate) binding: BindingId,
    pub(crate) local: LocalId,
    pub(crate) name: String,
    pub(crate) local_type: TypeId,
    pub(crate) span: SourceSpan,
}

impl HirLocalBinding {
    #[must_use]
    pub const fn binding(&self) -> BindingId {
        self.binding
    }

    #[must_use]
    pub const fn local(&self) -> LocalId {
        self.local
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
pub enum HirAssignmentTarget {
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
        base: HirExpression,
        field: FieldId,
        value_type: TypeId,
    },
    Array {
        array: HirExpression,
        index: HirExpression,
        element_type: TypeId,
    },
    Table {
        table: HirExpression,
        key: HirExpression,
        value_type: TypeId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirMatchArm {
    pub(crate) union: SymbolId,
    pub(crate) case: UnionCaseId,
    pub(crate) bindings: Vec<HirMatchBinding>,
    pub(crate) body: Vec<HirStatement>,
    pub(crate) span: SourceSpan,
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
    pub(crate) binding: Option<BindingId>,
    pub(crate) local: Option<LocalId>,
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
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
    pub(crate) dispatch: HirCallDispatch,
    pub(crate) type_arguments: Vec<TypeId>,
    pub(crate) arguments: Vec<HirExpression>,
    pub(crate) span: SourceSpan,
}

impl HirCall {
    #[must_use]
    pub const fn dispatch(&self) -> &HirCallDispatch {
        &self.dispatch
    }

    #[must_use]
    pub fn type_arguments(&self) -> &[TypeId] {
        &self.type_arguments
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
    pub(crate) kind: HirExpressionKind,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
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
    TableGet {
        table: Box<HirExpression>,
        key: Box<HirExpression>,
    },
    TupleGet {
        tuple: Box<HirExpression>,
        index: u32,
    },
    ArrayCreate {
        length: Box<HirExpression>,
        initial_value: Box<HirExpression>,
    },
    ArrayLength {
        array: Box<HirExpression>,
    },
    ArrayGetChecked {
        array: Box<HirExpression>,
        index: Box<HirExpression>,
    },
    ArrayFill {
        array: Box<HirExpression>,
        value: Box<HirExpression>,
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
    EnumCase {
        definition: SymbolId,
        case: EnumCaseId,
        discriminant: u32,
    },
    Tuple(Vec<HirExpression>),
    StringConcat {
        left: Box<HirExpression>,
        right: Box<HirExpression>,
    },
    StringFormat {
        kind: StringFormatKind,
        value: Box<HirExpression>,
    },
    Unary {
        operator: TypedUnaryOperator,
        operand: Box<HirExpression>,
    },
    Binary {
        operator: TypedBinaryOperator,
        left: Box<HirExpression>,
        right: Box<HirExpression>,
    },
    OptionalDefault {
        optional: Box<HirExpression>,
        fallback: Box<HirExpression>,
    },
    OptionalPropagate {
        optional: Box<HirExpression>,
        enclosing_result: TypeId,
    },
    OptionalNarrow {
        optional: Box<HirExpression>,
    },
    Conditional {
        condition: Box<HirExpression>,
        when_true: Box<HirExpression>,
        when_false: Box<HirExpression>,
    },
    Call {
        dispatch: HirCallDispatch,
        type_arguments: Vec<TypeId>,
        arguments: Vec<HirExpression>,
    },
    InterfaceUpcast {
        value: Box<HirExpression>,
        interface: InterfaceId,
    },
    NumericConvert {
        value: Box<HirExpression>,
        conversion: NumericConversionKind,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HirClosure {
    pub(crate) function: NestedFunctionId,
    pub(crate) parameters: Vec<HirClosureParameter>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) captures: Vec<HirCapture>,
    pub(crate) body: Vec<HirStatement>,
    pub(crate) span: SourceSpan,
    pub(crate) effects: pop_types::EffectSummary,
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
    pub(crate) binding: BindingId,
    pub(crate) parameter: ValueParameterId,
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
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
    pub(crate) capture: CaptureId,
    pub(crate) binding: BindingId,
    pub(crate) source: HirCaptureSource,
    pub(crate) type_id: TypeId,
    pub(crate) mode: HirCaptureMode,
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
    pub(crate) key: HirExpression,
    pub(crate) value: HirExpression,
    pub(crate) span: SourceSpan,
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
    pub(crate) field: FieldId,
    pub(crate) value: HirExpression,
    pub(crate) span: SourceSpan,
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
