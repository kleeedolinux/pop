use std::collections::BTreeMap;

use pop_diagnostics::types as type_diagnostics;
use pop_foundation::{
    BubbleId, BuiltinTypeId, ClassId, Diagnostic, InterfaceId, InterfaceMethodId,
    IterationProtocolMethodId, MethodId, ModuleId, ParameterId, SourceSpan, SymbolId, TypeId,
};
use pop_resolve::Visibility;
use pop_syntax::{ClassDeclarationSyntax, InterfaceDeclarationSyntax, TypeSyntax, TypeSyntaxKind};

use crate::{
    ClassMethodDefinition, ClassMethodDispatch, ResolvedTypeKind, ResolvedTypeParameter,
    SemanticType, SignatureResolver,
};

/// One source-declared nominal interface and its canonical dispatch surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceDefinition {
    symbol: SymbolId,
    module: ModuleId,
    bubble: BubbleId,
    interface: InterfaceId,
    type_id: TypeId,
    type_parameters: Vec<ResolvedTypeParameter>,
    methods: Vec<InterfaceMethodDefinition>,
    span: SourceSpan,
}

impl InterfaceDefinition {
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
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn type_parameters(&self) -> &[ResolvedTypeParameter] {
        &self.type_parameters
    }

    #[must_use]
    pub fn methods(&self) -> &[InterfaceMethodDefinition] {
        &self.methods
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

/// One exact public instance signature in an interface dispatch surface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InterfaceMethodDefinition {
    method: InterfaceMethodId,
    slot: u32,
    name: String,
    parameters: Vec<(String, TypeId, SourceSpan)>,
    results: Vec<TypeId>,
    span: SourceSpan,
}

impl InterfaceMethodDefinition {
    #[must_use]
    pub const fn method(&self) -> InterfaceMethodId {
        self.method
    }

    /// Returns the deterministic ordinal inside this interface's canonical
    /// member-identity order.
    #[must_use]
    pub const fn slot(&self) -> u32 {
        self.slot
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
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug)]
pub struct InterfaceDefinitionResult {
    definition: Option<InterfaceDefinition>,
    diagnostics: Vec<Diagnostic>,
}

impl InterfaceDefinitionResult {
    #[must_use]
    pub const fn definition(&self) -> Option<&InterfaceDefinition> {
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

/// The verified slot map for one interface explicitly named by a class.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassInterfaceImplementation {
    interface: InterfaceId,
    interface_type: TypeId,
    methods: Vec<InterfaceMethodImplementation>,
}

impl ClassInterfaceImplementation {
    pub(crate) const fn referenced(interface: InterfaceId, interface_type: TypeId) -> Self {
        Self {
            interface,
            interface_type,
            methods: Vec::new(),
        }
    }

    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn interface_type(&self) -> TypeId {
        self.interface_type
    }

    #[must_use]
    pub fn methods(&self) -> &[InterfaceMethodImplementation] {
        &self.methods
    }

