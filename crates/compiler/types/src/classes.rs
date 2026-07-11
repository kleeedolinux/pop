use std::collections::BTreeMap;

use pop_diagnostics::types as type_diagnostics;
use pop_foundation::{
    BubbleId, ClassId, Diagnostic, FieldId, MethodId, ModuleId, SourceSpan, SymbolId, TypeId,
};
use pop_resolve::Visibility;
use pop_syntax::{ClassDeclarationSyntax, ClassMethodDispatchSyntax, VisibilitySyntax};

use crate::field_defaults::resolve_field_default;
use crate::required_constants::field_default_matches_type;
use crate::{
    ClassInterfaceImplementation, FieldDefault, PendingConstantExpression, RequiredConstantError,
    RequiredConstantTarget, ResolvedFunctionSignature, SemanticType, SignatureResolver,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassMethodDispatch {
    Static,
    Receiver,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassDefinition {
    symbol: SymbolId,
    module: ModuleId,
    bubble: BubbleId,
    class: ClassId,
    type_id: TypeId,
    is_open: bool,
    interfaces: Vec<ClassInterfaceImplementation>,
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
    pub const fn is_open(&self) -> bool {
        self.is_open
    }

    #[must_use]
    pub fn interfaces(&self) -> &[ClassInterfaceImplementation] {
        &self.interfaces
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
        ResolvedFunctionSignature::canonical(
            definition.symbol(),
            format!("{}{separator}{}", definition.class().raw(), method.name()),
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

    fn define_class_impl(
        &mut self,
        module: ModuleId,
        symbol: SymbolId,
        syntax: &ClassDeclarationSyntax,
        defer_defaults: bool,
    ) -> ClassDefinitionResult {
        let class = ClassId::from_raw(self.next_class);
        self.next_class = self.next_class.saturating_add(1);
        let type_id = self
            .arena
            .intern(SemanticType::Class {
                class,
                arguments: Vec::new(),
            })
            .expect("class identity has no type dependencies");
        self.class_types.insert(symbol, type_id);
        let mut diagnostics = Vec::new();
        let fields = self.resolve_class_fields(module, syntax, defer_defaults, &mut diagnostics);
        let methods = self.resolve_class_methods(module, syntax, &mut diagnostics);
        let interfaces = self.resolve_class_interfaces(module, syntax, &methods, &mut diagnostics);
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
            is_open: syntax.is_open(),
            interfaces,
            fields,
            methods,
            span: syntax.span(),
        });
        if let Some(definition) = &definition {
            self.classes_by_type.insert(type_id, symbol);
            self.class_definitions.insert(symbol, definition.clone());
        } else {
            self.class_types.remove(&symbol);
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
                self.resolve_type(module, field.field_type(), &BTreeMap::new(), diagnostics)
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
                        &BTreeMap::new(),
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
                    self.resolve_type(module, result, &BTreeMap::new(), diagnostics)?
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
