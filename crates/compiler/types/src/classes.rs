use std::collections::BTreeMap;

use pop_diagnostics::types as type_diagnostics;
use pop_foundation::{
    BubbleId, ClassId, Diagnostic, FieldId, MethodId, ModuleId, ParameterId, SourceSpan, SymbolId,
    TypeId,
};
use pop_resolve::Visibility;
use pop_syntax::{ClassDeclarationSyntax, ClassMethodDispatchSyntax, VisibilitySyntax};
use serde::{Deserialize, Serialize};

use crate::field_defaults::resolve_field_default;
use crate::required_constants::field_default_matches_type;
use crate::{
    ClassBuiltinInterfaceImplementation, ClassInterfaceImplementation, FieldDefault,
    PendingConstantExpression, RequiredConstantError, RequiredConstantTarget,
    ResolvedFunctionSignature, SemanticType, SignatureResolver,
};

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum ClassMethodDispatch {
    Static,
    Receiver,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassDefinition {
    symbol: SymbolId,
    source_symbol: SymbolId,
    module: ModuleId,
    bubble: BubbleId,
    class: ClassId,
    type_id: TypeId,
    type_parameters: Vec<crate::ResolvedTypeParameter>,
    is_open: bool,
    interfaces: Vec<ClassInterfaceImplementation>,
    builtin_interfaces: Vec<ClassBuiltinInterfaceImplementation>,
    fields: Vec<ClassFieldDefinition>,
    methods: Vec<ClassMethodDefinition>,
    span: SourceSpan,
}

impl ClassDefinition {
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn source_symbol(&self) -> SymbolId {
        self.source_symbol
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
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[crate::ResolvedTypeParameter] {
        &self.type_parameters
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.is_open
    }

    #[must_use]
    pub fn interfaces(&self) -> &[ClassInterfaceImplementation] {
        &self.interfaces
    }

    #[must_use]
    pub fn builtin_interfaces(&self) -> &[ClassBuiltinInterfaceImplementation] {
        &self.builtin_interfaces
    }

    #[must_use]
    pub fn fields(&self) -> &[ClassFieldDefinition] {
        &self.fields
    }

    #[must_use]
    pub fn methods(&self) -> &[ClassMethodDefinition] {
        &self.methods
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassFieldDefinition {
    field: FieldId,
    visibility: Visibility,
    name: String,
    field_type: TypeId,
    default: Option<FieldDefault>,
    pending_default: Option<PendingConstantExpression>,
    span: SourceSpan,
}

impl ClassFieldDefinition {
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassMethodDefinition {
    method: MethodId,
    visibility: Visibility,
    name: String,
    dispatch: ClassMethodDispatch,
    parameters: Vec<(String, TypeId, SourceSpan)>,
    results: Vec<TypeId>,
    span: SourceSpan,
}

impl ClassMethodDefinition {
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
    pub fn parameters(&self) -> &[(String, TypeId, SourceSpan)] {
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

#[derive(Clone, Debug)]
pub struct ClassDefinitionResult {
    definition: Option<ClassDefinition>,
    diagnostics: Vec<Diagnostic>,
}

impl ClassDefinitionResult {
    #[must_use]
    pub const fn definition(&self) -> Option<&ClassDefinition> {
        self.definition.as_ref()
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        let mut output = String::new();
        for diagnostic in &self.diagnostics {
            let range = diagnostic.primary_span().range();
            output.push_str(diagnostic.code().as_str());
            output.push('@');
            output.push_str(&range.start().to_u32().to_string());
            output.push_str("..");
            output.push_str(&range.end().to_u32().to_string());
            output.push('\n');
        }
        output
    }
}

impl SignatureResolver<'_> {
    #[must_use]
    pub fn class_definition(&self, symbol: SymbolId) -> Option<&ClassDefinition> {
        self.class_definitions.get(&symbol)
    }

    #[must_use]
    pub fn class_definition_for_type(&self, type_id: TypeId) -> Option<&ClassDefinition> {
        self.classes_by_type
            .get(&type_id)
            .and_then(|symbol| self.class_definitions.get(symbol))
    }

    pub fn class_definitions(&self) -> impl Iterator<Item = &ClassDefinition> {
        self.class_definitions.values()
    }

    pub fn class_instances(&self, definition: SymbolId) -> impl Iterator<Item = &ClassDefinition> {
        self.class_instances
            .iter()
            .filter(move |((source, _), _)| *source == definition)
            .filter_map(|(_, symbol)| self.class_definitions.get(symbol))
    }

    #[must_use]
    pub fn class_is_generic(&self, definition: SymbolId) -> bool {
        self.class_type_parameters
            .get(&definition)
            .is_some_and(|parameters| !parameters.is_empty())
    }

    #[must_use]
    pub fn class_instance_source(&self, instance: SymbolId) -> Option<SymbolId> {
        self.class_instance_sources.get(&instance).copied()
    }

    #[must_use]
    pub fn class_source_identity(&self, class: ClassId) -> Option<SymbolId> {
        let symbol = self
            .class_definitions
            .values()
            .find(|definition| definition.class() == class)?
            .symbol();
        Some(
            self.class_instance_sources
                .get(&symbol)
                .copied()
                .unwrap_or(symbol),
        )
    }

    pub(crate) fn validate_class_arguments(
        &mut self,
        definition: SymbolId,
        arguments: &[TypeId],
        span: SourceSpan,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> bool {
        let Some(parameters) = self.class_type_parameters.get(&definition).cloned() else {
            return arguments.is_empty();
        };
        if parameters.len() != arguments.len() {
            return false;
        }
        let substitutions: BTreeMap<_, _> = parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| (parameter.parameter(), *argument))
            .collect();
        for (parameter, argument) in parameters.iter().zip(arguments) {
            let Some(bound) = parameter.bound() else {
                continue;
            };
            let Some(bound) = self.substitute_type_parameters(bound, &substitutions) else {
                diagnostics.push(type_diagnostics::generic_inference_failure(
                    span,
                    parameter.name(),
                    "nominal interface bound cannot be specialized",
                ));
                return false;
            };
            if !self.type_satisfies_nominal_bound(*argument, bound) {
                diagnostics.push(type_diagnostics::generic_inference_failure(
                    span,
                    parameter.name(),
                    "nominal interface bound is not satisfied",
                ));
                return false;
            }
        }
        true
    }

    pub(crate) fn type_satisfies_nominal_bound(&self, actual: TypeId, bound: TypeId) -> bool {
        if actual == bound {
            return true;
        }
        match (self.arena().get(actual), self.arena().get(bound)) {
            (Some(SemanticType::Class { .. }), Some(SemanticType::Interface { .. })) => self
                .class_definition_for_type(actual)
                .is_some_and(|definition| {
                    definition
                        .interfaces()
                        .iter()
                        .any(|implementation| implementation.interface_type() == bound)
                }),
            (
                Some(SemanticType::Class { .. }),
                Some(SemanticType::Builtin {
                    definition,
                    arguments,
                }),
            ) => self.class_definition_for_type(actual).is_some_and(|class| {
                class.builtin_interfaces().iter().any(|implementation| {
                    implementation.interface_type() == bound
                        || self.schema().iteration_protocol().is_some_and(|protocol| {
                            *definition == protocol.iterable()
                                && implementation.interface() == protocol.iterator()
                                && matches!(
                                    self.arena().get(implementation.interface_type()),
                                    Some(SemanticType::Builtin {
                                        arguments: implemented_arguments,
                                        ..
                                    }) if implemented_arguments == arguments
                                )
                        })
                })
            }),
            (
                Some(SemanticType::Array(element)),
                Some(SemanticType::Builtin {
                    definition,
                    arguments,
                }),
            ) => self.schema().iteration_protocol().is_some_and(|protocol| {
                *definition == protocol.iterable() && arguments.as_slice() == [*element]
            }),
            (
                Some(SemanticType::Builtin {
                    definition: actual_definition,
                    arguments: actual_arguments,
                }),
                Some(SemanticType::Builtin {
                    definition: bound_definition,
                    arguments: bound_arguments,
                }),
            ) => self.schema().iteration_protocol().is_some_and(|protocol| {
                actual_arguments == bound_arguments
                    && ((*bound_definition == protocol.iterable()
                        && matches!(
                            *actual_definition,
                            definition
                                if definition == protocol.list()
                                    || definition == protocol.iterable()
                                    || definition == protocol.iterator()
                        ))
                        || (*bound_definition == protocol.iterator()
                            && *actual_definition == protocol.iterator()))
            }),
            _ => false,
        }
    }

    #[must_use]
    pub fn is_class_to_builtin_interface_upcast(&self, source: TypeId, target: TypeId) -> bool {
        matches!(
            (self.arena().get(source), self.arena().get(target)),
            (
                Some(SemanticType::Class { .. }),
                Some(SemanticType::Builtin { .. })
            )
        ) && self.type_satisfies_nominal_bound(source, target)
    }

    pub fn instantiate_class(
        &mut self,
        definition: SymbolId,
        arguments: &[TypeId],
    ) -> Option<ClassDefinition> {
        let key = (definition, arguments.to_vec());
        if let Some(symbol) = self.class_instances.get(&key) {
            return self.class_definitions.get(symbol).cloned();
        }
        let parameters = self.class_type_parameters.get(&definition)?.clone();
        if parameters.len() != arguments.len() {
            return None;
        }
        if parameters
            .iter()
            .map(crate::ResolvedTypeParameter::type_id)
            .eq(arguments.iter().copied())
        {
            return self.class_definitions.get(&definition).cloned();
        }
        let substitutions: BTreeMap<_, _> = parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| (parameter.parameter(), *argument))
            .collect();
        let template = self.class_definitions.get(&definition)?.clone();
        let symbol = SymbolId::from_raw(self.next_instance_symbol);
        self.next_instance_symbol = self.next_instance_symbol.saturating_add(1);
        let class = ClassId::from_raw(self.next_class);
        self.next_class = self.next_class.saturating_add(1);
        let type_id = self
            .arena
            .intern(SemanticType::Class {
                class,
                arguments: arguments.to_vec(),
            })
            .ok()?;
        self.arena
            .register_class_specialization(template.class, arguments.to_vec(), type_id)
            .ok()?;

        let active_key = (template.class, arguments.to_vec());
        self.active_class_specializations
            .insert(active_key.clone(), type_id);
        let specialized = (|| {
            let mut fields = template.fields.clone();
            for field in &mut fields {
                field.field_type =
                    self.substitute_type_parameters(field.field_type, &substitutions)?;
                if let Some(pending) = field.pending_default.take() {
                    field.pending_default = Some(PendingConstantExpression::new(
                        pending.expression().clone(),
                        field.field_type,
                    ));
                }
                field.field = FieldId::from_raw(self.next_field);
                self.next_field = self.next_field.saturating_add(1);
            }

            let mut method_ids = BTreeMap::new();
            let mut methods = template.methods.clone();
            for method in &mut methods {
                let specialized = MethodId::from_raw(self.next_method);
                self.next_method = self.next_method.saturating_add(1);
                method_ids.insert(method.method, specialized);
                method.method = specialized;
                for (_, parameter_type, _) in &mut method.parameters {
                    *parameter_type =
                        self.substitute_type_parameters(*parameter_type, &substitutions)?;
                }
                for result in &mut method.results {
                    *result = self.substitute_type_parameters(*result, &substitutions)?;
                }
            }
            let interfaces = template
                .interfaces
                .iter()
                .map(|implementation| {
                    let interface_type = self.substitute_type_parameters(
                        implementation.interface_type(),
                        &substitutions,
                    )?;
                    let interface = self.interface_definition_for_type(interface_type)?.clone();
                    implementation.specialize(&interface, &method_ids)
                })
                .collect::<Option<Vec<_>>>()?;
            let builtin_interfaces = template
                .builtin_interfaces
                .iter()
                .map(|implementation| {
                    let interface_type = self.substitute_type_parameters(
                        implementation.interface_type(),
                        &substitutions,
                    )?;
                    implementation.specialize(interface_type, &method_ids)
                })
                .collect::<Option<Vec<_>>>()?;
            Some((fields, methods, interfaces, builtin_interfaces))
        })();
        self.active_class_specializations.remove(&active_key);
        let (fields, methods, interfaces, builtin_interfaces) = specialized?;
        let instance = ClassDefinition {
            symbol,
            source_symbol: template.source_symbol,
            module: template.module,
            bubble: template.bubble,
            class,
            type_id,
            type_parameters: Vec::new(),
            is_open: template.is_open,
            interfaces,
            builtin_interfaces,
            fields,
            methods,
            span: template.span,
        };
        self.class_instances.insert(key, symbol);
        self.class_instance_sources.insert(symbol, definition);
        self.class_types.insert(symbol, type_id);
        self.classes_by_type.insert(type_id, symbol);
        self.class_definitions.insert(symbol, instance.clone());
        if arguments
            .iter()
            .any(|argument| self.arena.contains_type_parameter(*argument))
        {
            for substitutions in self.generic_call_substitutions.clone() {
                let Some(concrete_arguments) = arguments
                    .iter()
                    .map(|argument| self.substitute_type_parameters(*argument, &substitutions))
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                if concrete_arguments != arguments
                    && concrete_arguments
                        .iter()
                        .all(|argument| !self.arena.contains_type_parameter(*argument))
                {
                    self.instantiate_class(definition, &concrete_arguments)?;
                }
            }
        }
        Some(instance)
    }

    pub(crate) fn materialize_class_instances_for_substitutions(
        &mut self,
        substitutions: &BTreeMap<pop_foundation::ParameterId, TypeId>,
    ) -> Option<()> {
        if !self.generic_call_substitutions.contains(substitutions) {
            self.generic_call_substitutions.push(substitutions.clone());
        }
        let symbolic_instances = self
            .class_instances
            .keys()
            .filter(|(_, arguments)| {
                arguments
                    .iter()
                    .any(|argument| self.arena.contains_type_parameter(*argument))
            })
            .cloned()
            .collect::<Vec<_>>();
        for (source, arguments) in symbolic_instances {
            let Some(concrete_arguments) = arguments
                .iter()
                .map(|argument| self.substitute_type_parameters(*argument, substitutions))
                .collect::<Option<Vec<_>>>()
            else {
                continue;
            };
            if concrete_arguments != arguments
                && concrete_arguments
                    .iter()
                    .all(|argument| !self.arena.contains_type_parameter(*argument))
            {
                self.instantiate_class(source, &concrete_arguments)?;
            }
        }
        Some(())
    }

    #[must_use]
    pub fn method_signature(
        &self,
        definition: &ClassDefinition,
        method: &ClassMethodDefinition,
    ) -> ResolvedFunctionSignature {
        let mut parameters = Vec::new();
        if method.dispatch() == ClassMethodDispatch::Receiver {
            parameters.push(("self".to_owned(), definition.type_id(), method.span()));
        }
        parameters.extend(method.parameters().iter().cloned());
        let separator = if method.dispatch() == ClassMethodDispatch::Receiver {
            ':'
        } else {
            '.'
        };
        ResolvedFunctionSignature::canonical_generic(
            definition.symbol(),
            format!("{}{separator}{}", definition.class().raw(), method.name()),
            definition.type_parameters().to_vec(),
            parameters,
            method
                .results()
                .iter()
                .map(|result| (*result, method.span()))
                .collect(),
        )
    }

    #[must_use]
    /// Defines one nominal class and its resolved native members.
    ///
    /// # Panics
    ///
    /// Panics only if the type arena rejects a fresh class identity with no
    /// referenced types, which is an internal compiler invariant violation.
    pub fn define_class(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &ClassDeclarationSyntax,
    ) -> ClassDefinitionResult {
        self.define_class_impl(module, symbol, syntax, false)
    }

    #[must_use]
    pub fn define_class_schema(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &ClassDeclarationSyntax,
    ) -> ClassDefinitionResult {
        self.define_class_impl(module, symbol, syntax, true)
    }

    /// Reconstructs one artifact-verified public class and its exact nominal
    /// interface witnesses. Fields, methods, and runtime descriptors remain in
    /// the linked implementation and are not exposed as reflection metadata.
    #[must_use]
    pub fn define_referenced_class(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        type_parameters: Vec<crate::ResolvedTypeParameter>,
        is_open: bool,
        interface_types: Vec<TypeId>,
        span: SourceSpan,
    ) -> Option<ClassDefinition> {
        if self.class_definitions.contains_key(&symbol) {
            return None;
        }
        let interfaces = interface_types
            .into_iter()
            .map(|interface_type| {
                let definition = self.interface_definition_for_type(interface_type)?;
                Some(ClassInterfaceImplementation::referenced(
                    definition.interface(),
                    interface_type,
                ))
            })
            .collect::<Option<Vec<_>>>()?;
        let class = ClassId::from_raw(self.next_class);
        self.next_class = self.next_class.saturating_add(1);
        let arguments = type_parameters
            .iter()
            .map(crate::ResolvedTypeParameter::type_id)
            .collect::<Vec<_>>();
        let type_id = self
            .arena
            .intern(SemanticType::Class { class, arguments })
            .ok()?;
        let bubble = self.database().index().declaration(symbol)?.bubble();
        let definition = ClassDefinition {
            symbol,
            source_symbol: symbol,
            module,
            bubble,
            class,
            type_id,
            type_parameters: type_parameters.clone(),
            is_open,
            interfaces,
            builtin_interfaces: Vec::new(),
            fields: Vec::new(),
            methods: Vec::new(),
            span,
        };
        self.class_types.insert(symbol, type_id);
        self.class_type_parameters.insert(symbol, type_parameters);
        self.classes_by_type.insert(type_id, symbol);
        self.class_definitions.insert(symbol, definition.clone());
        Some(definition)
    }

    fn define_class_impl(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &ClassDeclarationSyntax,
        defer_defaults: bool,
    ) -> ClassDefinitionResult {
        let mut diagnostics = Vec::new();
        let (type_parameters, generics) =
            self.resolve_generic_parameters(module, syntax.type_parameters(), &mut diagnostics);
        let class = ClassId::from_raw(self.next_class);
        self.next_class = self.next_class.saturating_add(1);
        let type_id = self
            .arena
            .intern(SemanticType::Class {
                class,
                arguments: type_parameters
                    .iter()
                    .map(crate::ResolvedTypeParameter::type_id)
                    .collect(),
            })
            .expect("class template arguments are canonical type parameters");
        self.class_types.insert(symbol, type_id);
        self.class_type_parameters
            .insert(symbol, type_parameters.clone());
        let fields =
            self.resolve_class_fields(module, syntax, &generics, defer_defaults, &mut diagnostics);
        let methods = self.resolve_class_methods(module, syntax, &generics, &mut diagnostics);
        let (interfaces, builtin_interfaces) =
            self.resolve_class_interfaces(module, syntax, &generics, &methods, &mut diagnostics);
        diagnostics.sort_by_key(|diagnostic| {
            let span = diagnostic.primary_span();
            (
                span.file(),
                span.range().start(),
                diagnostic.code().as_str(),
            )
        });
        let definition = diagnostics.is_empty().then(|| ClassDefinition {
            symbol,
            source_symbol: symbol,
            module,
            bubble: self
                .database()
                .index()
                .declaration(symbol)
                .map_or(BubbleId::from_raw(u32::MAX), |declaration| {
                    declaration.bubble()
                }),
            class,
            type_id,
            type_parameters,
            is_open: syntax.is_open(),
            interfaces,
            builtin_interfaces,
            fields,
            methods,
            span: syntax.span(),
        });
        if let Some(definition) = &definition {
            self.classes_by_type.insert(type_id, symbol);
            self.class_definitions.insert(symbol, definition.clone());
        } else {
            self.class_types.remove(&symbol);
            self.class_type_parameters.remove(&symbol);
        }
        ClassDefinitionResult {
            definition,
            diagnostics,
        }
    }

    fn resolve_class_fields(
        &mut self,
        module: ModuleId,
        syntax: &ClassDeclarationSyntax,
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        defer_defaults: bool,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<ClassFieldDefinition> {
        let mut fields = Vec::new();
        let mut names = BTreeMap::new();
        for field in syntax.fields() {
            if let Some(original) = names.insert(field.name().to_owned(), field.span()) {
                diagnostics.push(type_diagnostics::duplicate_record_field(
                    field.span(),
                    field.name(),
                    original,
                ));
                continue;
            }
            let Some(resolved) =
                self.resolve_type(module, field.field_type(), generics, diagnostics)
            else {
                continue;
            };
            let Some(field_type) = resolved.type_id() else {
                continue;
            };
            let id = FieldId::from_raw(self.next_field);
            self.next_field = self.next_field.saturating_add(1);
            let (default, pending_default) = match field.default() {
                Some(value) if defer_defaults => (
                    None,
                    Some(PendingConstantExpression::new(value.clone(), field_type)),
                ),
                Some(value) => (
                    resolve_field_default(self.arena(), field_type, value, "class", diagnostics),
                    None,
                ),
                None => (None, None),
            };
            fields.push(ClassFieldDefinition {
                field: id,
                visibility: visibility(field.visibility()),
                name: field.name().to_owned(),
                field_type,
                default,
                pending_default,
                span: field.span(),
            });
        }
        fields
    }

    /// Installs one already-evaluated class field default into a deferred schema.
    ///
    /// # Errors
    ///
    /// Rejects unknown identities, a non-pending target, or a value whose
    /// canonical type is not assignable to the field type.
    pub fn install_class_field_default(
        &mut self,
        definition: SymbolId,
        field: FieldId,
        value: FieldDefault,
    ) -> Result<(), RequiredConstantError> {
        let target = RequiredConstantTarget::ClassField { definition, field };
        let Some(class) = self.class_definitions.get(&definition) else {
            return Err(RequiredConstantError::UnknownTarget(target));
        };
        let Some(index) = class
            .fields
            .iter()
            .position(|candidate| candidate.field == field)
        else {
            return Err(RequiredConstantError::UnknownTarget(target));
        };
        let expected = class.fields[index].field_type;
        if class.fields[index].pending_default.is_none() {
            return Err(RequiredConstantError::NoPendingDefault(target));
        }
        if !field_default_matches_type(self.arena(), &value, expected) {
            return Err(RequiredConstantError::TypeMismatch { target, expected });
        }
        let field = self
            .class_definitions
            .get_mut(&definition)
            .and_then(|class| class.fields.get_mut(index))
            .ok_or(RequiredConstantError::UnknownTarget(target))?;
        field.default = Some(value);
        field.pending_default = None;
        Ok(())
    }

    fn resolve_class_methods(
        &mut self,
        module: ModuleId,
        syntax: &ClassDeclarationSyntax,
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<ClassMethodDefinition> {
        let mut methods = Vec::new();
        let mut names = BTreeMap::new();
        for method in syntax.methods() {
            if method.owner() != syntax.name() {
                diagnostics.push(type_diagnostics::wrong_class_method_owner(
                    method.signature_span(),
                    syntax.name(),
                    method.owner(),
                ));
                continue;
            }
            let key = (method.dispatch(), method.name().to_owned());
            if let Some(original) = names.insert(key, method.signature_span()) {
                diagnostics.push(type_diagnostics::duplicate_record_field(
                    method.signature_span(),
                    method.name(),
                    original,
                ));
                continue;
            }
            let parameters = method
                .parameters()
                .iter()
                .filter_map(|parameter| {
                    let resolved = self.resolve_type(
                        module,
                        parameter.parameter_type(),
                        generics,
                        diagnostics,
                    )?;
                    Some((
                        parameter.name().to_owned(),
                        resolved.type_id()?,
                        parameter.span(),
                    ))
                })
                .collect();
            let results = method
                .results()
                .iter()
                .filter_map(|result| {
                    self.resolve_type(module, result, generics, diagnostics)?
                        .type_id()
                })
                .collect();
            let id = MethodId::from_raw(self.next_method);
            self.next_method = self.next_method.saturating_add(1);
            methods.push(ClassMethodDefinition {
                method: id,
                visibility: visibility(method.visibility()),
                name: method.name().to_owned(),
                dispatch: match method.dispatch() {
                    ClassMethodDispatchSyntax::Static => ClassMethodDispatch::Static,
                    ClassMethodDispatchSyntax::Receiver => ClassMethodDispatch::Receiver,
                },
                parameters,
                results,
                span: method.signature_span(),
            });
        }
        methods
    }
}

const fn visibility(value: VisibilitySyntax) -> Visibility {
    match value {
        VisibilitySyntax::Public => Visibility::Public,
        VisibilitySyntax::Internal => Visibility::Internal,
        VisibilitySyntax::Private => Visibility::Private,
    }
}