    pub(crate) fn specialize(
        &self,
        interface: &InterfaceDefinition,
        method_ids: &BTreeMap<MethodId, MethodId>,
    ) -> Option<Self> {
        Some(Self {
            interface: interface.interface(),
            interface_type: interface.type_id(),
            methods: self
                .methods
                .iter()
                .map(|method| {
                    let specialized = interface
                        .methods()
                        .iter()
                        .find(|candidate| candidate.slot() == method.slot)?;
                    Some(InterfaceMethodImplementation {
                        interface_method: specialized.method(),
                        slot: method.slot,
                        class_method: *method_ids.get(&method.class_method)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        })
    }
}

/// One exact implementation of a reserved built-in nominal interface.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassBuiltinInterfaceImplementation {
    interface: BuiltinTypeId,
    interface_type: TypeId,
    methods: Vec<BuiltinInterfaceMethodImplementation>,
}

impl ClassBuiltinInterfaceImplementation {
    #[must_use]
    pub const fn interface(&self) -> BuiltinTypeId {
        self.interface
    }

    #[must_use]
    pub const fn interface_type(&self) -> TypeId {
        self.interface_type
    }

    #[must_use]
    pub fn methods(&self) -> &[BuiltinInterfaceMethodImplementation] {
        &self.methods
    }

    pub(crate) fn specialize(
        &self,
        interface_type: TypeId,
        method_ids: &BTreeMap<MethodId, MethodId>,
    ) -> Option<Self> {
        Some(Self {
            interface: self.interface,
            interface_type,
            methods: self
                .methods
                .iter()
                .map(|method| {
                    Some(BuiltinInterfaceMethodImplementation {
                        protocol_method: method.protocol_method,
                        class_method: *method_ids.get(&method.class_method)?,
                    })
                })
                .collect::<Option<Vec<_>>>()?,
        })
    }
}

/// A verified reserved protocol slot mapped to one native class method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BuiltinInterfaceMethodImplementation {
    protocol_method: IterationProtocolMethodId,
    class_method: MethodId,
}

impl BuiltinInterfaceMethodImplementation {
    #[must_use]
    pub const fn protocol_method(&self) -> IterationProtocolMethodId {
        self.protocol_method
    }

    #[must_use]
    pub const fn class_method(&self) -> MethodId {
        self.class_method
    }
}

/// A statically verified mapping from an interface slot to a receiver method.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InterfaceMethodImplementation {
    interface_method: InterfaceMethodId,
    slot: u32,
    class_method: MethodId,
}

impl InterfaceMethodImplementation {
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

impl SignatureResolver<'_> {
    #[must_use]
    pub fn interface_definition(&self, symbol: SymbolId) -> Option<&InterfaceDefinition> {
        self.interface_definitions.get(&symbol)
    }

    #[must_use]
    pub fn interface_definition_for_type(&self, type_id: TypeId) -> Option<&InterfaceDefinition> {
        self.interfaces_by_type
            .get(&type_id)
            .and_then(|symbol| self.interface_definitions.get(symbol))
    }

    /// Defines one nominal interface and resolves every method signature to
    /// canonical static types before any class can implement it.
    ///
    /// # Panics
    ///
    /// Panics only if the type arena rejects a fresh interface identity with
    /// no referenced types, which is an internal compiler invariant violation.
    #[must_use]
    pub fn define_interface(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &InterfaceDeclarationSyntax,
    ) -> InterfaceDefinitionResult {
        let mut diagnostics = Vec::new();
        let (type_parameters, generics) =
            self.resolve_generic_parameters(module, syntax.type_parameters(), &mut diagnostics);
        let interface = InterfaceId::from_raw(self.next_interface);
        self.next_interface = self.next_interface.saturating_add(1);
        let type_id = self
            .arena
            .intern(SemanticType::Interface {
                interface,
                arguments: type_parameters
                    .iter()
                    .map(ResolvedTypeParameter::type_id)
                    .collect(),
            })
            .expect("interface identity has verified type dependencies");
        self.interface_types.insert(symbol, type_id);
        self.interface_sources_by_id.insert(interface, symbol);

        if syntax.methods().is_empty() {
            diagnostics.push(type_diagnostics::empty_interface(
                syntax.span(),
                syntax.name(),
            ));
        }
        let methods = self.resolve_interface_methods(module, syntax, &generics, &mut diagnostics);
        diagnostics.sort_by_key(|diagnostic| {
            let span = diagnostic.primary_span();
            (
                span.file(),
                span.range().start(),
                diagnostic.code().as_str(),
            )
        });
        let definition = diagnostics.is_empty().then(|| InterfaceDefinition {
            symbol,
            module,
            bubble: self
                .database()
                .index()
                .declaration(symbol)
                .map_or(BubbleId::from_raw(u32::MAX), |declaration| {
                    declaration.bubble()
                }),
            interface,
            type_id,
            type_parameters: type_parameters.clone(),
            methods,
            span: syntax.span(),
        });
        if let Some(definition) = &definition {
            self.interface_type_parameters
                .insert(symbol, type_parameters);
            self.interfaces_by_type.insert(type_id, symbol);
            self.interface_definitions
                .insert(symbol, definition.clone());
        } else {
            self.interface_types.remove(&symbol);
            self.interface_sources_by_id.remove(&interface);
        }
        InterfaceDefinitionResult {
            definition,
            diagnostics,
        }
    }

