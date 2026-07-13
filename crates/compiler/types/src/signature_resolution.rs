use std::collections::BTreeMap;

use pop_diagnostics::types as type_diagnostics;
use pop_foundation::{
    BuiltinTypeId, Diagnostic, EnumCaseId, FieldId, ModuleId, ParameterId, SourceSpan, SymbolId,
    TypeId, UnionCaseId,
};
use pop_resolve::{ResolutionDatabase, SymbolSpace};
use pop_syntax::{
    EnumDeclarationSyntax, FunctionSignatureSyntax, RecordDeclarationSyntax,
    TypeAliasDeclarationSyntax, TypeSyntax, TypeSyntaxKind, UnionDeclarationSyntax,
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
        Self {
            symbol,
            name,
            type_parameters: Vec::new(),
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

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
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
    pub(crate) arena: TypeArena,
    next_parameter: u32,
    pub(crate) next_field: u32,
    next_union_case: u32,
    next_enum_case: u32,
    pub(crate) next_class: u32,
    pub(crate) next_method: u32,
    pub(crate) next_interface: u32,
    pub(crate) next_interface_method: u32,
    pub(crate) next_attribute: u32,
    record_definitions: BTreeMap<SymbolId, RecordDefinition>,
    records_by_type: BTreeMap<TypeId, SymbolId>,
    structural_record_fields: BTreeMap<(String, TypeId), FieldId>,
    union_definitions: BTreeMap<SymbolId, UnionDefinition>,
    enum_definitions: BTreeMap<SymbolId, EnumDefinition>,
    pub(crate) class_types: BTreeMap<SymbolId, TypeId>,
    pub(crate) class_definitions: BTreeMap<SymbolId, crate::ClassDefinition>,
    pub(crate) classes_by_type: BTreeMap<TypeId, SymbolId>,
    pub(crate) interface_types: BTreeMap<SymbolId, TypeId>,
    pub(crate) interface_definitions: BTreeMap<SymbolId, crate::InterfaceDefinition>,
    type_aliases: BTreeMap<SymbolId, (ModuleId, TypeSyntax)>,
    resolving_aliases: BTreeMap<SymbolId, SourceSpan>,
    pub(crate) interfaces_by_type: BTreeMap<TypeId, SymbolId>,
    pub(crate) attribute_definitions: BTreeMap<SymbolId, crate::AttributeDefinition>,
}

impl<'index> SignatureResolver<'index> {
    #[must_use]
    pub fn new(database: &'index ResolutionDatabase, schema: BootstrapSchema) -> Self {
        Self {
            database,
            schema,
            arena: TypeArena::new(),
            next_parameter: 0,
            next_field: 0,
            next_union_case: 0,
            next_enum_case: 0,
            next_class: 0,
            next_method: 0,
            next_interface: 0,
            next_interface_method: 0,
            next_attribute: 0,
            record_definitions: BTreeMap::new(),
            records_by_type: BTreeMap::new(),
            structural_record_fields: BTreeMap::new(),
            union_definitions: BTreeMap::new(),
            enum_definitions: BTreeMap::new(),
            class_types: BTreeMap::new(),
            class_definitions: BTreeMap::new(),
            classes_by_type: BTreeMap::new(),
            interface_types: BTreeMap::new(),
            interface_definitions: BTreeMap::new(),
            type_aliases: BTreeMap::new(),
            resolving_aliases: BTreeMap::new(),
            interfaces_by_type: BTreeMap::new(),
            attribute_definitions: BTreeMap::new(),
        }
    }

    #[must_use]
    pub const fn arena(&self) -> &TypeArena {
        &self.arena
    }

    pub(crate) const fn schema(&self) -> &BootstrapSchema {
        &self.schema
    }

    #[must_use]
    pub fn into_arena(self) -> TypeArena {
        self.arena
    }

    pub(crate) const fn database(&self) -> &ResolutionDatabase {
        self.database
    }

    pub(crate) fn arena_mut(&mut self) -> &mut TypeArena {
        &mut self.arena
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
        let generics = BTreeMap::new();
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
        let generics = BTreeMap::new();
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
        let type_id = self
            .arena
            .intern(SemanticType::TaggedUnion { definition: symbol })
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
            self.union_definitions.insert(symbol, definition.clone());
        }
        UnionDefinitionResult {
            definition,
            diagnostics,
        }
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
            self.resolve_type_parameters(syntax, &mut diagnostics);
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
        syntax: &FunctionSignatureSyntax,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (
        Vec<ResolvedTypeParameter>,
        BTreeMap<String, (ParameterId, TypeId)>,
    ) {
        let mut resolved = Vec::new();
        let mut by_name = BTreeMap::new();
        let mut spans = BTreeMap::new();
        for parameter in syntax.type_parameters() {
            if let Some(original) = spans.get(parameter.name()) {
                diagnostics.push(type_diagnostics::duplicate_type_parameter(
                    parameter.span(),
                    parameter.name(),
                    *original,
                ));
                continue;
            }
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
                parameters,
                results,
            } => self.resolve_function_type(
                module,
                syntax,
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

    fn resolve_function_type(
        &mut self,
        module: ModuleId,
        syntax: &TypeSyntax,
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
                        parameters,
                        results,
                        effects: crate::EffectSummary::empty(),
                    })
                    .ok()
            });
        Some(resolved(
            ResolvedTypeKind::Function {
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
