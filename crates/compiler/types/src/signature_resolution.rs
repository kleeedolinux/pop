use std::collections::BTreeMap;

use pop_diagnostics::types as type_diagnostics;
use pop_foundation::{
    BuiltinTypeId, ClassId, Diagnostic, EnumCaseId, ErrorCaseId, ErrorId, FieldId, InterfaceId,
    MethodId, ModuleId, ParameterId, SourceSpan, SymbolId, TypeId, UnionCaseId,
};
use pop_resolve::{ResolutionDatabase, SymbolSpace};
use pop_syntax::{
    EnumDeclarationSyntax, ErrorDeclarationSyntax, FunctionSignatureSyntax, GenericParameterSyntax,
    RecordDeclarationSyntax, TypeAliasDeclarationSyntax, TypeSyntax, TypeSyntaxKind,
    UnionDeclarationSyntax,
};

use crate::field_defaults::resolve_field_default;
use crate::required_constants::field_default_matches_type;
use crate::{
    BootstrapSchema, BootstrapTypeRole, FieldDefault, PendingConstantExpression,
    RequiredConstantError, RequiredConstantTarget, SemanticType, TypeArena,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedType {
    kind: ResolvedTypeKind,
    type_id: Option<TypeId>,
    span: SourceSpan,
}

impl ResolvedType {
    pub(crate) const fn canonical(type_id: TypeId, span: SourceSpan) -> Self {
        Self {
            kind: ResolvedTypeKind::Primitive,
            type_id: Some(type_id),
            span,
        }
    }

    #[must_use]
    pub const fn kind(&self) -> &ResolvedTypeKind {
        &self.kind
    }

    #[must_use]
    pub const fn type_id(&self) -> Option<TypeId> {
        self.type_id
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ResolvedTypeKind {
    Primitive,
    TypeParameter {
        parameter: ParameterId,
    },
    Builtin {
        definition: BuiltinTypeId,
        arguments: Vec<ResolvedType>,
    },
    Declaration {
        symbol: SymbolId,
        arguments: Vec<ResolvedType>,
    },
    Array(Box<ResolvedType>),
    Table {
        key: Box<ResolvedType>,
        value: Box<ResolvedType>,
    },
    Tuple(Vec<ResolvedType>),
    Union(Vec<ResolvedType>),
    Optional(Box<ResolvedType>),
    Function {
        is_async: bool,
        parameters: Vec<ResolvedType>,
        results: Vec<ResolvedType>,
        effects: crate::EffectSummary,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedTypeParameter {
    parameter: ParameterId,
    name: String,
    type_id: TypeId,
    bound: Option<TypeId>,
    span: SourceSpan,
}

impl ResolvedTypeParameter {
    #[must_use]
    pub const fn parameter(&self) -> ParameterId {
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
    pub const fn bound(&self) -> Option<TypeId> {
        self.bound
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedFunctionParameter {
    name: String,
    parameter_type: ResolvedType,
    span: SourceSpan,
}

impl ResolvedFunctionParameter {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn parameter_type(&self) -> &ResolvedType {
        &self.parameter_type
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedFunctionSignature {
    symbol: SymbolId,
    name: String,
    is_async: bool,
    type_parameters: Vec<ResolvedTypeParameter>,
    parameters: Vec<ResolvedFunctionParameter>,
    results: Vec<ResolvedType>,
    effects: crate::EffectSummary,
}

impl ResolvedFunctionSignature {
    pub(crate) fn canonical(
        symbol: SymbolId,
        name: String,
        parameters: Vec<(String, TypeId, SourceSpan)>,
        results: Vec<(TypeId, SourceSpan)>,
    ) -> Self {
        Self::canonical_generic(symbol, name, Vec::new(), parameters, results)
    }

    pub(crate) fn canonical_generic(
        symbol: SymbolId,
        name: String,
        type_parameters: Vec<ResolvedTypeParameter>,
        parameters: Vec<(String, TypeId, SourceSpan)>,
        results: Vec<(TypeId, SourceSpan)>,
    ) -> Self {
        Self {
            symbol,
            name,
            is_async: false,
            type_parameters,
            parameters: parameters
                .into_iter()
                .map(|(name, type_id, span)| ResolvedFunctionParameter {
                    name,
                    parameter_type: ResolvedType::canonical(type_id, span),
                    span,
                })
                .collect(),
            results: results
                .into_iter()
                .map(|(type_id, span)| ResolvedType::canonical(type_id, span))
                .collect(),
            effects: crate::EffectSummary::empty(),
        }
    }

    pub(crate) const fn with_async(mut self, is_async: bool) -> Self {
        self.is_async = is_async;
        self
    }

    #[must_use]
    pub const fn with_effects(mut self, effects: crate::EffectSummary) -> Self {
        self.effects = effects;
        self
    }

    /// Rehydrates one already-verified public reference signature in the
    /// consumer's isolated type arena.
    #[must_use]
    pub fn referenced(
        symbol: SymbolId,
        name: impl Into<String>,
        parameters: Vec<(String, TypeId, SourceSpan)>,
        results: Vec<(TypeId, SourceSpan)>,
        effects: crate::EffectSummary,
    ) -> Self {
        let mut signature = Self::canonical(symbol, name.into(), parameters, results);
        signature.effects = effects;
        signature
    }

    /// Rehydrates one verified generic reference signature.
    #[must_use]
    pub fn referenced_generic(
        symbol: SymbolId,
        name: impl Into<String>,
        is_async: bool,
        type_parameters: Vec<ResolvedTypeParameter>,
        parameters: Vec<(String, TypeId, SourceSpan)>,
        results: Vec<(TypeId, SourceSpan)>,
        effects: crate::EffectSummary,
    ) -> Self {
        let mut signature = Self::canonical(symbol, name.into(), parameters, results);
        signature.is_async = is_async;
        signature.type_parameters = type_parameters;
        signature.effects = effects;
        signature
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn is_async(&self) -> bool {
        self.is_async
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[ResolvedTypeParameter] {
        &self.type_parameters
    }

    #[must_use]
    pub fn parameters(&self) -> &[ResolvedFunctionParameter] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[ResolvedType] {
        &self.results
    }

    #[must_use]
    pub const fn effects(&self) -> crate::EffectSummary {
        self.effects
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedSignatureResult {
    signature: Option<ResolvedFunctionSignature>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordDefinition {
    symbol: SymbolId,
    type_id: TypeId,
    fields: Vec<RecordFieldDefinition>,
    span: SourceSpan,
}

impl RecordDefinition {
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn fields(&self) -> &[RecordFieldDefinition] {
        &self.fields
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecordFieldDefinition {
    field: FieldId,
    name: String,
    field_type: TypeId,
    default: Option<FieldDefault>,
    pending_default: Option<PendingConstantExpression>,
    span: SourceSpan,
}

impl RecordFieldDefinition {
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
    pub const fn pending_default(&self) -> Option<&PendingConstantExpression> {
        self.pending_default.as_ref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug)]
pub struct RecordDefinitionResult {
    definition: Option<RecordDefinition>,
    diagnostics: Vec<Diagnostic>,
}

impl RecordDefinitionResult {
    #[must_use]
    pub const fn definition(&self) -> Option<&RecordDefinition> {
        self.definition.as_ref()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        diagnostic_snapshot(&self.diagnostics)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnionDefinition {
    symbol: SymbolId,
    type_id: TypeId,
    cases: Vec<UnionCaseDefinition>,
    span: SourceSpan,
}

impl UnionDefinition {
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[UnionCaseDefinition] {
        &self.cases
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UnionCaseDefinition {
    case: UnionCaseId,
    name: String,
    parameters: Vec<(String, TypeId, SourceSpan)>,
    span: SourceSpan,
}

impl UnionCaseDefinition {
    #[must_use]
    pub const fn case(&self) -> UnionCaseId {
        self.case
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parameters(&self) -> &[(String, TypeId, SourceSpan)] {
        &self.parameters
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug)]
pub struct UnionDefinitionResult {
    definition: Option<UnionDefinition>,
    diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorDefinition {
    error: ErrorId,
    symbol: SymbolId,
    type_id: TypeId,
    cases: Vec<ErrorCaseDefinition>,
    span: SourceSpan,
}

impl ErrorDefinition {
    #[must_use]
    pub const fn error(&self) -> ErrorId {
        self.error
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[ErrorCaseDefinition] {
        &self.cases
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ErrorCaseDefinition {
    case: ErrorCaseId,
    name: String,
    parameters: Vec<(String, TypeId, SourceSpan)>,
    span: SourceSpan,
}

impl ErrorCaseDefinition {
    #[must_use]
    pub const fn case(&self) -> ErrorCaseId {
        self.case
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parameters(&self) -> &[(String, TypeId, SourceSpan)] {
        &self.parameters
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug)]
pub struct ErrorDefinitionResult {
    definition: Option<ErrorDefinition>,
    diagnostics: Vec<Diagnostic>,
}

impl ErrorDefinitionResult {
    #[must_use]
    pub const fn definition(&self) -> Option<&ErrorDefinition> {
        self.definition.as_ref()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        diagnostic_snapshot(&self.diagnostics)
    }
}

impl UnionDefinitionResult {
    #[must_use]
    pub const fn definition(&self) -> Option<&UnionDefinition> {
        self.definition.as_ref()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        diagnostic_snapshot(&self.diagnostics)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumDefinition {
    symbol: SymbolId,
    type_id: TypeId,
    cases: Vec<EnumCaseDefinition>,
    span: SourceSpan,
}

impl EnumDefinition {
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[EnumCaseDefinition] {
        &self.cases
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnumCaseDefinition {
    case: EnumCaseId,
    name: String,
    discriminant: u32,
    span: SourceSpan,
}

impl EnumCaseDefinition {
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

#[derive(Clone, Debug)]
pub struct EnumDefinitionResult {
    definition: Option<EnumDefinition>,
    diagnostics: Vec<Diagnostic>,
}

impl EnumDefinitionResult {
    #[must_use]
    pub const fn definition(&self) -> Option<&EnumDefinition> {
        self.definition.as_ref()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }
}

impl ResolvedSignatureResult {
    #[must_use]
    pub const fn signature(&self) -> Option<&ResolvedFunctionSignature> {
        self.signature.as_ref()
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

pub struct SignatureResolver<'index> {
    database: &'index ResolutionDatabase,
    schema: BootstrapSchema,
    has_ffi_dependency: bool,
    pub(crate) arena: TypeArena,
    next_parameter: u32,
    pub(crate) next_field: u32,
    next_union_case: u32,
    next_error: u32,
    next_error_case: u32,
    next_enum_case: u32,
    pub(crate) next_class: u32,
    pub(crate) next_method: u32,
    pub(crate) next_interface: u32,
    pub(crate) next_interface_method: u32,
    pub(crate) next_attribute: u32,
    pub(crate) next_instance_symbol: u32,
    record_definitions: BTreeMap<SymbolId, RecordDefinition>,
    record_type_parameters: BTreeMap<SymbolId, Vec<ResolvedTypeParameter>>,
    record_instances: BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    records_by_type: BTreeMap<TypeId, SymbolId>,
    structural_record_fields: BTreeMap<(String, TypeId), FieldId>,
    union_definitions: BTreeMap<SymbolId, UnionDefinition>,
    unions_by_type: BTreeMap<TypeId, SymbolId>,
    union_type_parameters: BTreeMap<SymbolId, Vec<ResolvedTypeParameter>>,
    union_instances: BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    union_instance_sources: BTreeMap<SymbolId, SymbolId>,
    error_definitions: BTreeMap<SymbolId, ErrorDefinition>,
    errors_by_type: BTreeMap<TypeId, SymbolId>,
    error_type_parameters: BTreeMap<SymbolId, Vec<ResolvedTypeParameter>>,
    error_instances: BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    enum_definitions: BTreeMap<SymbolId, EnumDefinition>,
    pub(crate) class_types: BTreeMap<SymbolId, TypeId>,
    pub(crate) class_type_parameters: BTreeMap<SymbolId, Vec<ResolvedTypeParameter>>,
    pub(crate) class_instances: BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    pub(crate) class_instance_sources: BTreeMap<SymbolId, SymbolId>,
    pub(crate) generic_call_substitutions: Vec<BTreeMap<ParameterId, TypeId>>,
    pub(crate) active_class_specializations: BTreeMap<(ClassId, Vec<TypeId>), TypeId>,
    pub(crate) class_definitions: BTreeMap<SymbolId, crate::ClassDefinition>,
    pub(crate) classes_by_type: BTreeMap<TypeId, SymbolId>,
    pub(crate) interface_types: BTreeMap<SymbolId, TypeId>,
    pub(crate) interface_type_parameters: BTreeMap<SymbolId, Vec<ResolvedTypeParameter>>,
    pub(crate) interface_instances: BTreeMap<(SymbolId, Vec<TypeId>), SymbolId>,
    pub(crate) interface_instance_sources: BTreeMap<SymbolId, SymbolId>,
    pub(crate) interface_sources_by_id: BTreeMap<InterfaceId, SymbolId>,
    pub(crate) interface_definitions: BTreeMap<SymbolId, crate::InterfaceDefinition>,
    type_aliases: BTreeMap<SymbolId, (ModuleId, TypeSyntax)>,
    resolving_aliases: BTreeMap<SymbolId, SourceSpan>,
    pub(crate) interfaces_by_type: BTreeMap<TypeId, SymbolId>,
    pub(crate) attribute_definitions: BTreeMap<SymbolId, crate::AttributeDefinition>,
}

impl<'index> SignatureResolver<'index> {
    #[must_use]
    pub fn new(database: &'index ResolutionDatabase, schema: BootstrapSchema) -> Self {
        let next_instance_symbol = database
            .index()
            .declarations()
            .map(pop_resolve::Declaration::symbol)
            .map(SymbolId::raw)
            .max()
            .unwrap_or(0)
            .saturating_add(1);
        Self {
            database,
            schema,
            has_ffi_dependency: false,
            arena: TypeArena::new(),
            next_parameter: 0,
            next_field: 0,
            next_union_case: 0,
            next_error: 0,
            next_error_case: 0,
            next_enum_case: 0,
            next_class: 0,
            next_method: 0,
            next_interface: 0,
            next_interface_method: 0,
            next_attribute: 0,
            next_instance_symbol,
            record_definitions: BTreeMap::new(),
            record_type_parameters: BTreeMap::new(),
            record_instances: BTreeMap::new(),
            records_by_type: BTreeMap::new(),
            structural_record_fields: BTreeMap::new(),
            union_definitions: BTreeMap::new(),
            unions_by_type: BTreeMap::new(),
            union_type_parameters: BTreeMap::new(),
            union_instances: BTreeMap::new(),
            union_instance_sources: BTreeMap::new(),
            error_definitions: BTreeMap::new(),
            errors_by_type: BTreeMap::new(),
            error_type_parameters: BTreeMap::new(),
            error_instances: BTreeMap::new(),
            enum_definitions: BTreeMap::new(),
            class_types: BTreeMap::new(),
            class_type_parameters: BTreeMap::new(),
            class_instances: BTreeMap::new(),
            class_instance_sources: BTreeMap::new(),
            generic_call_substitutions: Vec::new(),
            active_class_specializations: BTreeMap::new(),
            class_definitions: BTreeMap::new(),
            classes_by_type: BTreeMap::new(),
            interface_types: BTreeMap::new(),
            interface_type_parameters: BTreeMap::new(),
            interface_instances: BTreeMap::new(),
            interface_instance_sources: BTreeMap::new(),
            interface_sources_by_id: BTreeMap::new(),
            interface_definitions: BTreeMap::new(),
            type_aliases: BTreeMap::new(),
            resolving_aliases: BTreeMap::new(),
            interfaces_by_type: BTreeMap::new(),
            attribute_definitions: BTreeMap::new(),
        }
    }

    /// Enables compiler-owned `Pop.Ffi` types after the caller verifies an
    /// explicit dependency on the reserved `Pop.Ffi` Bubble identity.
    #[must_use]
    pub const fn with_ffi_dependency(mut self) -> Self {
        self.has_ffi_dependency = true;
        self
    }

    #[must_use]
    pub const fn has_ffi_dependency(&self) -> bool {
        self.has_ffi_dependency
    }

    #[must_use]
    pub const fn arena(&self) -> &TypeArena {
        &self.arena
    }

    #[doc(hidden)]
    pub fn allocate_capsule_class(&mut self) -> ClassId {
        let id = ClassId::from_raw(self.next_class);
        self.next_class = self.next_class.saturating_add(1);
        id
    }

    #[doc(hidden)]
    pub fn allocate_capsule_field(&mut self) -> FieldId {
        let id = FieldId::from_raw(self.next_field);
        self.next_field = self.next_field.saturating_add(1);
        id
    }

    #[doc(hidden)]
    pub fn allocate_capsule_method(&mut self) -> MethodId {
        let id = MethodId::from_raw(self.next_method);
        self.next_method = self.next_method.saturating_add(1);
        id
    }

    #[doc(hidden)]
    pub fn reserve_capsule_identifiers(
        &mut self,
        next_class: u32,
        next_field: u32,
        next_method: u32,
    ) {
        self.next_class = self.next_class.max(next_class);
        self.next_field = self.next_field.max(next_field);
        self.next_method = self.next_method.max(next_method);
    }

    pub(crate) const fn schema(&self) -> &BootstrapSchema {
        &self.schema
    }

    #[must_use]
    pub fn result_parts(&self, type_id: TypeId) -> Option<(TypeId, TypeId)> {
        let result = self.schema.type_by_source_name("Result")?;
        match self.arena.get(type_id)? {
            SemanticType::Builtin {
                definition,
                arguments,
            } if *definition == result.id() && arguments.len() == 2 => {
                Some((arguments[0], arguments[1]))
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn result_definition(&self) -> Option<BuiltinTypeId> {
        Some(self.schema.type_by_source_name("Result")?.id())
    }

    pub(crate) fn result_type(&mut self, success: TypeId, error: TypeId) -> Option<TypeId> {
        let definition = self.result_definition()?;
        self.arena
            .intern(SemanticType::Builtin {
                definition,
                arguments: vec![success, error],
            })
            .ok()
    }

    #[must_use]
    pub fn into_arena(self) -> TypeArena {
        self.arena
    }

    pub(crate) const fn database(&self) -> &ResolutionDatabase {
        self.database
    }

    pub fn arena_mut(&mut self) -> &mut TypeArena {
        &mut self.arena
    }

    /// Allocates one metadata-owned type parameter in this isolated analysis
    /// session. Published identity remains its ordered metadata position.
    #[must_use]
    pub fn referenced_type_parameter(
        &mut self,
        name: impl Into<String>,
        bound: Option<TypeId>,
        span: SourceSpan,
    ) -> ResolvedTypeParameter {
        let parameter = ParameterId::from_raw(self.next_parameter);
        self.next_parameter = self.next_parameter.saturating_add(1);
        let type_id = self
            .arena
            .intern(SemanticType::TypeParameter(parameter))
            .expect("fresh referenced type parameter is canonical");
        ResolvedTypeParameter {
            parameter,
            name: name.into(),
            type_id,
            bound,
            span,
        }
    }

    pub fn substitute_type_parameters(
        &mut self,
        type_id: TypeId,
        substitutions: &BTreeMap<ParameterId, TypeId>,
    ) -> Option<TypeId> {
        let semantic = self.arena.get(type_id)?.clone();
        if let SemanticType::TypeParameter(parameter) = semantic {
            return substitutions.get(&parameter).copied();
        }
        let substituted = match semantic {
            SemanticType::Primitive(_)
            | SemanticType::Enum { .. }
            | SemanticType::Opaque(_)
            | SemanticType::Error => return Some(type_id),
            SemanticType::TypeParameter(_) => unreachable!("handled above"),
            SemanticType::TaggedUnion {
                definition,
                source,
                arguments,
            } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| self.substitute_type_parameters(argument, substitutions))
                    .collect::<Option<Vec<_>>>()?;
                if self.union_type_parameters.contains_key(&source) {
                    return self
                        .instantiate_union(source, &arguments)
                        .map(|instance| instance.type_id());
                }
                SemanticType::TaggedUnion {
                    definition,
                    source,
                    arguments,
                }
            }
            SemanticType::ErrorUnion {
                definition,
                source,
                arguments,
            } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| self.substitute_type_parameters(argument, substitutions))
                    .collect::<Option<Vec<_>>>()?;
                if self.error_type_parameters.contains_key(&source) {
                    return self
                        .instantiate_error(source, &arguments)
                        .map(|instance| instance.type_id());
                }
                SemanticType::ErrorUnion {
                    definition,
                    source,
                    arguments,
                }
            }
            SemanticType::Tuple(elements) => SemanticType::Tuple(
                elements
                    .into_iter()
                    .map(|element| self.substitute_type_parameters(element, substitutions))
                    .collect::<Option<_>>()?,
            ),
            SemanticType::Union(elements) => SemanticType::Union(
                elements
                    .into_iter()
                    .map(|element| self.substitute_type_parameters(element, substitutions))
                    .collect::<Option<_>>()?,
            ),
            SemanticType::Record(fields) => SemanticType::Record(
                fields
                    .into_iter()
                    .map(|(name, field_type)| {
                        Some((
                            name,
                            self.substitute_type_parameters(field_type, substitutions)?,
                        ))
                    })
                    .collect::<Option<_>>()?,
            ),
            SemanticType::Array(element) => {
                SemanticType::Array(self.substitute_type_parameters(element, substitutions)?)
            }
            SemanticType::Table { key, value } => SemanticType::Table {
                key: self.substitute_type_parameters(key, substitutions)?,
                value: self.substitute_type_parameters(value, substitutions)?,
            },
            SemanticType::Optional(element) => {
                SemanticType::Optional(self.substitute_type_parameters(element, substitutions)?)
            }
            SemanticType::Function {
                is_async,
                parameters,
                results,
                effects,
            } => SemanticType::Function {
                is_async,
                parameters: parameters
                    .into_iter()
                    .map(|parameter| self.substitute_type_parameters(parameter, substitutions))
                    .collect::<Option<_>>()?,
                results: results
                    .into_iter()
                    .map(|result| self.substitute_type_parameters(result, substitutions))
                    .collect::<Option<_>>()?,
                effects,
            },
            SemanticType::Class { class, arguments } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| self.substitute_type_parameters(argument, substitutions))
                    .collect::<Option<Vec<_>>>()?;
                if let Some(type_id) = self
                    .active_class_specializations
                    .get(&(class, arguments.clone()))
                    .copied()
                {
                    return Some(type_id);
                }
                SemanticType::Class { class, arguments }
            }
            SemanticType::Interface {
                interface,
                arguments,
            } => {
                let arguments = arguments
                    .into_iter()
                    .map(|argument| self.substitute_type_parameters(argument, substitutions))
                    .collect::<Option<Vec<_>>>()?;
                if let Some(source) = self.interface_sources_by_id.get(&interface).copied()
                    && self.interface_type_parameters.contains_key(&source)
                {
                    return self
                        .instantiate_interface(source, &arguments)
                        .map(|instance| instance.type_id());
                }
                SemanticType::Interface {
                    interface,
                    arguments,
                }
            }
            SemanticType::Builtin {
                definition,
                arguments,
            } => SemanticType::Builtin {
                definition,
                arguments: arguments
                    .into_iter()
                    .map(|argument| self.substitute_type_parameters(argument, substitutions))
                    .collect::<Option<_>>()?,
            },
            SemanticType::Attribute {
                attribute,
                parameters,
            } => SemanticType::Attribute {
                attribute,
                parameters: parameters
                    .into_iter()
                    .map(|parameter| self.substitute_type_parameters(parameter, substitutions))
                    .collect::<Option<_>>()?,
            },
        };
        self.arena.intern(substituted).ok()
    }

    #[must_use]
    pub fn record_definition(&self, symbol: SymbolId) -> Option<&RecordDefinition> {
        self.record_definitions.get(&symbol)
    }

    #[must_use]
    pub fn record_definition_for_type(&self, type_id: TypeId) -> Option<&RecordDefinition> {
        self.records_by_type
            .get(&type_id)
            .and_then(|symbol| self.record_definitions.get(symbol))
    }

    pub fn record_instances(
        &self,
        definition: SymbolId,
    ) -> impl Iterator<Item = &RecordDefinition> {
        self.record_instances
            .iter()
            .filter(move |((source, _), _)| *source == definition)
            .filter_map(|(_, symbol)| self.record_definitions.get(symbol))
    }

    #[must_use]
    pub fn record_is_generic(&self, definition: SymbolId) -> bool {
        self.record_type_parameters
            .get(&definition)
            .is_some_and(|parameters| !parameters.is_empty())
    }

    pub fn union_instances(&self, definition: SymbolId) -> impl Iterator<Item = &UnionDefinition> {
        self.union_instances
            .iter()
            .filter(move |((source, _), _)| *source == definition)
            .filter_map(|(_, symbol)| self.union_definitions.get(symbol))
    }

    #[must_use]
    pub fn union_is_generic(&self, definition: SymbolId) -> bool {
        self.union_type_parameters
            .get(&definition)
            .is_some_and(|parameters| !parameters.is_empty())
    }

    pub fn error_instances(&self, definition: SymbolId) -> impl Iterator<Item = &ErrorDefinition> {
        self.error_instances
            .iter()
            .filter(move |((source, _), _)| *source == definition)
            .filter_map(|(_, symbol)| self.error_definitions.get(symbol))
    }

    #[must_use]
    pub fn error_is_generic(&self, definition: SymbolId) -> bool {
        self.error_type_parameters
            .get(&definition)
            .is_some_and(|parameters| !parameters.is_empty())
    }

    pub(crate) fn instantiate_record(
        &mut self,
        definition: SymbolId,
        arguments: &[TypeId],
    ) -> Option<RecordDefinition> {
        let key = (definition, arguments.to_vec());
        if let Some(symbol) = self.record_instances.get(&key) {
            return self.record_definitions.get(symbol).cloned();
        }
        let parameters = self.record_type_parameters.get(&definition)?.clone();
        if parameters.len() != arguments.len() {
            return None;
        }
        if parameters
            .iter()
            .map(ResolvedTypeParameter::type_id)
            .eq(arguments.iter().copied())
        {
            return self.record_definitions.get(&definition).cloned();
        }
        let substitutions = parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| (parameter.parameter(), *argument))
            .collect();
        let template = self.record_definitions.get(&definition)?.clone();
        let symbol = SymbolId::from_raw(self.next_instance_symbol);
        self.next_instance_symbol = self.next_instance_symbol.saturating_add(1);
        let mut fields = template.fields.clone();
        for field in &mut fields {
            field.field_type = self.substitute_type_parameters(field.field_type, &substitutions)?;
            field.field = FieldId::from_raw(self.next_field);
            self.next_field = self.next_field.saturating_add(1);
        }
        let type_id = self.substitute_type_parameters(template.type_id, &substitutions)?;
        let instance = RecordDefinition {
            symbol,
            type_id,
            fields,
            span: template.span,
        };
        self.record_instances.insert(key, symbol);
        self.records_by_type.insert(type_id, symbol);
        self.record_definitions.insert(symbol, instance.clone());
        Some(instance)
    }

    pub(crate) fn instantiate_union(
        &mut self,
        definition: SymbolId,
        arguments: &[TypeId],
    ) -> Option<UnionDefinition> {
        let key = (definition, arguments.to_vec());
        if let Some(symbol) = self.union_instances.get(&key) {
            return self.union_definitions.get(symbol).cloned();
        }
        let parameters = self.union_type_parameters.get(&definition)?.clone();
        if parameters.len() != arguments.len() {
            return None;
        }
        if parameters
            .iter()
            .map(ResolvedTypeParameter::type_id)
            .eq(arguments.iter().copied())
        {
            return self.union_definitions.get(&definition).cloned();
        }
        let substitutions = parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| (parameter.parameter(), *argument))
            .collect();
        let template = self.union_definitions.get(&definition)?.clone();
        let symbol = SymbolId::from_raw(self.next_instance_symbol);
        self.next_instance_symbol = self.next_instance_symbol.saturating_add(1);
        let mut cases = template.cases.clone();
        for case in &mut cases {
            for (_, parameter_type, _) in &mut case.parameters {
                *parameter_type =
                    self.substitute_type_parameters(*parameter_type, &substitutions)?;
            }
        }
        let type_id = self
            .arena
            .intern(SemanticType::TaggedUnion {
                definition: symbol,
                source: definition,
                arguments: arguments.to_vec(),
            })
            .ok()?;
        let instance = UnionDefinition {
            symbol,
            type_id,
            cases,
            span: template.span,
        };
        self.union_instances.insert(key, symbol);
        self.union_instance_sources.insert(symbol, definition);
        self.unions_by_type.insert(type_id, symbol);
        self.union_definitions.insert(symbol, instance.clone());
        Some(instance)
    }

    pub(crate) fn instantiate_error(
        &mut self,
        definition: SymbolId,
        arguments: &[TypeId],
    ) -> Option<ErrorDefinition> {
        let key = (definition, arguments.to_vec());
        if let Some(symbol) = self.error_instances.get(&key) {
            return self.error_definitions.get(symbol).cloned();
        }
        let parameters = self.error_type_parameters.get(&definition)?.clone();
        if parameters.len() != arguments.len() {
            return None;
        }
        if parameters
            .iter()
            .map(ResolvedTypeParameter::type_id)
            .eq(arguments.iter().copied())
        {
            return self.error_definitions.get(&definition).cloned();
        }
        let substitutions = parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| (parameter.parameter(), *argument))
            .collect();
        let template = self.error_definitions.get(&definition)?.clone();
        let symbol = SymbolId::from_raw(self.next_instance_symbol);
        self.next_instance_symbol = self.next_instance_symbol.saturating_add(1);
        let mut cases = template.cases.clone();
        for case in &mut cases {
            for (_, parameter_type, _) in &mut case.parameters {
                *parameter_type =
                    self.substitute_type_parameters(*parameter_type, &substitutions)?;
            }
        }
        let type_id = self
            .arena
            .intern(SemanticType::ErrorUnion {
                definition: template.error,
                source: definition,
                arguments: arguments.to_vec(),
            })
            .ok()?;
        let instance = ErrorDefinition {
            error: template.error,
            symbol,
            type_id,
            cases,
            span: template.span,
        };
        self.error_instances.insert(key, symbol);
        self.errors_by_type.insert(type_id, symbol);
        self.error_definitions.insert(symbol, instance.clone());
        Some(instance)
    }

    #[must_use]
    pub fn declaration_type(&self, symbol: SymbolId) -> Option<TypeId> {
        self.record_definitions
            .get(&symbol)
            .map(RecordDefinition::type_id)
            .or_else(|| {
                self.union_definitions
                    .get(&symbol)
                    .map(UnionDefinition::type_id)
            })
            .or_else(|| {
                self.error_definitions
                    .get(&symbol)
                    .map(ErrorDefinition::type_id)
            })
            .or_else(|| {
                self.enum_definitions
                    .get(&symbol)
                    .map(EnumDefinition::type_id)
            })
            .or_else(|| {
                self.class_definitions
                    .get(&symbol)
                    .map(crate::ClassDefinition::type_id)
            })
            .or_else(|| {
                self.interface_definitions
                    .get(&symbol)
                    .map(crate::InterfaceDefinition::type_id)
            })
            .or_else(|| {
                self.attribute_definitions
                    .get(&symbol)
                    .map(crate::AttributeDefinition::type_id)
            })
    }

    pub fn interface_definitions(&self) -> impl Iterator<Item = &crate::InterfaceDefinition> {
        self.interface_definitions.values()
    }

    #[must_use]
    pub fn union_definition(&self, symbol: SymbolId) -> Option<&UnionDefinition> {
        self.union_definitions.get(&symbol)
    }

    #[must_use]
    pub fn union_definition_for_type(&self, type_id: TypeId) -> Option<&UnionDefinition> {
        self.unions_by_type
            .get(&type_id)
            .and_then(|symbol| self.union_definitions.get(symbol))
    }

    #[must_use]
    pub fn error_definition(&self, symbol: SymbolId) -> Option<&ErrorDefinition> {
        self.error_definitions.get(&symbol)
    }

    #[must_use]
    pub fn error_definition_for_type(&self, type_id: TypeId) -> Option<&ErrorDefinition> {
        self.errors_by_type
            .get(&type_id)
            .and_then(|symbol| self.error_definitions.get(symbol))
    }

    #[must_use]
    pub(crate) fn error_type_parameter_count(&self, symbol: SymbolId) -> Option<usize> {
        self.error_type_parameters.get(&symbol).map(Vec::len)
    }

    #[must_use]
    pub(crate) fn union_type_parameter_count(&self, symbol: SymbolId) -> Option<usize> {
        self.union_type_parameters.get(&symbol).map(Vec::len)
    }

    #[must_use]
    pub(crate) fn union_source_symbol(&self, symbol: SymbolId) -> SymbolId {
        self.union_instance_sources
            .get(&symbol)
            .copied()
            .unwrap_or(symbol)
    }

    #[must_use]
    pub fn enum_definition(&self, symbol: SymbolId) -> Option<&EnumDefinition> {
        self.enum_definitions.get(&symbol)
    }

    #[must_use]
    pub fn define_enum(
        &mut self,
        symbol: SymbolId,
        syntax: &EnumDeclarationSyntax,
    ) -> EnumDefinitionResult {
        let mut diagnostics = Vec::new();
        let mut names = BTreeMap::new();
        let mut cases = Vec::new();
        for (discriminant, case) in syntax.cases().iter().enumerate() {
            if let Some(original) = names.insert(case.name().to_owned(), case.span()) {
                diagnostics.push(type_diagnostics::duplicate_record_field(
                    case.span(),
                    case.name(),
                    original,
                ));
                continue;
            }
            let case_id = EnumCaseId::from_raw(self.next_enum_case);
            self.next_enum_case = self.next_enum_case.saturating_add(1);
            cases.push(EnumCaseDefinition {
                case: case_id,
                name: case.name().to_owned(),
                discriminant: u32::try_from(discriminant).unwrap_or(u32::MAX),
                span: case.span(),
            });
        }
        let type_id = self
            .arena
            .intern(SemanticType::Enum { definition: symbol })
            .ok();
        let definition = if diagnostics.is_empty() {
            type_id.map(|type_id| EnumDefinition {
                symbol,
                type_id,
                cases,
                span: syntax.span(),
            })
        } else {
            None
        };
        if let Some(definition) = &definition {
            self.enum_definitions.insert(symbol, definition.clone());
        }
        EnumDefinitionResult {
            definition,
            diagnostics,
        }
    }

    pub fn register_type_alias(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &TypeAliasDeclarationSyntax,
    ) {
        self.type_aliases
            .insert(symbol, (module, syntax.target().clone()));
    }

    #[must_use]
    pub fn define_record(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &RecordDeclarationSyntax,
    ) -> RecordDefinitionResult {
        self.define_record_impl(module, symbol, syntax, false)
    }

    #[must_use]
    pub fn define_record_schema(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &RecordDeclarationSyntax,
    ) -> RecordDefinitionResult {
        self.define_record_impl(module, symbol, syntax, true)
    }

    fn define_record_impl(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &RecordDeclarationSyntax,
        defer_defaults: bool,
    ) -> RecordDefinitionResult {
        let mut diagnostics = Vec::new();
        let (type_parameters, generics) =
            self.resolve_data_type_parameters(module, syntax.type_parameters(), &mut diagnostics);
        let mut pending_fields = Vec::new();
        let mut semantic_fields = Vec::new();
        let mut names = BTreeMap::new();
        for field in syntax.fields() {
            if let Some(original) = names.get(field.name()).copied() {
                diagnostics.push(type_diagnostics::duplicate_record_field(
                    field.span(),
                    field.name(),
                    original,
                ));
                continue;
            }
            names.insert(field.name().to_owned(), field.span());
            let Some(resolved) =
                self.resolve_type(module, field.field_type(), &generics, &mut diagnostics)
            else {
                continue;
            };
            let Some(field_type) = resolved.type_id() else {
                continue;
            };
            let (default, pending_default) = match field.default_value() {
                Some(value) if defer_defaults => (
                    None,
                    Some(PendingConstantExpression::new(value.clone(), field_type)),
                ),
                Some(value) => (
                    resolve_field_default(
                        &self.arena,
                        field_type,
                        value,
                        "record",
                        &mut diagnostics,
                    ),
                    None,
                ),
                None => (None, None),
            };
            semantic_fields.push((field.name().to_owned(), field_type));
            pending_fields.push((
                field.name().to_owned(),
                field_type,
                default,
                pending_default,
                field.span(),
            ));
        }
        let type_id = self
            .arena
            .intern(SemanticType::Record(semantic_fields))
            .ok();
        let definition = if diagnostics.is_empty() {
            type_id.map(|type_id| {
                let fields = pending_fields
                    .into_iter()
                    .map(|(name, field_type, default, pending_default, span)| {
                        let key = (name.clone(), field_type);
                        let field =
                            *self.structural_record_fields.entry(key).or_insert_with(|| {
                                let field = FieldId::from_raw(self.next_field);
                                self.next_field = self.next_field.saturating_add(1);
                                field
                            });
                        RecordFieldDefinition {
                            field,
                            name,
                            field_type,
                            default,
                            pending_default,
                            span,
                        }
                    })
                    .collect();
                RecordDefinition {
                    symbol,
                    type_id,
                    fields,
                    span: syntax.span(),
                }
            })
        } else {
            None
        };
        if let Some(definition) = &definition {
            self.record_type_parameters.insert(symbol, type_parameters);
            self.records_by_type
                .entry(definition.type_id())
                .or_insert(symbol);
            self.record_definitions.insert(symbol, definition.clone());
        }
        RecordDefinitionResult {
            definition,
            diagnostics,
        }
    }

    /// Installs one already-evaluated record field default into a deferred schema.
    ///
    /// # Errors
    ///
    /// Rejects unknown identities, a non-pending target, or a value whose
    /// canonical type is not assignable to the field type.
    pub fn install_record_field_default(
        &mut self,
        definition: SymbolId,
        field: FieldId,
        value: FieldDefault,
    ) -> Result<(), RequiredConstantError> {
        let target = RequiredConstantTarget::RecordField { definition, field };
        let Some(record) = self.record_definitions.get(&definition) else {
            return Err(RequiredConstantError::UnknownTarget(target));
        };
        let Some(index) = record
            .fields
            .iter()
            .position(|candidate| candidate.field == field)
        else {
            return Err(RequiredConstantError::UnknownTarget(target));
        };
        let expected = record.fields[index].field_type;
        if record.fields[index].pending_default.is_none() {
            return Err(RequiredConstantError::NoPendingDefault(target));
        }
        if !field_default_matches_type(self.arena(), &value, expected) {
            return Err(RequiredConstantError::TypeMismatch { target, expected });
        }
        let field = self
            .record_definitions
            .get_mut(&definition)
            .and_then(|record| record.fields.get_mut(index))
            .ok_or(RequiredConstantError::UnknownTarget(target))?;
        field.default = Some(value);
        field.pending_default = None;
        Ok(())
    }

    #[must_use]
    pub fn define_union(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &UnionDeclarationSyntax,
    ) -> UnionDefinitionResult {
        let mut diagnostics = Vec::new();
        let (type_parameters, generics) =
            self.resolve_data_type_parameters(module, syntax.type_parameters(), &mut diagnostics);
        let mut cases = Vec::new();
        let mut names = BTreeMap::new();
        for case in syntax.cases() {
            if let Some(original) = names.insert(case.name().to_owned(), case.span()) {
                diagnostics.push(type_diagnostics::duplicate_record_field(
                    case.span(),
                    case.name(),
                    original,
                ));
                continue;
            }
            let mut parameters = Vec::new();
            for parameter in case.payload() {
                let Some(resolved) = self.resolve_type(
                    module,
                    parameter.parameter_type(),
                    &generics,
                    &mut diagnostics,
                ) else {
                    continue;
                };
                let Some(type_id) = resolved.type_id() else {
                    continue;
                };
                parameters.push((parameter.name().to_owned(), type_id, parameter.span()));
            }
            let id = UnionCaseId::from_raw(self.next_union_case);
            self.next_union_case = self.next_union_case.saturating_add(1);
            cases.push(UnionCaseDefinition {
                case: id,
                name: case.name().to_owned(),
                parameters,
                span: case.span(),
            });
        }
        let generic_arguments = type_parameters
            .iter()
            .map(ResolvedTypeParameter::type_id)
            .collect();
        let type_id = self
            .arena
            .intern(SemanticType::TaggedUnion {
                definition: symbol,
                source: symbol,
                arguments: generic_arguments,
            })
            .ok();
        let definition = if diagnostics.is_empty() {
            type_id.map(|type_id| UnionDefinition {
                symbol,
                type_id,
                cases,
                span: syntax.span(),
            })
        } else {
            None
        };
        if let Some(definition) = &definition {
            self.union_type_parameters.insert(symbol, type_parameters);
            self.unions_by_type.insert(definition.type_id(), symbol);
            self.union_definitions.insert(symbol, definition.clone());
        }
        UnionDefinitionResult {
            definition,
            diagnostics,
        }
    }

    #[must_use]
    pub fn define_error(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &ErrorDeclarationSyntax,
    ) -> ErrorDefinitionResult {
        let mut diagnostics = Vec::new();
        let (type_parameters, generics) =
            self.resolve_data_type_parameters(module, syntax.type_parameters(), &mut diagnostics);
        let error = ErrorId::from_raw(self.next_error);
        self.next_error = self.next_error.saturating_add(1);
        let mut cases = Vec::new();
        let mut names = BTreeMap::new();
        for case in syntax.cases() {
            if let Some(original) = names.insert(case.name().to_owned(), case.span()) {
                diagnostics.push(type_diagnostics::duplicate_record_field(
                    case.span(),
                    case.name(),
                    original,
                ));
                continue;
            }
            let mut parameters = Vec::new();
            for parameter in case.payload() {
                let Some(resolved) = self.resolve_type(
                    module,
                    parameter.parameter_type(),
                    &generics,
                    &mut diagnostics,
                ) else {
                    continue;
                };
                let Some(type_id) = resolved.type_id() else {
                    continue;
                };
                parameters.push((parameter.name().to_owned(), type_id, parameter.span()));
            }
            let case_id = ErrorCaseId::from_raw(self.next_error_case);
            self.next_error_case = self.next_error_case.saturating_add(1);
            cases.push(ErrorCaseDefinition {
                case: case_id,
                name: case.name().to_owned(),
                parameters,
                span: case.span(),
            });
        }
        let generic_arguments = type_parameters
            .iter()
            .map(ResolvedTypeParameter::type_id)
            .collect();
        let type_id = self
            .arena
            .intern(SemanticType::ErrorUnion {
                definition: error,
                source: symbol,
                arguments: generic_arguments,
            })
            .ok();
        let definition = if diagnostics.is_empty() {
            type_id.map(|type_id| ErrorDefinition {
                error,
                symbol,
                type_id,
                cases,
                span: syntax.span(),
            })
        } else {
            None
        };
        if let Some(definition) = &definition {
            self.error_type_parameters.insert(symbol, type_parameters);
            self.errors_by_type.insert(definition.type_id(), symbol);
            self.error_definitions.insert(symbol, definition.clone());
        }
        ErrorDefinitionResult {
            definition,
            diagnostics,
        }
    }

    fn resolve_data_type_parameters(
        &mut self,
        module: ModuleId,
        syntax: &[GenericParameterSyntax],
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (
        Vec<ResolvedTypeParameter>,
        BTreeMap<String, (ParameterId, TypeId)>,
    ) {
        self.resolve_generic_parameters(module, syntax, diagnostics)
    }

    pub(crate) fn resolve_annotation(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
        signature: &ResolvedFunctionSignature,
    ) -> (Option<ResolvedType>, Vec<Diagnostic>) {
        let generics = signature
            .type_parameters()
            .iter()
            .map(|parameter| {
                (
                    parameter.name().to_owned(),
                    (parameter.parameter(), parameter.type_id()),
                )
            })
            .collect();
        let mut diagnostics = Vec::new();
        let resolved = self.resolve_type(module, syntax, &generics, &mut diagnostics);
        (resolved, diagnostics)
    }

    #[must_use]
    pub fn resolve(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &FunctionSignatureSyntax,
    ) -> ResolvedSignatureResult {
        let mut diagnostics = Vec::new();
        let (type_parameters, generic_types) =
            self.resolve_type_parameters(module, syntax, &mut diagnostics);
        let parameters = syntax
            .parameters()
            .iter()
            .filter_map(|parameter| {
                self.resolve_type(
                    module,
                    parameter.parameter_type(),
                    &generic_types,
                    &mut diagnostics,
                )
                .map(|parameter_type| ResolvedFunctionParameter {
                    name: parameter.name().to_owned(),
                    parameter_type,
                    span: parameter.span(),
                })
            })
            .collect();
        let results = syntax
            .results()
            .iter()
            .filter_map(|result| {
                self.resolve_type(module, result, &generic_types, &mut diagnostics)
            })
            .collect();
        diagnostics.sort_by_key(|diagnostic| {
            let span = diagnostic.primary_span();
            (
                span.file(),
                span.range().start(),
                diagnostic.code().as_str(),
            )
        });
        let signature = diagnostics.is_empty().then(|| ResolvedFunctionSignature {
            symbol,
            name: syntax.name().to_owned(),
            is_async: syntax.is_async(),
            type_parameters,
            parameters,
            results,
            effects: crate::EffectSummary::empty(),
        });
        ResolvedSignatureResult {
            signature,
            diagnostics,
        }
    }

    fn resolve_type_parameters(
        &mut self,
        module: ModuleId,
        syntax: &FunctionSignatureSyntax,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (
        Vec<ResolvedTypeParameter>,
        BTreeMap<String, (ParameterId, TypeId)>,
    ) {
        self.resolve_generic_parameters(module, syntax.type_parameters(), diagnostics)
    }

    pub(crate) fn resolve_generic_parameters(
        &mut self,
        module: ModuleId,
        syntax: &[GenericParameterSyntax],
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (
        Vec<ResolvedTypeParameter>,
        BTreeMap<String, (ParameterId, TypeId)>,
    ) {
        let mut resolved = Vec::new();
        let mut by_name = BTreeMap::new();
        let mut spans = BTreeMap::new();
        for parameter in syntax {
            if let Some(original) = spans.get(parameter.name()) {
                diagnostics.push(type_diagnostics::duplicate_type_parameter(
                    parameter.span(),
                    parameter.name(),
                    *original,
                ));
                continue;
            }
            let bound = parameter.bound().and_then(|bound| {
                let resolved = self.resolve_type(module, bound, &by_name, diagnostics)?;
                let bound_type = resolved.type_id()?;
                let valid = match self.arena.get(bound_type) {
                    Some(SemanticType::Interface { .. }) => true,
                    Some(SemanticType::Builtin { definition, .. }) => {
                        self.schema.types().iter().any(|entry| {
                            entry.id() == *definition
                                && entry.role() == BootstrapTypeRole::Interface
                        })
                    }
                    _ => false,
                };
                if !valid {
                    diagnostics.push(type_diagnostics::invalid_generic_bound(
                        bound.span(),
                        parameter.name(),
                        "nominal interface",
                    ));
                    return None;
                }
                Some(bound_type)
            });
            let id = ParameterId::from_raw(self.next_parameter);
            self.next_parameter = self.next_parameter.saturating_add(1);
            let type_id = self
                .arena
                .intern(SemanticType::TypeParameter(id))
                .expect("new type parameter references no invalid TypeId");
            spans.insert(parameter.name().to_owned(), parameter.span());
            by_name.insert(parameter.name().to_owned(), (id, type_id));
            resolved.push(ResolvedTypeParameter {
                parameter: id,
                name: parameter.name().to_owned(),
                type_id,
                bound,
                span: parameter.span(),
            });
        }
        (resolved, by_name)
    }

    pub(crate) fn resolve_type(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<ResolvedType> {
        match syntax.kind() {
            TypeSyntaxKind::Named { path, arguments } => {
                self.resolve_named(module, syntax, path, arguments, generics, diagnostics)
            }
            TypeSyntaxKind::Array(element) => {
                let element = self.resolve_type(module, element, generics, diagnostics)?;
                let type_id = canonical_single(&mut self.arena, &element, SemanticType::Array);
                Some(resolved(
                    ResolvedTypeKind::Array(Box::new(element)),
                    type_id,
                    syntax.span(),
                ))
            }
            TypeSyntaxKind::Table { key, value } => {
                let key = self.resolve_type(module, key, generics, diagnostics)?;
                let value = self.resolve_type(module, value, generics, diagnostics)?;
                let type_id = match (key.type_id(), value.type_id()) {
                    (Some(key), Some(value)) => {
                        self.arena.intern(SemanticType::Table { key, value }).ok()
                    }
                    _ => None,
                };
                Some(resolved(
                    ResolvedTypeKind::Table {
                        key: Box::new(key),
                        value: Box::new(value),
                    },
                    type_id,
                    syntax.span(),
                ))
            }
            TypeSyntaxKind::Tuple(elements) => self.resolve_compound(
                module,
                syntax,
                elements,
                generics,
                diagnostics,
                Compound::Tuple,
            ),
            TypeSyntaxKind::Union(members) => self.resolve_compound(
                module,
                syntax,
                members,
                generics,
                diagnostics,
                Compound::Union,
            ),
            TypeSyntaxKind::Optional(inner) => {
                let inner = self.resolve_type(module, inner, generics, diagnostics)?;
                let type_id = inner
                    .type_id()
                    .and_then(|inner| self.arena.optional(inner).ok());
                Some(resolved(
                    ResolvedTypeKind::Optional(Box::new(inner)),
                    type_id,
                    syntax.span(),
                ))
            }
            TypeSyntaxKind::Function {
                is_async,
                parameters,
                results,
            } => self.resolve_function_type(
                module,
                syntax,
                *is_async,
                parameters,
                results,
                generics,
                diagnostics,
            ),
        }
    }

    /// Resolves one source type outside a function-generic environment.
    #[must_use]
    pub fn resolve_standalone_type(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
    ) -> (Option<TypeId>, Vec<Diagnostic>) {
        let mut diagnostics = Vec::new();
        let resolved = self.resolve_type(module, syntax, &BTreeMap::new(), &mut diagnostics);
        (
            resolved.and_then(|resolved| resolved.type_id()),
            diagnostics,
        )
    }

    fn resolve_named(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
        path: &[String],
        arguments: &[TypeSyntax],
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<ResolvedType> {
        let simple = prelude_name(path);
        if let Some((parameter, type_id)) = simple.and_then(|name| generics.get(name)) {
            if !arguments.is_empty() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    simple.unwrap_or_default(),
                    0,
                    arguments.len(),
                ));
                return None;
            }
            return Some(resolved(
                ResolvedTypeKind::TypeParameter {
                    parameter: *parameter,
                },
                Some(*type_id),
                syntax.span(),
            ));
        }
        if let Some(type_id) = simple.and_then(|name| self.arena.source_type(name)) {
            if !arguments.is_empty() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    simple.unwrap_or_default(),
                    0,
                    arguments.len(),
                ));
                return None;
            }
            return Some(resolved(
                ResolvedTypeKind::Primitive,
                Some(type_id),
                syntax.span(),
            ));
        }
        if let Some(entry) = simple
            .and_then(|name| self.schema.type_by_source_name(name))
            .copied()
            .filter(|entry| self.bootstrap_type_is_available(*entry))
        {
            return self.resolve_builtin(module, syntax, entry, arguments, generics, diagnostics);
        }
        if let Some(entry) = self
            .schema
            .type_by_source_name(&path.join("."))
            .copied()
            .filter(|entry| self.bootstrap_type_is_available(*entry))
        {
            return self.resolve_builtin(module, syntax, entry, arguments, generics, diagnostics);
        }
        let name = path.join(".");
        let resolution = self
            .database
            .resolve(module, &name, SymbolSpace::Type, syntax.span());
        if !resolution.diagnostics().is_empty() {
            diagnostics.extend(resolution.diagnostics().iter().cloned());
            return None;
        }
        let symbol = resolution.symbol()?;
        if let Some((alias_module, target)) = self.type_aliases.get(&symbol).cloned() {
            if !arguments.is_empty() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    &name,
                    0,
                    arguments.len(),
                ));
                return None;
            }
            if self.resolving_aliases.contains_key(&symbol) {
                diagnostics.push(pop_diagnostics::resolution::unknown_name(
                    syntax.span(),
                    name,
                ));
                return None;
            }
            self.resolving_aliases.insert(symbol, syntax.span());
            let resolved = self.resolve_type(alias_module, &target, generics, diagnostics);
            self.resolving_aliases.remove(&symbol);
            return resolved;
        }
        let arguments = self.resolve_types(module, arguments, generics, diagnostics)?;
        let canonical_arguments = arguments
            .iter()
            .map(ResolvedType::type_id)
            .collect::<Option<Vec<_>>>()?;
        if let Some(parameters) = self.record_type_parameters.get(&symbol).cloned() {
            if parameters.len() != arguments.len() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    &name,
                    u16::try_from(parameters.len()).unwrap_or(u16::MAX),
                    arguments.len(),
                ));
                return None;
            }
            let template_arguments: Vec<_> = parameters
                .iter()
                .map(ResolvedTypeParameter::type_id)
                .collect();
            let definition = if canonical_arguments == template_arguments {
                self.record_definitions.get(&symbol).cloned()
            } else {
                self.instantiate_record(symbol, &canonical_arguments)
            }?;
            return Some(resolved(
                ResolvedTypeKind::Declaration {
                    symbol: definition.symbol(),
                    arguments,
                },
                Some(definition.type_id()),
                syntax.span(),
            ));
        }
        if let Some(parameters) = self.union_type_parameters.get(&symbol).cloned() {
            if parameters.len() != arguments.len() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    &name,
                    u16::try_from(parameters.len()).unwrap_or(u16::MAX),
                    arguments.len(),
                ));
                return None;
            }
            let template_arguments: Vec<_> = parameters
                .iter()
                .map(ResolvedTypeParameter::type_id)
                .collect();
            let definition = if canonical_arguments == template_arguments {
                self.union_definitions.get(&symbol).cloned()
            } else {
                self.instantiate_union(symbol, &canonical_arguments)
            }?;
            return Some(resolved(
                ResolvedTypeKind::Declaration {
                    symbol: definition.symbol(),
                    arguments,
                },
                Some(definition.type_id()),
                syntax.span(),
            ));
        }
        if let Some(parameters) = self.error_type_parameters.get(&symbol).cloned() {
            if parameters.len() != arguments.len() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    &name,
                    u16::try_from(parameters.len()).unwrap_or(u16::MAX),
                    arguments.len(),
                ));
                return None;
            }
            let template_arguments: Vec<_> = parameters
                .iter()
                .map(ResolvedTypeParameter::type_id)
                .collect();
            let definition = if canonical_arguments == template_arguments {
                self.error_definitions.get(&symbol).cloned()
            } else {
                self.instantiate_error(symbol, &canonical_arguments)
            }?;
            return Some(resolved(
                ResolvedTypeKind::Declaration {
                    symbol: definition.symbol(),
                    arguments,
                },
                Some(definition.type_id()),
                syntax.span(),
            ));
        }
        if let Some(parameters) = self.class_type_parameters.get(&symbol).cloned() {
            if parameters.len() != arguments.len() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    &name,
                    u16::try_from(parameters.len()).unwrap_or(u16::MAX),
                    arguments.len(),
                ));
                return None;
            }
            if !self.validate_class_arguments(
                symbol,
                &canonical_arguments,
                syntax.span(),
                diagnostics,
            ) {
                return None;
            }
            let template_arguments: Vec<_> = parameters
                .iter()
                .map(ResolvedTypeParameter::type_id)
                .collect();
            if canonical_arguments == template_arguments {
                return Some(resolved(
                    ResolvedTypeKind::Declaration { symbol, arguments },
                    self.class_types.get(&symbol).copied(),
                    syntax.span(),
                ));
            }
            let definition = self.instantiate_class(symbol, &canonical_arguments)?;
            return Some(resolved(
                ResolvedTypeKind::Declaration {
                    symbol: definition.symbol(),
                    arguments,
                },
                Some(definition.type_id()),
                syntax.span(),
            ));
        }
        if let Some(parameters) = self.interface_type_parameters.get(&symbol).cloned() {
            if parameters.len() != arguments.len() {
                diagnostics.push(type_diagnostics::wrong_type_arity(
                    syntax.span(),
                    &name,
                    u16::try_from(parameters.len()).unwrap_or(u16::MAX),
                    arguments.len(),
                ));
                return None;
            }
            if !self.validate_interface_arguments(
                symbol,
                &canonical_arguments,
                syntax.span(),
                diagnostics,
            ) {
                return None;
            }
            let template_arguments: Vec<_> = parameters
                .iter()
                .map(ResolvedTypeParameter::type_id)
                .collect();
            let definition = if canonical_arguments == template_arguments {
                self.interface_definitions.get(&symbol).cloned()
            } else {
                self.instantiate_interface(symbol, &canonical_arguments)
            }?;
            return Some(resolved(
                ResolvedTypeKind::Declaration {
                    symbol: definition.symbol(),
                    arguments,
                },
                Some(definition.type_id()),
                syntax.span(),
            ));
        }
        if !arguments.is_empty() {
            diagnostics.push(type_diagnostics::wrong_type_arity(
                syntax.span(),
                &name,
                0,
                arguments.len(),
            ));
            return None;
        }
        let type_id = self
            .record_definitions
            .get(&symbol)
            .map(RecordDefinition::type_id)
            .or_else(|| {
                self.union_definitions
                    .get(&symbol)
                    .map(UnionDefinition::type_id)
            })
            .or_else(|| {
                self.error_definitions
                    .get(&symbol)
                    .map(ErrorDefinition::type_id)
            })
            .or_else(|| {
                self.enum_definitions
                    .get(&symbol)
                    .map(EnumDefinition::type_id)
            })
            .or_else(|| self.class_types.get(&symbol).copied())
            .or_else(|| self.interface_types.get(&symbol).copied());
        Some(resolved(
            ResolvedTypeKind::Declaration { symbol, arguments },
            type_id,
            syntax.span(),
        ))
    }

    fn bootstrap_type_is_available(&self, entry: crate::BootstrapTypeEntry) -> bool {
        entry.owner_bubble() != "Pop.Ffi" || self.has_ffi_dependency
    }

    fn resolve_builtin(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
        entry: crate::BootstrapTypeEntry,
        arguments: &[TypeSyntax],
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<ResolvedType> {
        if usize::from(entry.arity()) != arguments.len() {
            diagnostics.push(type_diagnostics::wrong_type_arity(
                syntax.span(),
                entry.source_name(),
                entry.arity(),
                arguments.len(),
            ));
            return None;
        }
        let arguments = self.resolve_types(module, arguments, generics, diagnostics)?;
        let canonical_arguments: Option<Vec<_>> =
            arguments.iter().map(ResolvedType::type_id).collect();
        let type_id = canonical_arguments.and_then(|canonical| {
            let semantic = match entry.role() {
                BootstrapTypeRole::Array => SemanticType::Array(canonical[0]),
                BootstrapTypeRole::Table => SemanticType::Table {
                    key: canonical[0],
                    value: canonical[1],
                },
                BootstrapTypeRole::Nominal | BootstrapTypeRole::Interface => {
                    SemanticType::Builtin {
                        definition: entry.id(),
                        arguments: canonical,
                    }
                }
            };
            self.arena.intern(semantic).ok()
        });
        Some(resolved(
            ResolvedTypeKind::Builtin {
                definition: entry.id(),
                arguments,
            },
            type_id,
            syntax.span(),
        ))
    }

    fn resolve_types(
        &mut self,
        module: ModuleId,
        types: &[TypeSyntax],
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<Vec<ResolvedType>> {
        let mut resolved = Vec::new();
        for type_syntax in types {
            resolved.push(self.resolve_type(module, type_syntax, generics, diagnostics)?);
        }
        Some(resolved)
    }

    fn resolve_compound(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
        elements: &[TypeSyntax],
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
        compound: Compound,
    ) -> Option<ResolvedType> {
        let elements = self.resolve_types(module, elements, generics, diagnostics)?;
        let canonical: Option<Vec<_>> = elements.iter().map(ResolvedType::type_id).collect();
        let type_id = canonical.and_then(|canonical| match compound {
            Compound::Tuple => self.arena.intern(SemanticType::Tuple(canonical)).ok(),
            Compound::Union => self.arena.union(canonical).ok(),
        });
        let kind = match compound {
            Compound::Tuple => ResolvedTypeKind::Tuple(elements),
            Compound::Union => ResolvedTypeKind::Union(elements),
        };
        Some(resolved(kind, type_id, syntax.span()))
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_function_type(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
        is_async: bool,
        parameters: &[TypeSyntax],
        results: &[TypeSyntax],
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<ResolvedType> {
        let parameters = self.resolve_types(module, parameters, generics, diagnostics)?;
        let results = self.resolve_types(module, results, generics, diagnostics)?;
        let parameter_ids: Option<Vec<_>> = parameters.iter().map(ResolvedType::type_id).collect();
        let result_ids: Option<Vec<_>> = results.iter().map(ResolvedType::type_id).collect();
        let type_id = parameter_ids
            .zip(result_ids)
            .and_then(|(parameters, results)| {
                self.arena
                    .intern(SemanticType::Function {
                        is_async,
                        parameters,
                        results,
                        effects: crate::EffectSummary::empty(),
                    })
                    .ok()
            });
        Some(resolved(
            ResolvedTypeKind::Function {
                is_async,
                parameters,
                results,
                effects: crate::EffectSummary::empty(),
            },
            type_id,
            syntax.span(),
        ))
    }
}

#[derive(Clone, Copy)]
enum Compound {
    Tuple,
    Union,
}

fn canonical_single(
    arena: &mut TypeArena,
    inner: &ResolvedType,
    constructor: impl FnOnce(TypeId) -> SemanticType,
) -> Option<TypeId> {
    inner
        .type_id()
        .and_then(|inner| arena.intern(constructor(inner)).ok())
}

fn resolved(kind: ResolvedTypeKind, type_id: Option<TypeId>, span: SourceSpan) -> ResolvedType {
    ResolvedType {
        kind,
        type_id,
        span,
    }
}

fn prelude_name(path: &[String]) -> Option<&str> {
    match path {
        [name] => Some(name),
        [pop, name] if pop == "Pop" => Some(name),
        _ => None,
    }
}

fn diagnostic_snapshot(diagnostics: &[Diagnostic]) -> String {
    let mut snapshot = String::new();
    for diagnostic in diagnostics {
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