    /// Reconstructs one artifact-verified public interface declaration in an
    /// isolated consumer arena. Member lookup is deliberately absent: this
    /// closed projection exists only to retain nominal type and cast-witness
    /// identity, never runtime reflection.
    #[must_use]
    pub fn define_referenced_interface(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        type_parameters: Vec<ResolvedTypeParameter>,
        span: SourceSpan,
    ) -> Option<InterfaceDefinition> {
        if self.interface_definitions.contains_key(&symbol) {
            return None;
        }
        let interface = InterfaceId::from_raw(self.next_interface);
        self.next_interface = self.next_interface.saturating_add(1);
        let arguments = type_parameters
            .iter()
            .map(ResolvedTypeParameter::type_id)
            .collect();
        let type_id = self
            .arena
            .intern(SemanticType::Interface {
                interface,
                arguments,
            })
            .ok()?;
        let bubble = self.database().index().declaration(symbol)?.bubble();
        let definition = InterfaceDefinition {
            symbol,
            module,
            bubble,
            interface,
            type_id,
            type_parameters: type_parameters.clone(),
            methods: Vec::new(),
            span,
        };
        self.interface_types.insert(symbol, type_id);
        self.interface_type_parameters
            .insert(symbol, type_parameters);
        self.interface_sources_by_id.insert(interface, symbol);
        self.interfaces_by_type.insert(type_id, symbol);
        self.interface_definitions
            .insert(symbol, definition.clone());
        Some(definition)
    }

