use std::collections::{BTreeMap, BTreeSet};

use pop_diagnostics::{compile_time as compile_time_diagnostics, types as type_diagnostics};
use pop_foundation::{AttributeId, Diagnostic, FunctionId, ModuleId, SourceSpan, SymbolId, TypeId};
use pop_resolve::{ResolutionDatabase, SymbolSpace};
use pop_syntax::{
    AttributeDeclarationSyntax, AttributeUseSyntax, ExpressionSyntax, ExpressionSyntaxKind,
    UnaryOperator,
};

use crate::required_constants::attribute_constant_matches_type;
use crate::{
    AttributeParameterId, FloatKind, FloatValue, IntegerValue, NumericError,
    PendingConstantExpression, PrimitiveType, RequiredConstantError, RequiredConstantTarget,
    SemanticType, SignatureResolver,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AttributeConstant {
    Nil,
    Boolean(bool),
    Integer(IntegerValue),
    Float(FloatValue),
    String(String),
    Tuple(Vec<Self>),
}

/// The closed set of source targets that can carry an attribute.
///
/// It covers every currently supported Item kind plus the file-scoped
/// namespace declaration. `Namespace` is kept distinct from declarations that
/// happen to live at namespace scope so the ADR 0023 default cannot silently
/// broaden into an unrestricted policy.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttributeTarget {
    Namespace,
    Function,
    Constant,
    TypeAlias,
    Attribute,
    Record,
    Union,
    Class,
    Interface,
    Enum,
    Field,
    Case,
    Method,
}

impl AttributeTarget {
    const ALL: [Self; 13] = [
        Self::Namespace,
        Self::Function,
        Self::Constant,
        Self::TypeAlias,
        Self::Attribute,
        Self::Record,
        Self::Union,
        Self::Class,
        Self::Interface,
        Self::Enum,
        Self::Field,
        Self::Case,
        Self::Method,
    ];

    #[must_use]
    pub const fn all() -> &'static [Self] {
        &Self::ALL
    }
}

/// Canonical target and repeatability policy for one nominal attribute.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeUsage {
    targets: Vec<AttributeTarget>,
    repeatable: bool,
}

impl AttributeUsage {
    #[must_use]
    pub fn new(targets: impl IntoIterator<Item = AttributeTarget>, repeatable: bool) -> Self {
        let mut targets: Vec<_> = targets.into_iter().collect();
        targets.sort_unstable();
        targets.dedup();
        Self {
            targets,
            repeatable,
        }
    }

    #[must_use]
    pub fn targets(&self) -> &[AttributeTarget] {
        &self.targets
    }

    #[must_use]
    pub fn allows(&self, target: AttributeTarget) -> bool {
        self.targets.binary_search(&target).is_ok()
    }

    #[must_use]
    pub const fn is_repeatable(&self) -> bool {
        self.repeatable
    }
}

impl Default for AttributeUsage {
    fn default() -> Self {
        Self {
            targets: vec![AttributeTarget::Namespace],
            repeatable: false,
        }
    }
}

/// An already-resolved validator function identity.
///
/// Eligibility and the compiler-defined validator signature are checked by
/// the compile-time/type integration before this identity is installed. The
/// semantic contract never stores or resolves a validator by source spelling.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttributeValidator {
    function: FunctionId,
}

impl AttributeValidator {
    #[must_use]
    pub const fn new(function: FunctionId) -> Self {
        Self { function }
    }

