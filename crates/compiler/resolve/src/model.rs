use std::collections::BTreeMap;
use std::fmt::Write;

use pop_foundation::{BubbleId, ModuleId, SourceSpan, SymbolId, SymbolIdentity};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Visibility {
    Public,
    Internal,
    Private,
}

impl Visibility {
    pub(crate) const fn text(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Internal => "internal",
            Self::Private => "private",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum DeclarationKind {
    Function,
    Constant,
    TypeAlias,
    Attribute,
    Record,
    Union,
    Error,
    Class,
    Interface,
    Enum,
    /// Compiler-originated immutable `Codec.Schema<T>` value reserved by an
    /// exact trusted `@RetainMetadata` attachment.
    GeneratedCodecSchema,
}

impl DeclarationKind {
    #[must_use]
    pub const fn symbol_space(self) -> SymbolSpace {
        match self {
            Self::Function | Self::Constant | Self::GeneratedCodecSchema => SymbolSpace::Value,
            Self::TypeAlias
            | Self::Attribute
            | Self::Record
            | Self::Union
            | Self::Error
            | Self::Class
            | Self::Interface
            | Self::Enum => SymbolSpace::Type,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SymbolSpace {
    Type,
    Value,
}

impl SymbolSpace {
    pub(crate) const fn text(self) -> &'static str {
        match self {
            Self::Type => "type",
            Self::Value => "value",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Declaration {
    symbol: SymbolId,
    owner: DeclarationOwner,
    name: String,
    kind: DeclarationKind,
    visibility: Visibility,
    span: SourceSpan,
    reference_identity: Option<SymbolIdentity>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeclarationOwner {
    module: ModuleId,
    bubble: BubbleId,
    namespace: String,
}

impl DeclarationOwner {
    #[must_use]
    pub fn new(module: ModuleId, bubble: BubbleId, namespace: impl Into<String>) -> Self {
        Self {
            module,
            bubble,
            namespace: namespace.into(),
        }
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
    pub fn namespace(&self) -> &str {
        &self.namespace
    }
}

impl Declaration {
    #[must_use]
    pub fn new(
        symbol: SymbolId,
        module: ModuleId,
        bubble: BubbleId,
        name: impl Into<String>,
        kind: DeclarationKind,
        visibility: Visibility,
        span: SourceSpan,
    ) -> Self {
        Self::new_in_namespace(
            symbol,
            DeclarationOwner::new(module, bubble, String::new()),
            name,
            kind,
            visibility,
            span,
        )
    }

    #[must_use]
    pub fn new_in_namespace(
        symbol: SymbolId,
        owner: DeclarationOwner,
        name: impl Into<String>,
        kind: DeclarationKind,
        visibility: Visibility,
        span: SourceSpan,
    ) -> Self {
        Self {
            symbol,
            owner,
            name: name.into(),
            kind,
            visibility,
            span,
            reference_identity: None,
        }
    }

    #[must_use]
    pub fn referenced(
        symbol: SymbolId,
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        kind: DeclarationKind,
        span: SourceSpan,
    ) -> Self {
        Self {
            symbol,
            owner: DeclarationOwner::new(module, identity.bubble(), namespace),
            name: name.into(),
            kind,
            visibility: Visibility::Public,
            span,
            reference_identity: Some(identity),
        }
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn module(&self) -> ModuleId {
        self.owner.module
    }

    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.owner.bubble
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.owner.namespace
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn qualified_name(&self) -> String {
        if self.owner.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.owner.namespace, self.name)
        }
    }

    #[must_use]
    pub const fn kind(&self) -> DeclarationKind {
        self.kind
    }

    #[must_use]
    pub const fn symbol_space(&self) -> SymbolSpace {
        self.kind.symbol_space()
    }

    #[must_use]
    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }

    #[must_use]
    pub const fn reference_identity(&self) -> Option<SymbolIdentity> {
        self.reference_identity
    }

    #[must_use]
    pub fn is_accessible_from(&self, module: ModuleId, bubble: BubbleId) -> bool {
        match self.visibility {
            Visibility::Public => true,
            Visibility::Internal => self.owner.bubble == bubble,
            Visibility::Private => self.owner.module == module,
        }
    }

    #[must_use]
    pub const fn is_in_public_reference_surface(&self) -> bool {
        matches!(self.visibility, Visibility::Public)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UsingDirective {
    namespace: String,
    alias: Option<String>,
    span: SourceSpan,
}

impl UsingDirective {
    #[must_use]
    pub fn new(namespace: impl Into<String>, alias: Option<String>, span: SourceSpan) -> Self {
        Self {
            namespace: namespace.into(),
            alias,
            span,
        }
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    #[must_use]
    pub fn alias(&self) -> Option<&str> {
        self.alias.as_deref()
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ModuleIndex {
    module: ModuleId,
    bubble: BubbleId,
    namespace: String,
    usings: Vec<UsingDirective>,
    declarations: Vec<SymbolId>,
}

impl ModuleIndex {
    pub(crate) fn new(
        module: ModuleId,
        bubble: BubbleId,
        namespace: String,
        usings: Vec<UsingDirective>,
        declarations: Vec<SymbolId>,
    ) -> Self {
        Self {
            module,
            bubble,
            namespace,
            usings,
            declarations,
        }
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
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    #[must_use]
    pub fn usings(&self) -> &[UsingDirective] {
        &self.usings
    }

    #[must_use]
    pub fn declarations(&self) -> &[SymbolId] {
        &self.declarations
    }
}

#[derive(Clone, Debug, Default)]
pub struct DeclarationIndex {
    modules: BTreeMap<ModuleId, ModuleIndex>,
    declarations: BTreeMap<SymbolId, Declaration>,
}

impl DeclarationIndex {
    pub(crate) fn new(
        modules: BTreeMap<ModuleId, ModuleIndex>,
        declarations: BTreeMap<SymbolId, Declaration>,
    ) -> Self {
        Self {
            modules,
            declarations,
        }
    }

    #[must_use]
    pub fn module(&self, module: ModuleId) -> Option<&ModuleIndex> {
        self.modules.get(&module)
    }

    #[must_use]
    pub fn declaration(&self, symbol: SymbolId) -> Option<&Declaration> {
        self.declarations.get(&symbol)
    }

    #[must_use]
    pub fn declaration_by_reference_identity(
        &self,
        identity: SymbolIdentity,
    ) -> Option<&Declaration> {
        self.declarations
            .values()
            .find(|declaration| declaration.reference_identity() == Some(identity))
    }

    pub fn declarations(&self) -> impl Iterator<Item = &Declaration> {
        self.declarations.values()
    }

    /// Adds verified public dependency declarations with session-local symbol
    /// remapping while retaining their original Bubble-scoped identities.
    ///
    /// # Errors
    ///
    /// Rejects duplicate reference identities or exhaustion of the local
    /// symbol arena.
    pub fn with_referenced_declarations(
        mut self,
        references: impl IntoIterator<Item = ReferencedDeclaration>,
    ) -> Result<Self, ReferenceIndexError> {
        let mut identities: BTreeMap<_, _> = self
            .declarations
            .values()
            .filter_map(|declaration| {
                declaration
                    .reference_identity()
                    .map(|identity| (identity, declaration.symbol()))
            })
            .collect();
        let mut next = self
            .declarations
            .keys()
            .next_back()
            .map_or(0, |symbol| symbol.raw().saturating_add(1));
        for reference in references {
            if identities.contains_key(&reference.identity) {
                return Err(ReferenceIndexError::DuplicateIdentity(reference.identity));
            }
            let symbol = SymbolId::from_raw(next);
            next = next
                .checked_add(if reference.kind == DeclarationKind::GeneratedCodecSchema {
                    3
                } else {
                    1
                })
                .ok_or(ReferenceIndexError::SymbolArenaExhausted)?;
            identities.insert(reference.identity, symbol);
            self.declarations.insert(
                symbol,
                Declaration::referenced(
                    symbol,
                    reference.identity,
                    reference.module,
                    reference.namespace,
                    reference.name,
                    reference.kind,
                    reference.span,
                ),
            );
        }
        Ok(self)
    }

    /// Reserves compiler-originated same-Module codec-schema Items after the
    /// trusted attachment identity has been resolved.
    ///
    /// # Errors
    ///
    /// Rejects a duplicate source name, missing target Module, or exhausted
    /// local symbol identity space.
    pub fn with_generated_codec_schemas(
        mut self,
        mut schemas: Vec<GeneratedCodecSchemaDeclaration>,
    ) -> Result<Self, GeneratedDeclarationError> {
        schemas.sort_by_key(|schema| schema.target);
        let mut next = self
            .declarations
            .keys()
            .next_back()
            .map_or(0, |symbol| symbol.raw().saturating_add(1));
        for schema in schemas {
            let module = self
                .modules
                .get_mut(&schema.module)
                .ok_or(GeneratedDeclarationError::MissingModule)?;
            if self.declarations.values().any(|declaration| {
                declaration.owner.namespace == schema.namespace && declaration.name == schema.name
            }) {
                return Err(GeneratedDeclarationError::DuplicateName);
            }
            let symbol = SymbolId::from_raw(next);
            next = next
                // The two following local identities are private lowering
                // slots for the sealed encode/decode payload entries. They
                // are not namespace declarations or public Items.
                .checked_add(3)
                .ok_or(GeneratedDeclarationError::SymbolArenaExhausted)?;
            self.declarations.insert(
                symbol,
                Declaration::new_in_namespace(
                    symbol,
                    DeclarationOwner::new(schema.module, schema.bubble, schema.namespace),
                    schema.name,
                    DeclarationKind::GeneratedCodecSchema,
                    schema.visibility,
                    schema.span,
                ),
            );
            module.declarations.push(symbol);
            module.declarations.sort_unstable();
        }
        Ok(self)
    }

    #[must_use]
    pub fn declaration_by_qualified_name(
        &self,
        qualified_name: &str,
        space: SymbolSpace,
    ) -> Vec<&Declaration> {
        self.declarations
            .values()
            .filter(|declaration| {
                declaration.symbol_space() == space
                    && declaration.qualified_name() == qualified_name
            })
            .collect()
    }

    #[must_use]
    pub fn dump(&self) -> String {
        let mut dump = String::new();
        for module in self.modules.values() {
            let _ = writeln!(
                dump,
                "module {} bubble {} namespace {}",
                module.module.raw(),
                module.bubble.raw(),
                module.namespace
            );
            for using in &module.usings {
                if let Some(alias) = &using.alias {
                    let _ = writeln!(dump, "using {alias} = {}", using.namespace);
                } else {
                    let _ = writeln!(dump, "using {}", using.namespace);
                }
            }
            for symbol in &module.declarations {
                let Some(declaration) = self.declarations.get(symbol) else {
                    continue;
                };
                let _ = writeln!(
                    dump,
                    "symbol {} {} {} {:?} {}",
                    declaration.symbol.raw(),
                    declaration.visibility.text(),
                    declaration.symbol_space().text(),
                    declaration.kind,
                    declaration.qualified_name()
                );
            }
        }
        dump
    }

    pub(crate) fn lookup(
        &self,
        namespace: &str,
        name: &str,
        space: SymbolSpace,
    ) -> Vec<&Declaration> {
        self.declarations
            .values()
            .filter(|declaration| {
                declaration.owner.namespace == namespace
                    && declaration.name == name
                    && declaration.symbol_space() == space
            })
            .collect()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedCodecSchemaDeclaration {
    target: SymbolId,
    module: ModuleId,
    bubble: BubbleId,
    namespace: String,
    name: String,
    visibility: Visibility,
    span: SourceSpan,
}

impl GeneratedCodecSchemaDeclaration {
    #[must_use]
    pub fn new(
        target: SymbolId,
        module: ModuleId,
        bubble: BubbleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        visibility: Visibility,
        span: SourceSpan,
    ) -> Self {
        Self {
            target,
            module,
            bubble,
            namespace: namespace.into(),
            name: name.into(),
            visibility,
            span,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GeneratedDeclarationError {
    MissingModule,
    DuplicateName,
    SymbolArenaExhausted,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferencedDeclaration {
    identity: SymbolIdentity,
    module: ModuleId,
    namespace: String,
    name: String,
    kind: DeclarationKind,
    span: SourceSpan,
}

impl ReferencedDeclaration {
    #[must_use]
    pub const fn identity(&self) -> SymbolIdentity {
        self.identity
    }

    #[must_use]
    pub fn generated_codec_schema(
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            identity,
            module,
            namespace: namespace.into(),
            name: name.into(),
            kind: DeclarationKind::GeneratedCodecSchema,
            span,
        }
    }
    #[must_use]
    pub fn class(
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            identity,
            module,
            namespace: namespace.into(),
            name: name.into(),
            kind: DeclarationKind::Class,
            span,
        }
    }

    #[must_use]
    pub fn interface(
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            identity,
            module,
            namespace: namespace.into(),
            name: name.into(),
            kind: DeclarationKind::Interface,
            span,
        }
    }

    #[must_use]
    pub fn function(
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            identity,
            module,
            namespace: namespace.into(),
            name: name.into(),
            kind: DeclarationKind::Function,
            span,
        }
    }

    #[must_use]
    pub fn record(
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            identity,
            module,
            namespace: namespace.into(),
            name: name.into(),
            kind: DeclarationKind::Record,
            span,
        }
    }

    #[must_use]
    pub fn enumeration(
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            identity,
            module,
            namespace: namespace.into(),
            name: name.into(),
            kind: DeclarationKind::Enum,
            span,
        }
    }

    #[must_use]
    pub fn union(
        identity: SymbolIdentity,
        module: ModuleId,
        namespace: impl Into<String>,
        name: impl Into<String>,
        span: SourceSpan,
    ) -> Self {
        Self {
            identity,
            module,
            namespace: namespace.into(),
            name: name.into(),
            kind: DeclarationKind::Union,
            span,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceIndexError {
    DuplicateIdentity(SymbolIdentity),
    SymbolArenaExhausted,
}