    fn resolve_interface_methods(
        &mut self,
        module: ModuleId,
        syntax: &InterfaceDeclarationSyntax,
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Vec<InterfaceMethodDefinition> {
        let mut methods = Vec::new();
        let mut names = BTreeMap::new();
        for method in syntax.methods() {
            if let Some(original) = names.insert(method.name().to_owned(), method.span()) {
                diagnostics.push(type_diagnostics::duplicate_interface_method(
                    method.span(),
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
            let id = InterfaceMethodId::from_raw(self.next_interface_method);
            self.next_interface_method = self.next_interface_method.saturating_add(1);
            methods.push(InterfaceMethodDefinition {
                method: id,
                slot: 0,
                name: method.name().to_owned(),
                parameters,
                results,
                span: method.span(),
            });
        }
        methods.sort_by_key(InterfaceMethodDefinition::method);
        for (slot, method) in methods.iter_mut().enumerate() {
            method.slot = u32::try_from(slot).unwrap_or(u32::MAX);
        }
        methods
    }

    pub fn instantiate_interface(
        &mut self,
        definition: SymbolId,
        arguments: &[TypeId],
    ) -> Option<InterfaceDefinition> {
        let key = (definition, arguments.to_vec());
        if let Some(symbol) = self.interface_instances.get(&key) {
            return self.interface_definitions.get(symbol).cloned();
        }
        let parameters = self.interface_type_parameters.get(&definition)?.clone();
        if parameters.len() != arguments.len() {
            return None;
        }
        if parameters
            .iter()
            .map(ResolvedTypeParameter::type_id)
            .eq(arguments.iter().copied())
        {
            return self.interface_definitions.get(&definition).cloned();
        }
        let substitutions = parameters
            .iter()
            .zip(arguments)
            .map(|(parameter, argument)| (parameter.parameter(), *argument))
            .collect();
        let template = self.interface_definitions.get(&definition)?.clone();
        let symbol = SymbolId::from_raw(self.next_instance_symbol);
        self.next_instance_symbol = self.next_instance_symbol.saturating_add(1);
        let interface = InterfaceId::from_raw(self.next_interface);
        self.next_interface = self.next_interface.saturating_add(1);
        let type_id = self
            .arena
            .intern(SemanticType::Interface {
                interface,
                arguments: arguments.to_vec(),
            })
            .ok()?;
        let mut methods = template.methods.clone();
        for method in &mut methods {
            method.method = InterfaceMethodId::from_raw(self.next_interface_method);
            self.next_interface_method = self.next_interface_method.saturating_add(1);
            for (_, parameter_type, _) in &mut method.parameters {
                *parameter_type =
                    self.substitute_type_parameters(*parameter_type, &substitutions)?;
            }
            for result in &mut method.results {
                *result = self.substitute_type_parameters(*result, &substitutions)?;
            }
        }
        let instance = InterfaceDefinition {
            symbol,
            module: template.module,
            bubble: template.bubble,
            interface,
            type_id,
            type_parameters: Vec::new(),
            methods,
            span: template.span,
        };
        self.interface_instances.insert(key, symbol);
        self.interface_instance_sources.insert(symbol, definition);
        self.interface_sources_by_id.insert(interface, definition);
        self.interfaces_by_type.insert(type_id, symbol);
        self.interface_definitions.insert(symbol, instance.clone());
        Some(instance)
    }

    pub(crate) fn validate_interface_arguments(
        &mut self,
        definition: SymbolId,
        arguments: &[TypeId],
        span: SourceSpan,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> bool {
        let Some(parameters) = self.interface_type_parameters.get(&definition).cloned() else {
            return arguments.is_empty();
        };
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

    pub fn interface_instances(
        &self,
        definition: SymbolId,
    ) -> impl Iterator<Item = &InterfaceDefinition> {
        self.interface_instances
            .iter()
            .filter(move |((source, _), _)| *source == definition)
            .filter_map(|(_, symbol)| self.interface_definitions.get(symbol))
    }

    #[must_use]
    pub fn interface_is_generic(&self, definition: SymbolId) -> bool {
        self.interface_type_parameters
            .get(&definition)
            .is_some_and(|parameters| !parameters.is_empty())
    }

    #[must_use]
    pub fn interface_source_identity(&self, interface: InterfaceId) -> Option<SymbolId> {
        self.interface_sources_by_id.get(&interface).copied()
    }

    pub(crate) fn resolve_class_interfaces(
        &mut self,
        module: ModuleId,
        syntax: &ClassDeclarationSyntax,
        generics: &BTreeMap<String, (ParameterId, TypeId)>,
        class_methods: &[ClassMethodDefinition],
        diagnostics: &mut Vec<Diagnostic>,
    ) -> (
        Vec<ClassInterfaceImplementation>,
        Vec<ClassBuiltinInterfaceImplementation>,
    ) {
        let mut resolved_interfaces = Vec::new();
        let mut resolved_builtin_interfaces = Vec::new();
        let mut seen = BTreeMap::new();
        let mut seen_builtin = BTreeMap::new();
        for implemented in syntax.interfaces() {
            let Some(resolved) = self.resolve_type(module, implemented, generics, diagnostics)
            else {
                continue;
            };
            if let ResolvedTypeKind::Builtin {
                definition,
                arguments,
            } = resolved.kind()
            {
                let Some(interface_type) = resolved.type_id() else {
                    continue;
                };
                let Some(implementation) = self.resolve_builtin_class_interface(
                    syntax,
                    implemented,
                    *definition,
                    arguments,
                    interface_type,
                    class_methods,
                    &mut seen_builtin,
                    diagnostics,
                ) else {
                    continue;
                };
                resolved_builtin_interfaces.push(implementation);
                continue;
            }
            let ResolvedTypeKind::Declaration { symbol, .. } = resolved.kind() else {
                diagnostics.push(type_diagnostics::invalid_interface_implementation(
                    implemented.span(),
                    source_type_name(implemented),
                ));
                continue;
            };
            let Some(interface) = self.interface_definitions.get(symbol).cloned() else {
                diagnostics.push(type_diagnostics::invalid_interface_implementation(
                    implemented.span(),
                    source_type_name(implemented),
                ));
                continue;
            };
            if seen
                .insert(interface.interface(), implemented.span())
                .is_some()
            {
                diagnostics.push(type_diagnostics::invalid_interface_implementation(
                    implemented.span(),
                    source_type_name(implemented),
                ));
                continue;
            }

            let mut mappings = Vec::new();
            for required in interface.methods() {
                let candidates: Vec<_> = class_methods
                    .iter()
                    .filter(|method| method.name() == required.name())
                    .collect();
                if let Some(method) = candidates.iter().copied().find(|method| {
                    method.dispatch() == ClassMethodDispatch::Receiver
                        && method.visibility() == Visibility::Public
                        && signatures_match(required, method)
                }) {
                    mappings.push(InterfaceMethodImplementation {
                        interface_method: required.method(),
                        slot: required.slot(),
                        class_method: method.method(),
                    });
                    continue;
                }
                if candidates.is_empty() {
                    diagnostics.push(type_diagnostics::missing_interface_method(
                        syntax.span(),
                        syntax.name(),
                        interface_name(self, &interface),
                        required.name(),
                    ));
                    continue;
                }
                let reason = incompatible_reason(required, &candidates);
                diagnostics.push(type_diagnostics::incompatible_interface_method(
                    candidates[0].span(),
                    syntax.name(),
                    interface_name(self, &interface),
                    required.name(),
                    reason,
                ));
            }
            mappings.sort_by_key(InterfaceMethodImplementation::interface_method);
            resolved_interfaces.push(ClassInterfaceImplementation {
                interface: interface.interface(),
                interface_type: interface.type_id(),
                methods: mappings,
            });
        }
        resolved_interfaces.sort_by_key(ClassInterfaceImplementation::interface);
        resolved_builtin_interfaces.sort_by_key(ClassBuiltinInterfaceImplementation::interface);
        (resolved_interfaces, resolved_builtin_interfaces)
    }

    #[allow(clippy::too_many_arguments)]
    fn resolve_builtin_class_interface(
        &mut self,
        syntax: &ClassDeclarationSyntax,
        implemented: &TypeSyntax,
        definition: BuiltinTypeId,
        arguments: &[crate::ResolvedType],
        interface_type: TypeId,
        class_methods: &[ClassMethodDefinition],
        seen: &mut BTreeMap<BuiltinTypeId, SourceSpan>,
        diagnostics: &mut Vec<Diagnostic>,
    ) -> Option<ClassBuiltinInterfaceImplementation> {
        let protocol = self.schema().iteration_protocol()?;
        if arguments.len() != 1
            || (definition != protocol.iterable() && definition != protocol.iterator())
        {
            diagnostics.push(type_diagnostics::invalid_interface_implementation(
                implemented.span(),
                source_type_name(implemented),
            ));
            return None;
        }
        if seen.insert(definition, implemented.span()).is_some() {
            diagnostics.push(type_diagnostics::invalid_interface_implementation(
                implemented.span(),
                source_type_name(implemented),
            ));
            return None;
        }
        let item_type = arguments[0].type_id()?;
        let iterator_type = self
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: protocol.iterator(),
                arguments: vec![item_type],
            })
            .ok()?;
        let iteration_type = self
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: protocol.iteration(),
                arguments: vec![item_type],
            })
            .ok()?;
        let mut requirements = vec![(protocol.iterator_method(), "iterator", iterator_type)];
        if definition == protocol.iterator() {
            requirements.push((protocol.next_method(), "next", iteration_type));
        }
        let interface_name = if definition == protocol.iterator() {
            "Iterator"
        } else {
            "Iterable"
        };
        let mut mappings = Vec::new();
        for (protocol_method, name, result_type) in requirements {
            let candidates: Vec<_> = class_methods
                .iter()
                .filter(|method| method.name() == name)
                .collect();
            if let Some(method) = candidates.iter().copied().find(|method| {
                method.dispatch() == ClassMethodDispatch::Receiver
                    && method.visibility() == Visibility::Public
                    && method.parameters().is_empty()
                    && method.results() == [result_type]
            }) {
                mappings.push(BuiltinInterfaceMethodImplementation {
                    protocol_method,
                    class_method: method.method(),
                });
                continue;
            }
            if candidates.is_empty() {
                diagnostics.push(type_diagnostics::missing_interface_method(
                    syntax.span(),
                    syntax.name(),
                    interface_name,
                    name,
                ));
            } else {
                let reason = if candidates.iter().any(|method| {
                    method.dispatch() == ClassMethodDispatch::Receiver
                        && method.visibility() != Visibility::Public
                        && method.parameters().is_empty()
                        && method.results() == [result_type]
                }) {
                    "InaccessibleReceiverMethod"
                } else if candidates
                    .iter()
                    .all(|method| method.dispatch() == ClassMethodDispatch::Static)
                {
                    "StaticMethod"
                } else {
                    "SignatureMismatch"
                };
                diagnostics.push(type_diagnostics::incompatible_interface_method(
                    candidates[0].span(),
                    syntax.name(),
                    interface_name,
                    name,
                    reason,
                ));
            }
        }
        mappings.sort_by_key(BuiltinInterfaceMethodImplementation::protocol_method);
        Some(ClassBuiltinInterfaceImplementation {
            interface: definition,
            interface_type,
            methods: mappings,
        })
    }

    #[must_use]
    pub fn class_implements_interface(&self, class: ClassId, interface: InterfaceId) -> bool {
        self.class_definitions
            .values()
            .find(|definition| definition.class() == class)
            .is_some_and(|definition| {
                definition
                    .interfaces()
                    .iter()
                    .any(|implementation| implementation.interface() == interface)
            })
    }

    #[must_use]
    pub fn is_class_to_interface_upcast(&self, source: TypeId, target: TypeId) -> bool {
        let Some(SemanticType::Class { class, .. }) = self.arena().get(source) else {
            return false;
        };
        let Some(SemanticType::Interface { interface, .. }) = self.arena().get(target) else {
            return false;
        };
        self.class_implements_interface(*class, *interface)
    }
}

fn signatures_match(interface: &InterfaceMethodDefinition, class: &ClassMethodDefinition) -> bool {
    interface
        .parameters()
        .iter()
        .map(|(_, parameter_type, _)| parameter_type)
        .eq(class
            .parameters()
            .iter()
            .map(|(_, parameter_type, _)| parameter_type))
        && interface.results() == class.results()
}

fn incompatible_reason(
    required: &InterfaceMethodDefinition,
    candidates: &[&ClassMethodDefinition],
) -> &'static str {
    if candidates.iter().any(|method| {
        method.dispatch() == ClassMethodDispatch::Receiver
            && method.visibility() != Visibility::Public
            && signatures_match(required, method)
    }) {
        "InaccessibleReceiverMethod"
    } else if candidates
        .iter()
        .all(|method| method.dispatch() == ClassMethodDispatch::Static)
    {
        "StaticMethod"
    } else {
        "SignatureMismatch"
    }
}

fn interface_name(resolver: &SignatureResolver<'_>, interface: &InterfaceDefinition) -> String {
    resolver
        .database()
        .index()
        .declaration(interface.symbol())
        .map_or_else(
            || format!("Interface{}", interface.interface().raw()),
            pop_resolve::Declaration::qualified_name,
        )
}

fn source_type_name(syntax: &TypeSyntax) -> String {
    match syntax.kind() {
        TypeSyntaxKind::Named { path, .. } => path.join("."),
        _ => "non-interface type".to_owned(),
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