    #[must_use]
    pub const fn function(self) -> FunctionId {
        self.function
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeContractError {
    UnknownDefinition {
        definition: SymbolId,
    },
    UsageAlreadySpecified {
        definition: SymbolId,
    },
    ValidatorAlreadySpecified {
        definition: SymbolId,
        original: FunctionId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeDefinition {
    attribute: AttributeId,
    symbol: SymbolId,
    parameters: Vec<AttributeParameterDefinition>,
    usage: AttributeUsage,
    explicit_usage: bool,
    validator: Option<AttributeValidator>,
    span: SourceSpan,
}

impl AttributeDefinition {
    #[must_use]
    pub const fn attribute(&self) -> AttributeId {
        self.attribute
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub fn parameters(&self) -> &[AttributeParameterDefinition] {
        &self.parameters
    }

    #[must_use]
    pub const fn usage(&self) -> &AttributeUsage {
        &self.usage
    }

    #[must_use]
    pub const fn has_explicit_usage(&self) -> bool {
        self.explicit_usage
    }

    #[must_use]
    pub const fn validator(&self) -> Option<AttributeValidator> {
        self.validator
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeParameterDefinition {
    parameter: AttributeParameterId,
    name: String,
    parameter_type: TypeId,
    default_value: Option<AttributeConstant>,
    pending_default: Option<PendingConstantExpression>,
    span: SourceSpan,
}

impl AttributeParameterDefinition {
    #[must_use]
    pub const fn parameter(&self) -> AttributeParameterId {
        self.parameter
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn parameter_type(&self) -> TypeId {
        self.parameter_type
    }

    #[must_use]
    pub const fn default_value(&self) -> Option<&AttributeConstant> {
        self.default_value.as_ref()
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
pub struct AttributeDefinitionResult {
    definition: Option<AttributeDefinition>,
    diagnostics: Vec<Diagnostic>,
}

impl AttributeDefinitionResult {
    #[must_use]
    pub const fn definition(&self) -> Option<&AttributeDefinition> {
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
pub struct ResolvedAttribute {
    attribute: AttributeId,
    definition: SymbolId,
    arguments: Vec<ResolvedAttributeArgument>,
    span: SourceSpan,
}

impl ResolvedAttribute {
    #[must_use]
    pub const fn attribute(&self) -> AttributeId {
        self.attribute
    }

    #[must_use]
    pub const fn definition(&self) -> SymbolId {
        self.definition
    }

    #[must_use]
    pub fn arguments(&self) -> &[ResolvedAttributeArgument] {
        &self.arguments
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedAttributeArgument {
    name: String,
    value: AttributeConstant,
    value_type: TypeId,
    origin: SourceSpan,
    defaulted: bool,
}

impl ResolvedAttributeArgument {
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

    #[must_use]
    pub const fn is_defaulted(&self) -> bool {
        self.defaulted
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedAttributeResult {
    attribute: Option<ResolvedAttribute>,
    diagnostics: Vec<Diagnostic>,
}

impl ResolvedAttributeResult {
    #[must_use]
    pub const fn attribute(&self) -> Option<&ResolvedAttribute> {
        self.attribute.as_ref()
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeAttachmentError {
    UnknownAttribute {
        attribute: AttributeId,
        span: SourceSpan,
    },
    WrongTarget {
        attribute: AttributeId,
        target: AttributeTarget,
        span: SourceSpan,
    },
    NonRepeatableDuplicate {
        attribute: AttributeId,
        first: SourceSpan,
        duplicate: SourceSpan,
    },
}

/// A target's validated attributes in original source order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeAttachmentSet {
    attachments: Vec<ResolvedAttribute>,
    by_attribute: BTreeMap<AttributeId, Vec<ResolvedAttribute>>,
}

impl AttributeAttachmentSet {
    #[must_use]
    pub fn attachments(&self) -> &[ResolvedAttribute] {
        &self.attachments
    }

    fn new(attachments: Vec<ResolvedAttribute>) -> Self {
        let mut by_attribute = BTreeMap::<_, Vec<_>>::new();
        for attachment in &attachments {
            by_attribute
                .entry(attachment.attribute())
                .or_default()
                .push(attachment.clone());
        }
        Self {
            attachments,
            by_attribute,
        }
    }

    fn attributes(&self, attribute: AttributeId) -> &[ResolvedAttribute] {
        self.by_attribute.get(&attribute).map_or(&[], Vec::as_slice)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AttributeAttachmentResult {
    attachment_set: Option<AttributeAttachmentSet>,
    errors: Vec<AttributeAttachmentError>,
}

impl AttributeAttachmentResult {
    #[must_use]
    pub const fn attachment_set(&self) -> Option<&AttributeAttachmentSet> {
        self.attachment_set.as_ref()
    }

    #[must_use]
    pub fn errors(&self) -> &[AttributeAttachmentError] {
        &self.errors
    }
}

/// A compile-time query operand after ordinary resolution.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttributeQuerySubject {
    Symbol(SymbolId),
    Type(TypeId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeQueryError {
    UnknownModule {
        module: ModuleId,
    },
    UnknownSubject {
        subject: AttributeQuerySubject,
    },
    UnknownAttribute {
        attribute: AttributeId,
    },
    MissingVisibilityDefinition {
        definition: SymbolId,
    },
    InaccessibleSubject {
        subject: AttributeQuerySubject,
        definition: SymbolId,
    },
    InaccessibleAttribute {
        attribute: AttributeId,
        definition: SymbolId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeQueryIndexError {
    MissingVisibilityDefinition { definition: SymbolId },
    DuplicateSubject { subject: AttributeQuerySubject },
}

/// The statically selected return shape of `attribute<<A>>(subject)`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AttributeQueryValue<'attribute> {
    Optional(Option<&'attribute ResolvedAttribute>),
    ImmutableSequence(&'attribute [ResolvedAttribute]),
}

#[derive(Clone, Debug)]
struct AttributeQueryDefinition {
    definition: SymbolId,
    usage: AttributeUsage,
}

#[derive(Clone, Debug)]
struct AttributeQueryEntry {
    visibility_definition: SymbolId,
    attachments: AttributeAttachmentSet,
}

/// Immutable UDA query facts indexed exclusively by resolved compiler IDs.
#[derive(Clone, Debug)]
pub struct AttributeQueryIndex {
    database: ResolutionDatabase,
    definitions: BTreeMap<AttributeId, AttributeQueryDefinition>,
    entries: BTreeMap<AttributeQuerySubject, AttributeQueryEntry>,
}

impl AttributeQueryIndex {
    /// Registers a symbol operand and its already-validated attachments.
    ///
    /// # Errors
    ///
    /// Rejects an unknown visibility definition or a second registration for
    /// the same resolved subject.
    pub fn insert_symbol(
        &mut self,
        symbol: SymbolId,
        attachments: AttributeAttachmentSet,
    ) -> Result<(), AttributeQueryIndexError> {
        self.insert(AttributeQuerySubject::Symbol(symbol), symbol, attachments)
    }

    /// Registers a type operand with the resolved declaration that owns its
    /// visibility.
    ///
    /// # Errors
    ///
    /// Rejects an unknown visibility definition or a second registration for
    /// the same resolved subject.
    pub fn insert_type(
        &mut self,
        type_id: TypeId,
        definition: SymbolId,
        attachments: AttributeAttachmentSet,
    ) -> Result<(), AttributeQueryIndexError> {
        self.insert(
            AttributeQuerySubject::Type(type_id),
            definition,
            attachments,
        )
    }

    fn insert(
        &mut self,
        subject: AttributeQuerySubject,
        visibility_definition: SymbolId,
        attachments: AttributeAttachmentSet,
    ) -> Result<(), AttributeQueryIndexError> {
        if self
            .database
            .index()
            .declaration(visibility_definition)
            .is_none()
        {
            return Err(AttributeQueryIndexError::MissingVisibilityDefinition {
                definition: visibility_definition,
            });
        }
        if self.entries.contains_key(&subject) {
            return Err(AttributeQueryIndexError::DuplicateSubject { subject });
        }
        self.entries.insert(
            subject,
            AttributeQueryEntry {
                visibility_definition,
                attachments,
            },
        );
        Ok(())
    }

    /// Performs the typed `attribute<<A>>(subject)` query.
    ///
    /// # Errors
    ///
    /// Rejects unknown resolved IDs and any attribute or subject that is not
    /// visible from `module`.
    pub fn attribute(
        &self,
        module: ModuleId,
        subject: AttributeQuerySubject,
        attribute: AttributeId,
    ) -> Result<AttributeQueryValue<'_>, AttributeQueryError> {
        let (definition, entry) = self.check_query_access(module, subject, attribute)?;
        let attributes = entry.attachments.attributes(attribute);
        if definition.usage.is_repeatable() {
            Ok(AttributeQueryValue::ImmutableSequence(attributes))
        } else {
            debug_assert!(attributes.len() <= 1);
            Ok(AttributeQueryValue::Optional(attributes.first()))
        }
    }

    /// Performs the typed `hasAttribute<<A>>(subject)` query.
    ///
    /// # Errors
    ///
    /// Rejects unknown resolved IDs and any attribute or subject that is not
    /// visible from `module`.
    pub fn has_attribute(
        &self,
        module: ModuleId,
        subject: AttributeQuerySubject,
        attribute: AttributeId,
    ) -> Result<bool, AttributeQueryError> {
        let (_, entry) = self.check_query_access(module, subject, attribute)?;
        Ok(!entry.attachments.attributes(attribute).is_empty())
    }

    fn check_query_access(
        &self,
        module: ModuleId,
        subject: AttributeQuerySubject,
        attribute: AttributeId,
    ) -> Result<(&AttributeQueryDefinition, &AttributeQueryEntry), AttributeQueryError> {
        let Some(module_context) = self.database.index().module(module) else {
            return Err(AttributeQueryError::UnknownModule { module });
        };
        let Some(definition) = self.definitions.get(&attribute) else {
            return Err(AttributeQueryError::UnknownAttribute { attribute });
        };
        let Some(attribute_declaration) = self.database.index().declaration(definition.definition)
        else {
            return Err(AttributeQueryError::MissingVisibilityDefinition {
                definition: definition.definition,
            });
        };
        if !attribute_declaration.is_accessible_from(module, module_context.bubble()) {
            return Err(AttributeQueryError::InaccessibleAttribute {
                attribute,
                definition: definition.definition,
            });
        }
        let Some(entry) = self.entries.get(&subject) else {
            return Err(AttributeQueryError::UnknownSubject { subject });
        };
        let Some(subject_declaration) = self
            .database
            .index()
            .declaration(entry.visibility_definition)
        else {
            return Err(AttributeQueryError::MissingVisibilityDefinition {
                definition: entry.visibility_definition,
            });
        };
        if !subject_declaration.is_accessible_from(module, module_context.bubble()) {
            return Err(AttributeQueryError::InaccessibleSubject {
                subject,
                definition: entry.visibility_definition,
            });
        }
        Ok((definition, entry))
    }
}

struct EvaluatedAttributeArguments {
    supplied: BTreeMap<usize, (AttributeConstant, SourceSpan)>,
    invalid_supplied: BTreeSet<usize>,
}

impl SignatureResolver<'_> {
    #[must_use]
    pub fn attribute_definition(&self, symbol: SymbolId) -> Option<&AttributeDefinition> {
        self.attribute_definitions.get(&symbol)
    }

    /// Replaces the restrictive ADR 0023 default with an explicitly resolved
    /// `@AttributeUsage` contract.
    ///
    /// # Errors
    ///
    /// Rejects an unknown attribute declaration or a second explicit usage
    /// contract on the same declaration.
    pub fn install_attribute_usage(
        &mut self,
        definition: SymbolId,
        usage: AttributeUsage,
    ) -> Result<(), AttributeContractError> {
        let Some(attribute) = self.attribute_definitions.get_mut(&definition) else {
            return Err(AttributeContractError::UnknownDefinition { definition });
        };
        if attribute.explicit_usage {
            return Err(AttributeContractError::UsageAlreadySpecified { definition });
        }
        attribute.usage = usage;
        attribute.explicit_usage = true;
        Ok(())
    }

    /// Installs the resolved function identity carried by one trusted
    /// `@AttributeValidator` attachment.
    ///
    /// # Errors
    ///
    /// Rejects an unknown attribute declaration or a second validator on the
    /// same declaration.
    pub fn install_attribute_validator(
        &mut self,
        definition: SymbolId,
        validator: AttributeValidator,
    ) -> Result<(), AttributeContractError> {
        let Some(attribute) = self.attribute_definitions.get_mut(&definition) else {
            return Err(AttributeContractError::UnknownDefinition { definition });
        };
        if let Some(original) = attribute.validator {
            return Err(AttributeContractError::ValidatorAlreadySpecified {
                definition,
                original: original.function(),
            });
        }
        attribute.validator = Some(validator);
        Ok(())
    }

    /// Validates target permissions and repeatability without reordering or
    /// dropping any recognized attachment.
    #[must_use]
    pub fn validate_attribute_attachments(
        &self,
        target: AttributeTarget,
        attachments: impl IntoIterator<Item = ResolvedAttribute>,
    ) -> AttributeAttachmentResult {
        let attachments: Vec<_> = attachments.into_iter().collect();
        let mut errors = Vec::new();
        let mut first_by_attribute = BTreeMap::new();
        for attachment in &attachments {
            let Some(definition) = self
                .attribute_definitions
                .values()
                .find(|definition| definition.attribute == attachment.attribute)
            else {
                errors.push(AttributeAttachmentError::UnknownAttribute {
                    attribute: attachment.attribute,
                    span: attachment.span,
                });
                continue;
            };
            if !definition.usage.allows(target) {
                errors.push(AttributeAttachmentError::WrongTarget {
                    attribute: attachment.attribute,
                    target,
                    span: attachment.span,
                });
                continue;
            }
            if let Some(first) = first_by_attribute.get(&attachment.attribute).copied() {
                if !definition.usage.is_repeatable() {
                    errors.push(AttributeAttachmentError::NonRepeatableDuplicate {
                        attribute: attachment.attribute,
                        first,
                        duplicate: attachment.span,
                    });
                }
            } else {
                first_by_attribute.insert(attachment.attribute, attachment.span);
            }
        }
        let attachment_set = errors
            .is_empty()
            .then(|| AttributeAttachmentSet::new(attachments));
        AttributeAttachmentResult {
            attachment_set,
            errors,
        }
    }

    /// Creates an initially empty, visibility-preserving query index from all
    /// known nominal attribute definitions.
    #[must_use]
    pub fn attribute_query_index(&self) -> AttributeQueryIndex {
        let definitions = self
            .attribute_definitions
            .values()
            .map(|definition| {
                (
                    definition.attribute,
                    AttributeQueryDefinition {
                        definition: definition.symbol,
                        usage: definition.usage.clone(),
                    },
                )
            })
            .collect();
        AttributeQueryIndex {
            database: self.database().clone(),
            definitions,
            entries: BTreeMap::new(),
        }
    }

    #[must_use]
    pub fn define_attribute(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &AttributeDeclarationSyntax,
    ) -> AttributeDefinitionResult {
        self.define_attribute_impl(module, symbol, syntax, false)
    }

    #[must_use]
    pub fn define_attribute_schema(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &AttributeDeclarationSyntax,
    ) -> AttributeDefinitionResult {
        self.define_attribute_impl(module, symbol, syntax, true)
    }

    fn define_attribute_impl(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &AttributeDeclarationSyntax,
        defer_defaults: bool,
    ) -> AttributeDefinitionResult {
        let mut diagnostics = Vec::new();
        let generics = BTreeMap::new();
        let mut parameters = Vec::new();
        let mut names = BTreeMap::new();
        for parameter in syntax.parameters() {
            if let Some(original) = names.insert(parameter.name().to_owned(), parameter.span()) {
                diagnostics.push(type_diagnostics::duplicate_attribute_argument(
                    parameter.span(),
                    parameter.name(),
                    original,
                ));
                continue;
            }
            let Some(resolved) = self.resolve_type(
                module,
                parameter.parameter_type(),
                &generics,
                &mut diagnostics,
            ) else {
                continue;
            };
            let Some(parameter_type) = resolved.type_id() else {
                continue;
            };
            let parameter_id =
                AttributeParameterId::from_raw(u32::try_from(parameters.len()).unwrap_or(u32::MAX));
            let (default_value, pending_default) = match parameter.default_value() {
                Some(value) if defer_defaults => (
                    None,
                    Some(PendingConstantExpression::new(
                        value.clone(),
                        parameter_type,
                    )),
                ),
                Some(value) => (
                    self.resolve_constant(
                        value,
                        parameter_type,
                        parameter.span(),
                        &mut diagnostics,
                    ),
                    None,
                ),
                None => (None, None),
            };
            parameters.push(AttributeParameterDefinition {
                parameter: parameter_id,
                name: parameter.name().to_owned(),
                parameter_type,
                default_value,
                pending_default,
                span: parameter.span(),
            });
        }
        let definition = diagnostics.is_empty().then(|| AttributeDefinition {
            attribute: AttributeId::from_raw(self.next_attribute),
            symbol,
            parameters,
            usage: AttributeUsage::default(),
            explicit_usage: false,
            validator: None,
            span: syntax.span(),
        });
        if let Some(definition) = &definition {
            self.next_attribute = self.next_attribute.saturating_add(1);
            self.attribute_definitions
                .insert(symbol, definition.clone());
        }
        AttributeDefinitionResult {
            definition,
            diagnostics,
        }
    }

    /// Installs one already-evaluated attribute default into a deferred schema.
    ///
    /// # Errors
    ///
    /// Rejects unknown identities, a non-pending target, or a value whose
    /// canonical type does not match the parameter type.
    pub fn install_attribute_parameter_default(
        &mut self,
        definition: SymbolId,
        parameter: AttributeParameterId,
        value: AttributeConstant,
    ) -> Result<(), RequiredConstantError> {
        let target = RequiredConstantTarget::AttributeParameter {
            definition,
            parameter,
        };
        let Some(attribute) = self.attribute_definitions.get(&definition) else {
            return Err(RequiredConstantError::UnknownTarget(target));
        };
        let Some(index) = attribute
            .parameters
            .iter()
            .position(|candidate| candidate.parameter == parameter)
        else {
            return Err(RequiredConstantError::UnknownTarget(target));
        };
        let expected = attribute.parameters[index].parameter_type;
        if attribute.parameters[index].pending_default.is_none() {
            return Err(RequiredConstantError::NoPendingDefault(target));
        }
        if !attribute_constant_matches_type(self.arena(), &value, expected) {
            return Err(RequiredConstantError::TypeMismatch { target, expected });
        }
        let parameter = self
            .attribute_definitions
            .get_mut(&definition)
            .and_then(|attribute| attribute.parameters.get_mut(index))
            .ok_or(RequiredConstantError::UnknownTarget(target))?;
        parameter.default_value = Some(value);
        parameter.pending_default = None;
        Ok(())
    }

    #[must_use]
    pub fn resolve_attribute_use(
        &self,
        module: ModuleId,
        syntax: &AttributeUseSyntax,
    ) -> ResolvedAttributeResult {
        self.resolve_attribute_use_impl(module, syntax, |expression, expected, expected_origin| {
            let mut diagnostics = Vec::new();
            self.resolve_constant(expression, expected, expected_origin, &mut diagnostics)
                .ok_or(diagnostics)
        })
    }

    #[must_use]
    pub fn resolve_attribute_use_with_evaluator(
        &self,
        module: ModuleId,
        syntax: &AttributeUseSyntax,
        mut evaluator: impl FnMut(
            &ExpressionSyntax,
            TypeId,
        ) -> Result<AttributeConstant, Vec<Diagnostic>>,
    ) -> ResolvedAttributeResult {
        self.resolve_attribute_use_impl(module, syntax, |expression, expected, _| {
            evaluator(expression, expected)
        })
    }

    fn resolve_attribute_use_impl(
        &self,
        module: ModuleId,
        syntax: &AttributeUseSyntax,
        mut evaluator: impl FnMut(
            &ExpressionSyntax,
            TypeId,
            SourceSpan,
        ) -> Result<AttributeConstant, Vec<Diagnostic>>,
    ) -> ResolvedAttributeResult {
        let mut diagnostics = Vec::new();
        let Some(definition) = self.resolve_attribute_definition(module, syntax, &mut diagnostics)
        else {
            return ResolvedAttributeResult {
                attribute: None,
                diagnostics,
            };
        };
        let EvaluatedAttributeArguments {
            mut supplied,
            invalid_supplied,
        } = self.evaluate_attribute_arguments(syntax, definition, &mut evaluator, &mut diagnostics);
        let mut arguments = Vec::new();
        for (index, parameter) in definition.parameters().iter().enumerate() {
            if let Some((value, origin)) = supplied.remove(&index) {
                arguments.push(ResolvedAttributeArgument {
                    name: parameter.name().to_owned(),
                    value,
                    value_type: parameter.parameter_type(),
                    origin,
                    defaulted: false,
                });
            } else if !invalid_supplied.contains(&index) {
                if let Some(value) = parameter.default_value().cloned() {
                    arguments.push(ResolvedAttributeArgument {
                        name: parameter.name().to_owned(),
                        value,
                        value_type: parameter.parameter_type(),
                        origin: parameter.span(),
                        defaulted: true,
                    });
                } else {
                    diagnostics.push(type_diagnostics::wrong_value_arity(
                        syntax.span(),
                        "attribute",
                        definition.parameters().len(),
                        syntax.arguments().len(),
                    ));
                }
            }
        }
        sort_diagnostics(&mut diagnostics);
        let attribute = diagnostics.is_empty().then(|| ResolvedAttribute {
            attribute: definition.attribute(),
            definition: definition.symbol(),
            arguments,
            span: syntax.span(),
        });
        ResolvedAttributeResult {
            attribute,
            diagnostics,
        }
    }

    fn evaluate_attribute_arguments(
        &self,
        syntax: &AttributeUseSyntax,
        definition: &AttributeDefinition,
        evaluator: &mut impl FnMut(
            &ExpressionSyntax,
            TypeId,
            SourceSpan,
        ) -> Result<AttributeConstant, Vec<Diagnostic>>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> EvaluatedAttributeArguments {
        let mut supplied = BTreeMap::new();
        let mut invalid_supplied = BTreeSet::new();
        let mut next_positional = 0;
        for argument in syntax.arguments() {
            let index = if let Some(name) = argument.name() {
                let Some(index) = definition
                    .parameters()
                    .iter()
                    .position(|parameter| parameter.name() == name)
                else {
                    diagnostics.push(type_diagnostics::unknown_attribute_argument(
                        argument.span(),
                        name,
                    ));
                    continue;
                };
                index
            } else {
                let index = next_positional;
                next_positional += 1;
                index
            };
            let Some(parameter) = definition.parameters().get(index) else {
                diagnostics.push(type_diagnostics::wrong_value_arity(
                    argument.span(),
                    "attribute",
                    definition.parameters().len(),
                    syntax.arguments().len(),
                ));
                continue;
            };
            if let Some((_, original)) = supplied.get(&index) {
                diagnostics.push(type_diagnostics::duplicate_attribute_argument(
                    argument.span(),
                    parameter.name(),
                    *original,
                ));
                continue;
            }
            match evaluator(
                argument.value(),
                parameter.parameter_type(),
                parameter.span(),
            ) {
                Ok(value)
                    if attribute_constant_matches_type(
                        self.arena(),
                        &value,
                        parameter.parameter_type(),
                    ) =>
                {
                    supplied.insert(index, (value, argument.span()));
                }
                Ok(_) => {
                    Self::constant_mismatch(
                        argument.value(),
                        parameter.parameter_type(),
                        parameter.span(),
                        diagnostics,
                    );
                    invalid_supplied.insert(index);
                }
                Err(evaluation_diagnostics) => {
                    diagnostics.extend(evaluation_diagnostics);
                    invalid_supplied.insert(index);
                }
            }
        }
        EvaluatedAttributeArguments {
            supplied,
            invalid_supplied,
        }
    }

    fn resolve_attribute_definition<'definition>(
        &'definition self,
        module: ModuleId,
        syntax: &AttributeUseSyntax,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<&'definition AttributeDefinition> {
        let name = syntax.path().join(".");
        let resolution = self
            .database()
            .resolve(module, &name, SymbolSpace::Type, syntax.span());
        if !resolution.diagnostics().is_empty() {
            diagnostics.extend(resolution.diagnostics().iter().cloned());
            return None;
        }
        let symbol = resolution.symbol()?;
        let Some(definition) = self.attribute_definitions.get(&symbol) else {
            diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                syntax.span(),
                format!("attribute type {name}"),
            ));
            return None;
        };
        Some(definition)
    }

    fn resolve_constant(
        &self,
        syntax: &ExpressionSyntax,
        expected: TypeId,
        expected_origin: SourceSpan,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<AttributeConstant> {
        let diagnostic_count = diagnostics.len();
        let resolved = match syntax.kind() {
            ExpressionSyntaxKind::Integer(value) => {
                self.resolve_numeric_constant(value, expected, syntax, diagnostics)
            }
            ExpressionSyntaxKind::Unary {
                operator: UnaryOperator::Negate,
                operand,
            } if matches!(operand.kind(), ExpressionSyntaxKind::Integer(_)) => {
                let ExpressionSyntaxKind::Integer(value) = operand.kind() else {
                    unreachable!("guarded integer literal")
                };
                self.resolve_numeric_constant(&format!("-{value}"), expected, syntax, diagnostics)
            }
            ExpressionSyntaxKind::String(value)
                if self.is_primitive(expected, PrimitiveType::String) =>
            {
                Some(AttributeConstant::String(unquote(value)))
            }
            ExpressionSyntaxKind::Boolean(value)
                if self.is_primitive(expected, PrimitiveType::Boolean) =>
            {
                Some(AttributeConstant::Boolean(*value))
            }
            ExpressionSyntaxKind::Nil if self.is_primitive(expected, PrimitiveType::Nil) => {
                Some(AttributeConstant::Nil)
            }
            ExpressionSyntaxKind::Tuple(elements) => {
                let Some(SemanticType::Tuple(types)) = self.arena().get(expected) else {
                    return Self::constant_mismatch(syntax, expected, expected_origin, diagnostics);
                };
                if types.len() != elements.len() {
                    return Self::constant_mismatch(syntax, expected, expected_origin, diagnostics);
                }
                let values: Option<Vec<_>> = elements
                    .iter()
                    .zip(types)
                    .map(|(element, element_type)| {
                        self.resolve_constant(element, *element_type, expected_origin, diagnostics)
                    })
                    .collect();
                values.map(AttributeConstant::Tuple)
            }
            _ => None,
        };
        if resolved.is_none() && diagnostics.len() == diagnostic_count {
            Self::constant_mismatch(syntax, expected, expected_origin, diagnostics)
        } else {
            resolved
        }
    }

    fn resolve_numeric_constant(
        &self,
        literal: &str,
        expected: TypeId,
        syntax: &ExpressionSyntax,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<AttributeConstant> {
        let value = match self.arena().get(expected) {
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => {
                IntegerValue::parse_decimal(literal, *kind).map(AttributeConstant::Integer)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float32)) => {
                FloatValue::parse_decimal(literal, FloatKind::Float32).map(AttributeConstant::Float)
            }
            Some(SemanticType::Primitive(PrimitiveType::Float64)) => {
                FloatValue::parse_decimal(literal, FloatKind::Float64).map(AttributeConstant::Float)
            }
            _ => return None,
        };
        match value {
            Ok(value) => Some(value),
            Err(
                NumericError::InvalidLiteral
                | NumericError::OutOfRange
                | NumericError::Overflow
                | NumericError::KindMismatch
                | NumericError::DivisionByZero,
            ) => {
                diagnostics.push(compile_time_diagnostics::constant_integer_overflow(
                    syntax.span(),
                    "attribute constant",
                ));
                None
            }
        }
    }

    fn constant_mismatch(
        syntax: &ExpressionSyntax,
        expected: TypeId,
        expected_origin: SourceSpan,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<AttributeConstant> {
        diagnostics.push(type_diagnostics::type_mismatch(
            syntax.span(),
            type_name(expected),
            "attribute constant",
            expected_origin,
        ));
        None
    }

    fn is_primitive(&self, type_id: TypeId, primitive: PrimitiveType) -> bool {
        self.arena().get(type_id) == Some(&SemanticType::Primitive(primitive))
    }
}

fn unquote(value: &str) -> String {
    value
        .get(1..value.len().saturating_sub(1))
        .unwrap_or_default()
        .to_owned()
}

fn type_name(type_id: TypeId) -> String {
    format!("type#{}", type_id.raw())
}

fn sort_diagnostics(diagnostics: &mut [Diagnostic]) {
    diagnostics.sort_by_key(|diagnostic| {
        let span = diagnostic.primary_span();
        (
            span.file(),
            span.range().start(),
            diagnostic.code().as_str(),
        )
    });
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
