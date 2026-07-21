//! Canonical backend-neutral MIR implementation.
#![allow(
    clippy::match_same_arms,
    clippy::needless_pass_by_value,
    clippy::too_many_lines
)]

use std::fmt::{self, Write};

use pop_foundation::{
    AllocationSiteId, BindingId, BlockId, BorrowRegionId, BubbleId, BuiltinTypeId, CaptureId,
    ClassId, CleanupScopeId, CoroutineStateId, EnumCaseId, ErrorCaseId, ErrorId, FfiCallbackSiteId,
    FieldId, FunctionId, InterfaceId, InterfaceMethodId, IterationCaseId,
    IterationProtocolMethodId, LifetimeId, MethodId, NamespaceId, NestedFunctionId,
    NominalInterfaceId, ResultCaseId, SourceSpan, StandardFunctionId, SymbolId, SymbolIdentity,
    TypeId, UnionCaseId, ValueId,
};
use pop_runtime_interface::{
    ArrayElementMap, FfiAbiLayoutId, FfiCallbackLifetime, FfiCallbackThread, ObjectMap, ObjectSlot,
    PanicPayload, SafePointId, StackMap, Trap, UnwindReason,
};
use pop_types::{
    CallableLifetimeSummary, FloatKind, FloatValue, IntegerKind, IntegerValue, SemanticType,
    TypeArena,
};

use crate::render::{
    dump_declaration, dump_function, dump_function_reference, dump_nested_function,
};
use crate::verification::instruction_operands;
use crate::{MirFfiCallbackSignature, MirFfiLayoutCatalog};

pub(crate) const MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS: usize = 256;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirEffect {
    Allocates,
    WritesManagedReference,
    Synchronizes,
    MayTrap,
    MayUnwind,
    Suspends,
    Blocks,
    UnsafeMemory,
    ForeignFunction,
    AmbientIo,
    CompilerQuery,
    GcSafePoint,
    Roots,
}

impl MirEffect {
    const fn bit(self) -> u16 {
        1_u16 << self as u16
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MirEffectSummary(u16);

impl MirEffectSummary {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub fn from_effects(effects: impl IntoIterator<Item = MirEffect>) -> Self {
        effects.into_iter().fold(Self::empty(), Self::with)
    }

    #[must_use]
    pub const fn with(self, effect: MirEffect) -> Self {
        Self(self.0 | effect.bit())
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn is_subset_of(self, other: Self) -> bool {
        self.0 & !other.0 == 0
    }

    #[must_use]
    pub const fn contains(self, effect: MirEffect) -> bool {
        self.0 & effect.bit() != 0
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn iter(self) -> impl Iterator<Item = MirEffect> {
        const EFFECTS: [MirEffect; 13] = [
            MirEffect::Allocates,
            MirEffect::WritesManagedReference,
            MirEffect::Synchronizes,
            MirEffect::MayTrap,
            MirEffect::MayUnwind,
            MirEffect::Suspends,
            MirEffect::Blocks,
            MirEffect::UnsafeMemory,
            MirEffect::ForeignFunction,
            MirEffect::AmbientIo,
            MirEffect::CompilerQuery,
            MirEffect::GcSafePoint,
            MirEffect::Roots,
        ];
        EFFECTS
            .into_iter()
            .filter(move |effect| self.contains(*effect))
    }
}

pub(crate) fn lower_effect_summary(summary: pop_types::EffectSummary) -> MirEffectSummary {
    const EFFECTS: [(pop_types::Effect, MirEffect); 13] = [
        (pop_types::Effect::Allocates, MirEffect::Allocates),
        (
            pop_types::Effect::WritesManagedReference,
            MirEffect::WritesManagedReference,
        ),
        (pop_types::Effect::Synchronizes, MirEffect::Synchronizes),
        (pop_types::Effect::MayTrap, MirEffect::MayTrap),
        (pop_types::Effect::MayUnwind, MirEffect::MayUnwind),
        (pop_types::Effect::Suspends, MirEffect::Suspends),
        (pop_types::Effect::Blocks, MirEffect::Blocks),
        (pop_types::Effect::UnsafeMemory, MirEffect::UnsafeMemory),
        (
            pop_types::Effect::ForeignFunction,
            MirEffect::ForeignFunction,
        ),
        (pop_types::Effect::AmbientIo, MirEffect::AmbientIo),
        (pop_types::Effect::CompilerQuery, MirEffect::CompilerQuery),
        (pop_types::Effect::GcSafePoint, MirEffect::GcSafePoint),
        (pop_types::Effect::Roots, MirEffect::Roots),
    ];
    EFFECTS
        .into_iter()
        .filter_map(|(source, target)| summary.contains(source).then_some(target))
        .fold(MirEffectSummary::empty(), MirEffectSummary::with)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct MirNominalIdentity {
    pub(crate) definition: SymbolIdentity,
    pub(crate) arguments: Vec<TypeId>,
    pub(crate) canonical: pop_types::CanonicalNominalIdentity,
}

impl MirNominalIdentity {
    #[must_use]
    pub fn new(
        definition: SymbolIdentity,
        arguments: Vec<TypeId>,
        canonical_arguments: Vec<pop_types::CanonicalTypeIdentity>,
    ) -> Self {
        Self {
            definition,
            arguments,
            canonical: pop_types::CanonicalNominalIdentity::new(definition, canonical_arguments),
        }
    }

    #[must_use]
    pub const fn definition(&self) -> SymbolIdentity {
        self.definition
    }

    #[must_use]
    pub fn arguments(&self) -> &[TypeId] {
        &self.arguments
    }

    #[must_use]
    pub const fn canonical(&self) -> &pop_types::CanonicalNominalIdentity {
        &self.canonical
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInterfaceReference {
    pub(crate) identity: MirNominalIdentity,
    pub(crate) declaration: MirInterfaceDeclaration,
}

impl MirInterfaceReference {
    #[must_use]
    pub fn new(identity: MirNominalIdentity, interface: InterfaceId, type_id: TypeId) -> Self {
        Self {
            identity,
            declaration: MirInterfaceDeclaration {
                interface,
                type_id,
                methods: Vec::new(),
            },
        }
    }

    #[must_use]
    pub const fn identity(&self) -> &MirNominalIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.declaration.interface
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.declaration.type_id
    }

    #[must_use]
    pub const fn declaration(&self) -> &MirInterfaceDeclaration {
        &self.declaration
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirClassReference {
    pub(crate) identity: MirNominalIdentity,
    pub(crate) declaration: MirClassDeclaration,
    pub(crate) base_type: Option<TypeId>,
}

impl MirClassReference {
    #[must_use]
    pub fn new(
        identity: MirNominalIdentity,
        class: ClassId,
        type_id: TypeId,
        is_open: bool,
        base: Option<(ClassId, TypeId)>,
        interfaces: Vec<MirInterfaceReference>,
    ) -> Self {
        let definition = identity.definition();
        let interfaces = interfaces
            .into_iter()
            .map(|interface| MirInterfaceImplementation {
                interface: interface.interface(),
                interface_type: interface.type_id(),
                methods: Vec::new(),
            })
            .collect();
        Self {
            identity,
            declaration: MirClassDeclaration {
                definition,
                class,
                type_id,
                is_open,
                base: base.map(|(class, _)| class),
                fields: Vec::new(),
                methods: Vec::new(),
                interfaces,
                builtin_interfaces: Vec::new(),
            },
            base_type: base.map(|(_, type_id)| type_id),
        }
    }

    #[must_use]
    pub const fn identity(&self) -> &MirNominalIdentity {
        &self.identity
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.declaration.class
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.declaration.type_id
    }

    #[must_use]
    pub const fn is_open(&self) -> bool {
        self.declaration.is_open
    }

    #[must_use]
    pub const fn base(&self) -> Option<ClassId> {
        self.declaration.base
    }

    #[must_use]
    pub const fn base_type(&self) -> Option<TypeId> {
        self.base_type
    }

    #[must_use]
    pub fn interfaces(&self) -> &[MirInterfaceImplementation] {
        &self.declaration.interfaces
    }

    #[must_use]
    pub const fn declaration(&self) -> &MirClassDeclaration {
        &self.declaration
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct MirNominalReferenceCatalog {
    pub(crate) interfaces: Vec<MirInterfaceReference>,
    pub(crate) classes: Vec<MirClassReference>,
}

impl MirNominalReferenceCatalog {
    #[must_use]
    pub fn new(interfaces: Vec<MirInterfaceReference>, classes: Vec<MirClassReference>) -> Self {
        Self {
            interfaces,
            classes,
        }
    }

    #[must_use]
    pub fn interfaces(&self) -> &[MirInterfaceReference] {
        &self.interfaces
    }

    #[must_use]
    pub fn classes(&self) -> &[MirClassReference] {
        &self.classes
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirUnwindAction {
    Propagate,
    Cleanup(BlockId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirBubble {
    pub(crate) bubble: BubbleId,
    pub(crate) namespace: NamespaceId,
    pub(crate) dependencies: Vec<BubbleId>,
    pub(crate) declarations: Vec<MirDeclaration>,
    pub(crate) functions: Vec<MirFunction>,
    pub(crate) foreign_functions: Vec<MirForeignFunction>,
    pub(crate) methods: Vec<MirMethod>,
    pub(crate) nested_functions: Vec<MirNestedFunction>,
    pub(crate) function_references: Vec<MirFunctionReference>,
    pub(crate) nominal_references: MirNominalReferenceCatalog,
    pub(crate) ffi_layouts: MirFfiLayoutCatalog,
    pub(crate) generated_codec_adapters: Vec<MirGeneratedCodecAdapter>,
}

impl MirBubble {
    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub fn function_references(&self) -> &[MirFunctionReference] {
        &self.function_references
    }

    #[must_use]
    pub const fn nominal_references(&self) -> &MirNominalReferenceCatalog {
        &self.nominal_references
    }

    /// Resolves one exact class specialization into its stable structural runtime identity.
    #[must_use]
    pub fn canonical_class_identity(
        &self,
        arena: &TypeArena,
        class: ClassId,
        type_id: TypeId,
    ) -> Option<pop_types::CanonicalNominalIdentity> {
        if let Some(reference) = self
            .nominal_references()
            .classes()
            .iter()
            .find(|reference| reference.class() == class && reference.type_id() == type_id)
        {
            return Some(reference.identity().canonical().clone());
        }
        let definition =
            self.declarations()
                .iter()
                .find_map(|declaration| match declaration.kind() {
                    MirDeclarationKind::Class(candidate) if candidate.class() == class => {
                        Some(candidate.definition())
                    }
                    _ => None,
                })?;
        let SemanticType::Class {
            class: found,
            arguments,
        } = arena.get(type_id)?
        else {
            return None;
        };
        if *found != class {
            return None;
        }
        Some(pop_types::CanonicalNominalIdentity::new(
            definition,
            arguments
                .iter()
                .map(|argument| self.canonical_type_identity(arena, *argument))
                .collect::<Option<Vec<_>>>()?,
        ))
    }

    /// Resolves a closed semantic type into its stable structural runtime identity.
    #[must_use]
    pub fn canonical_type_identity(
        &self,
        arena: &TypeArena,
        type_id: TypeId,
    ) -> Option<pop_types::CanonicalTypeIdentity> {
        use pop_types::CanonicalTypeIdentity as Canonical;
        Some(match arena.get(type_id)? {
            SemanticType::Primitive(primitive) => Canonical::Primitive(*primitive),
            SemanticType::Record(_) => {
                let declaration = self.declarations().iter().find(|declaration| {
                    matches!(declaration.kind(), MirDeclarationKind::Record(record)
                        if record.type_id() == type_id)
                })?;
                Canonical::Record(SymbolIdentity::new(self.bubble(), declaration.symbol()))
            }
            SemanticType::Class { class, .. } => {
                Canonical::Class(self.canonical_class_identity(arena, *class, type_id)?)
            }
            SemanticType::Interface {
                interface,
                arguments,
            } => {
                if let Some(reference) =
                    self.nominal_references()
                        .interfaces()
                        .iter()
                        .find(|reference| {
                            reference.interface() == *interface && reference.type_id() == type_id
                        })
                {
                    Canonical::Interface(reference.identity().canonical().clone())
                } else {
                    let declaration = self.declarations().iter().find(|declaration| {
                        matches!(declaration.kind(), MirDeclarationKind::Interface(candidate)
                            if candidate.interface() == *interface)
                    })?;
                    Canonical::Interface(pop_types::CanonicalNominalIdentity::new(
                        SymbolIdentity::new(self.bubble(), declaration.symbol()),
                        arguments
                            .iter()
                            .map(|argument| self.canonical_type_identity(arena, *argument))
                            .collect::<Option<Vec<_>>>()?,
                    ))
                }
            }
            SemanticType::Tuple(elements) => Canonical::Tuple(
                elements
                    .iter()
                    .map(|element| self.canonical_type_identity(arena, *element))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SemanticType::Function {
                is_async,
                parameters,
                results,
                effects,
                lifetime_summary,
            } => Canonical::Function {
                is_async: *is_async,
                parameters: parameters
                    .iter()
                    .map(|parameter| self.canonical_type_identity(arena, *parameter))
                    .collect::<Option<Vec<_>>>()?,
                results: results
                    .iter()
                    .map(|result| self.canonical_type_identity(arena, *result))
                    .collect::<Option<Vec<_>>>()?,
                effects: *effects,
                lifetime_summary: lifetime_summary.clone(),
            },
            SemanticType::Array(element) => {
                Canonical::Array(Box::new(self.canonical_type_identity(arena, *element)?))
            }
            SemanticType::Table { key, value } => Canonical::Table {
                key: Box::new(self.canonical_type_identity(arena, *key)?),
                value: Box::new(self.canonical_type_identity(arena, *value)?),
            },
            SemanticType::Optional(element) => {
                Canonical::Optional(Box::new(self.canonical_type_identity(arena, *element)?))
            }
            SemanticType::Builtin {
                definition,
                arguments,
            } => Canonical::Builtin {
                definition: *definition,
                arguments: arguments
                    .iter()
                    .map(|argument| self.canonical_type_identity(arena, *argument))
                    .collect::<Option<Vec<_>>>()?,
            },
            SemanticType::Union(elements) => Canonical::Union(
                elements
                    .iter()
                    .map(|element| self.canonical_type_identity(arena, *element))
                    .collect::<Option<Vec<_>>>()?,
            ),
            SemanticType::TaggedUnion { .. }
            | SemanticType::ErrorUnion { .. }
            | SemanticType::Enum { .. }
            | SemanticType::Attribute { .. }
            | SemanticType::TypeParameter(_)
            | SemanticType::Opaque(_)
            | SemanticType::Error => return None,
        })
    }

    #[must_use]
    pub const fn ffi_layouts(&self) -> &MirFfiLayoutCatalog {
        &self.ffi_layouts
    }

    #[must_use]
    pub fn with_ffi_layouts(mut self, ffi_layouts: MirFfiLayoutCatalog) -> Self {
        self.ffi_layouts = ffi_layouts;
        self
    }

    #[must_use]
    pub fn functions(&self) -> &[MirFunction] {
        &self.functions
    }

    #[must_use]
    pub fn foreign_functions(&self) -> &[MirForeignFunction] {
        &self.foreign_functions
    }

    #[must_use]
    pub fn declarations(&self) -> &[MirDeclaration] {
        &self.declarations
    }

    #[must_use]
    pub fn methods(&self) -> &[MirMethod] {
        &self.methods
    }

    #[must_use]
    pub fn nested_functions(&self) -> &[MirNestedFunction] {
        &self.nested_functions
    }

    #[must_use]
    pub fn generated_codec_adapters(&self) -> &[MirGeneratedCodecAdapter] {
        &self.generated_codec_adapters
    }

    #[must_use]
    pub fn dump(&self) -> String {
        let mut output = format!(
            "mir bubble b{} namespace n{}\n",
            self.bubble.raw(),
            self.namespace.raw()
        );
        output.push_str("dependencies");
        for dependency in &self.dependencies {
            let _ = write!(output, " b{}", dependency.raw());
        }
        output.push('\n');
        for adapter in &self.generated_codec_adapters {
            let members = adapter
                .members
                .iter()
                .map(|member| {
                    let (kind, id) = match member.member {
                        MirGeneratedCodecMemberId::Field(id) => ("field", id.raw()),
                        MirGeneratedCodecMemberId::EnumCase(id) => ("enum", id.raw()),
                        MirGeneratedCodecMemberId::UnionCase(id) => ("union", id.raw()),
                    };
                    format!(
                        "{kind}:{id}:{}:{}:{}:{}",
                        member.ordinal,
                        member.name,
                        member
                            .discriminant
                            .map_or_else(|| "-".to_owned(), |value| value.to_string()),
                        member
                            .types
                            .iter()
                            .map(|type_id| format!("t{}", type_id.raw()))
                            .collect::<Vec<_>>()
                            .join(",")
                    )
                })
                .collect::<Vec<_>>()
                .join(";");
            let _ = writeln!(
                output,
                "codec.schema s{} target b{}:s{} module m{} visibility {:?} name {} targetName {} targetType t{} schemaType t{} version {} sha256 {} members {}",
                adapter.symbol.raw(),
                adapter.target.bubble().raw(),
                adapter.target.symbol().raw(),
                adapter.module.raw(),
                adapter.visibility,
                adapter.name,
                adapter.target_name,
                adapter.target_type.raw(),
                adapter.schema_type.raw(),
                adapter.schema_version,
                adapter.projection_sha256,
                members,
            );
        }
        for reference in self.nominal_references.interfaces() {
            crate::render::dump_nominal_interface_reference(&mut output, reference);
        }
        for reference in self.nominal_references.classes() {
            crate::render::dump_nominal_class_reference(&mut output, reference);
        }
        for reference in &self.function_references {
            dump_function_reference(&mut output, reference);
        }
        for declaration in &self.declarations {
            dump_declaration(&mut output, declaration);
        }
        for function in &self.functions {
            dump_function(&mut output, function);
        }
        for function in &self.foreign_functions {
            crate::render::dump_foreign_function(&mut output, function);
        }
        for method in &self.methods {
            let _ = writeln!(
                output,
                "method m{} c{}",
                method.method.raw(),
                method.class.raw()
            );
            dump_function(&mut output, &method.function);
        }
        for function in &self.nested_functions {
            dump_nested_function(&mut output, function);
        }
        output
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirGeneratedCodecMemberId {
    Field(FieldId),
    EnumCase(EnumCaseId),
    UnionCase(UnionCaseId),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirGeneratedCodecMember {
    pub(crate) ordinal: u16,
    pub(crate) name: String,
    pub(crate) member: MirGeneratedCodecMemberId,
    pub(crate) types: Vec<TypeId>,
    pub(crate) discriminant: Option<u32>,
}

impl MirGeneratedCodecMember {
    #[must_use]
    pub const fn new(
        ordinal: u16,
        name: String,
        member: MirGeneratedCodecMemberId,
        types: Vec<TypeId>,
        discriminant: Option<u32>,
    ) -> Self {
        Self {
            ordinal,
            name,
            member,
            types,
            discriminant,
        }
    }
    #[must_use]
    pub const fn ordinal(&self) -> u16 {
        self.ordinal
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub const fn member(&self) -> MirGeneratedCodecMemberId {
        self.member
    }
    #[must_use]
    pub fn types(&self) -> &[TypeId] {
        &self.types
    }
    #[must_use]
    pub const fn discriminant(&self) -> Option<u32> {
        self.discriminant
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirGeneratedCodecAdapter {
    pub(crate) symbol: SymbolId,
    pub(crate) target: SymbolIdentity,
    pub(crate) module: pop_foundation::ModuleId,
    pub(crate) visibility: pop_resolve::Visibility,
    pub(crate) name: String,
    pub(crate) target_name: String,
    pub(crate) target_type: TypeId,
    pub(crate) schema_type: TypeId,
    pub(crate) schema_version: u32,
    pub(crate) projection_sha256: String,
    pub(crate) members: Vec<MirGeneratedCodecMember>,
}

impl MirGeneratedCodecAdapter {
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub const fn new(
        symbol: SymbolId,
        target: SymbolIdentity,
        module: pop_foundation::ModuleId,
        visibility: pop_resolve::Visibility,
        name: String,
        target_name: String,
        target_type: TypeId,
        schema_type: TypeId,
        schema_version: u32,
        projection_sha256: String,
        members: Vec<MirGeneratedCodecMember>,
    ) -> Self {
        Self {
            symbol,
            target,
            module,
            visibility,
            name,
            target_name,
            target_type,
            schema_type,
            schema_version,
            projection_sha256,
            members,
        }
    }
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }
    #[must_use]
    pub const fn target(&self) -> SymbolIdentity {
        self.target
    }
    #[must_use]
    pub const fn target_type(&self) -> TypeId {
        self.target_type
    }
    #[must_use]
    pub const fn schema_type(&self) -> TypeId {
        self.schema_type
    }
    #[must_use]
    pub fn projection_sha256(&self) -> &str {
        &self.projection_sha256
    }
    #[must_use]
    pub fn members(&self) -> &[MirGeneratedCodecMember] {
        &self.members
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirForeignFunction {
    pub(crate) function: FunctionId,
    pub(crate) symbol: SymbolId,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) parameter_layouts: Vec<Option<FfiAbiLayoutId>>,
    pub(crate) result_layouts: Vec<Option<FfiAbiLayoutId>>,
    pub(crate) effects: MirEffectSummary,
    pub(crate) declaration: pop_types::ForeignFunctionDeclaration,
    pub(crate) reference_identity: Option<SymbolIdentity>,
}

impl MirForeignFunction {
    #[must_use]
    pub const fn function(&self) -> FunctionId {
        self.function
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    /// Returns the exact target catalog binding for each by-value layout
    /// parameter. Non-record ABI values have no record-layout binding.
    #[must_use]
    pub fn parameter_layouts(&self) -> &[Option<FfiAbiLayoutId>] {
        &self.parameter_layouts
    }

    /// Returns the exact target catalog binding for each by-value layout
    /// result. Non-record ABI values have no record-layout binding.
    #[must_use]
    pub fn result_layouts(&self) -> &[Option<FfiAbiLayoutId>] {
        &self.result_layouts
    }

    #[must_use]
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }

    #[must_use]
    pub const fn declaration(&self) -> &pop_types::ForeignFunctionDeclaration {
        &self.declaration
    }

    /// Returns the producer identity when this foreign declaration originated
    /// in direct-dependency reference metadata.
    #[must_use]
    pub const fn reference_identity(&self) -> Option<SymbolIdentity> {
        self.reference_identity
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirFunctionReference {
    pub(crate) identity: SymbolIdentity,
    pub(crate) is_async: bool,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) effects: MirEffectSummary,
    pub(crate) lifetime_summary: CallableLifetimeSummary,
}

impl MirFunctionReference {
    #[must_use]
    pub const fn identity(&self) -> SymbolIdentity {
        self.identity
    }

    #[must_use]
    pub const fn is_async(&self) -> bool {
        self.is_async
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
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }

    #[must_use]
    pub const fn lifetime_summary(&self) -> &CallableLifetimeSummary {
        &self.lifetime_summary
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirDeclaration {
    pub(crate) symbol: SymbolId,
    pub(crate) kind: MirDeclarationKind,
}

impl MirDeclaration {
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn kind(&self) -> &MirDeclarationKind {
        &self.kind
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirDeclarationKind {
    Record(MirRecordDeclaration),
    Union(MirUnionDeclaration),
    Error(MirErrorDeclaration),
    Enum(MirEnumDeclaration),
    Class(MirClassDeclaration),
    Interface(MirInterfaceDeclaration),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirErrorDeclaration {
    pub(crate) error: ErrorId,
    pub(crate) type_id: TypeId,
    pub(crate) cases: Vec<MirErrorCase>,
}

impl MirErrorDeclaration {
    #[must_use]
    pub const fn error(&self) -> ErrorId {
        self.error
    }
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }
    #[must_use]
    pub fn cases(&self) -> &[MirErrorCase] {
        &self.cases
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirErrorCase {
    pub(crate) case: ErrorCaseId,
    pub(crate) parameters: Vec<TypeId>,
}

impl MirErrorCase {
    #[must_use]
    pub const fn case(&self) -> ErrorCaseId {
        self.case
    }
    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirEnumDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) cases: Vec<MirEnumCase>,
}

impl MirEnumDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[MirEnumCase] {
        &self.cases
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirEnumCase {
    pub(crate) case: EnumCaseId,
    pub(crate) discriminant: u32,
}

impl MirEnumCase {
    #[must_use]
    pub const fn case(self) -> EnumCaseId {
        self.case
    }

    #[must_use]
    pub const fn discriminant(self) -> u32 {
        self.discriminant
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirRecordDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) fields: Vec<MirField>,
}

impl MirRecordDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn fields(&self) -> &[MirField] {
        &self.fields
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirUnionDeclaration {
    pub(crate) type_id: TypeId,
    pub(crate) cases: Vec<MirUnionCase>,
}

impl MirUnionDeclaration {
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn cases(&self) -> &[MirUnionCase] {
        &self.cases
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirClassDeclaration {
    pub(crate) definition: SymbolIdentity,
    pub(crate) class: ClassId,
    pub(crate) type_id: TypeId,
    pub(crate) is_open: bool,
    pub(crate) base: Option<ClassId>,
    pub(crate) fields: Vec<MirField>,
    pub(crate) methods: Vec<MethodId>,
    pub(crate) interfaces: Vec<MirInterfaceImplementation>,
    pub(crate) builtin_interfaces: Vec<MirBuiltinInterfaceImplementation>,
}

impl MirClassDeclaration {
    #[must_use]
    pub const fn definition(&self) -> SymbolIdentity {
        self.definition
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
    pub const fn base(&self) -> Option<ClassId> {
        self.base
    }

    #[must_use]
    pub fn fields(&self) -> &[MirField] {
        &self.fields
    }

    #[must_use]
    pub fn methods(&self) -> &[MethodId] {
        &self.methods
    }

    #[must_use]
    pub fn interfaces(&self) -> &[MirInterfaceImplementation] {
        &self.interfaces
    }

    #[must_use]
    pub fn builtin_interfaces(&self) -> &[MirBuiltinInterfaceImplementation] {
        &self.builtin_interfaces
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirBuiltinInterfaceImplementation {
    pub(crate) interface: BuiltinTypeId,
    pub(crate) interface_type: TypeId,
    pub(crate) methods: Vec<MirBuiltinInterfaceMethodImplementation>,
}

impl MirBuiltinInterfaceImplementation {
    #[must_use]
    pub const fn interface(&self) -> BuiltinTypeId {
        self.interface
    }

    #[must_use]
    pub const fn interface_type(&self) -> TypeId {
        self.interface_type
    }

    #[must_use]
    pub fn methods(&self) -> &[MirBuiltinInterfaceMethodImplementation] {
        &self.methods
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirBuiltinInterfaceMethodImplementation {
    pub(crate) protocol_method: IterationProtocolMethodId,
    pub(crate) class_method: MethodId,
}

impl MirBuiltinInterfaceMethodImplementation {
    #[must_use]
    pub const fn protocol_method(&self) -> IterationProtocolMethodId {
        self.protocol_method
    }

    #[must_use]
    pub const fn class_method(&self) -> MethodId {
        self.class_method
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInterfaceDeclaration {
    pub(crate) interface: InterfaceId,
    pub(crate) type_id: TypeId,
    pub(crate) methods: Vec<MirInterfaceMethod>,
}

impl MirInterfaceDeclaration {
    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }

    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub fn methods(&self) -> &[MirInterfaceMethod] {
        &self.methods
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInterfaceMethod {
    pub(crate) method: InterfaceMethodId,
    pub(crate) slot: u32,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) effects: MirEffectSummary,
}

impl MirInterfaceMethod {
    #[must_use]
    pub const fn method(&self) -> InterfaceMethodId {
        self.method
    }
    #[must_use]
    pub const fn slot(&self) -> u32 {
        self.slot
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
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInterfaceImplementation {
    pub(crate) interface: InterfaceId,
    pub(crate) interface_type: TypeId,
    pub(crate) methods: Vec<MirInterfaceMethodImplementation>,
}

impl MirInterfaceImplementation {
    #[must_use]
    pub const fn interface(&self) -> InterfaceId {
        self.interface
    }
    #[must_use]
    pub const fn interface_type(&self) -> TypeId {
        self.interface_type
    }
    #[must_use]
    pub fn methods(&self) -> &[MirInterfaceMethodImplementation] {
        &self.methods
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirInterfaceMethodImplementation {
    pub(crate) interface_method: InterfaceMethodId,
    pub(crate) slot: u32,
    pub(crate) class_method: MethodId,
}

impl MirInterfaceMethodImplementation {
    #[must_use]
    pub const fn interface_method(self) -> InterfaceMethodId {
        self.interface_method
    }
    #[must_use]
    pub const fn slot(self) -> u32 {
        self.slot
    }
    #[must_use]
    pub const fn class_method(self) -> MethodId {
        self.class_method
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirField {
    pub(crate) field: FieldId,
    pub(crate) field_type: TypeId,
}

impl MirField {
    #[must_use]
    pub const fn field(&self) -> FieldId {
        self.field
    }

    #[must_use]
    pub const fn field_type(&self) -> TypeId {
        self.field_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirUnionCase {
    pub(crate) case: UnionCaseId,
    pub(crate) parameters: Vec<TypeId>,
}

impl MirUnionCase {
    #[must_use]
    pub const fn case(&self) -> UnionCaseId {
        self.case
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirMethod {
    pub(crate) method: MethodId,
    pub(crate) class: ClassId,
    pub(crate) function: MirFunction,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirNestedFunction {
    pub(crate) owner: SymbolId,
    pub(crate) function: NestedFunctionId,
    pub(crate) captures: Vec<MirCapture>,
    pub(crate) is_async: bool,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) effects: MirEffectSummary,
    pub(crate) effects_explicit: bool,
    pub(crate) blocks: Vec<MirBlock>,
}

impl MirNestedFunction {
    pub(crate) fn transformation_adapter(&self) -> MirFunction {
        MirFunction {
            function: FunctionId::from_raw(self.function.raw()),
            symbol: self.owner,
            is_async: self.is_async,
            parameters: self.parameters.clone(),
            parameter_view_borrows: vec![None; self.parameters.len()],
            results: self.results.clone(),
            lifetime_summary: CallableLifetimeSummary::conservative(
                self.parameters.len(),
                self.results.len(),
            ),
            effects: self.effects,
            effects_explicit: self.effects_explicit,
            blocks: self.blocks.clone(),
        }
    }

    pub(crate) fn apply_transformation(&mut self, function: MirFunction) {
        self.effects = function.effects;
        self.effects_explicit = function.effects_explicit;
        self.blocks = function.blocks;
    }

    #[must_use]
    pub const fn owner(&self) -> SymbolId {
        self.owner
    }
    #[must_use]
    pub const fn function(&self) -> NestedFunctionId {
        self.function
    }
    #[must_use]
    pub const fn is_async(&self) -> bool {
        self.is_async
    }
    #[must_use]
    pub fn captures(&self) -> &[MirCapture] {
        &self.captures
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
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }
    #[must_use]
    pub fn blocks(&self) -> &[MirBlock] {
        &self.blocks
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirCapture {
    pub(crate) capture: CaptureId,
    pub(crate) binding: BindingId,
    pub(crate) slot: u32,
    pub(crate) type_id: TypeId,
    pub(crate) mode: MirCaptureMode,
}

impl MirCapture {
    #[must_use]
    pub const fn capture(self) -> CaptureId {
        self.capture
    }
    #[must_use]
    pub const fn binding(self) -> BindingId {
        self.binding
    }
    #[must_use]
    pub const fn slot(self) -> u32 {
        self.slot
    }
    #[must_use]
    pub const fn type_id(self) -> TypeId {
        self.type_id
    }
    #[must_use]
    pub const fn mode(self) -> MirCaptureMode {
        self.mode
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirCaptureMode {
    Value,
    Cell,
}

impl MirMethod {
    #[must_use]
    pub const fn method(&self) -> MethodId {
        self.method
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn function(&self) -> &MirFunction {
        &self.function
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirFunction {
    pub(crate) function: FunctionId,
    pub(crate) symbol: SymbolId,
    pub(crate) is_async: bool,
    pub(crate) parameters: Vec<TypeId>,
    pub(crate) parameter_view_borrows: Vec<Option<MirViewParameterBorrow>>,
    pub(crate) results: Vec<TypeId>,
    pub(crate) lifetime_summary: CallableLifetimeSummary,
    pub(crate) effects: MirEffectSummary,
    pub(crate) effects_explicit: bool,
    pub(crate) blocks: Vec<MirBlock>,
}

impl MirFunction {
    #[must_use]
    pub const fn function(&self) -> FunctionId {
        self.function
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub const fn is_async(&self) -> bool {
        self.is_async
    }

    #[must_use]
    pub fn parameters(&self) -> &[TypeId] {
        &self.parameters
    }

    #[must_use]
    pub fn parameter_view_borrows(&self) -> &[Option<MirViewParameterBorrow>] {
        &self.parameter_view_borrows
    }

    #[must_use]
    pub fn with_parameter_view_borrows(
        mut self,
        parameter_view_borrows: Vec<Option<MirViewParameterBorrow>>,
    ) -> Self {
        self.parameter_view_borrows = parameter_view_borrows;
        self
    }

    #[must_use]
    pub fn results(&self) -> &[TypeId] {
        &self.results
    }

    #[must_use]
    pub const fn lifetime_summary(&self) -> &CallableLifetimeSummary {
        &self.lifetime_summary
    }

    #[must_use]
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }

    #[must_use]
    pub fn blocks(&self) -> &[MirBlock] {
        &self.blocks
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirBlock {
    pub(crate) block: BlockId,
    pub(crate) cleanup: Option<MirCleanupBlock>,
    pub(crate) arguments: Vec<MirBlockArgument>,
    pub(crate) instructions: Vec<MirInstruction>,
    pub(crate) terminator: MirTerminator,
}

impl MirBlock {
    #[must_use]
    pub const fn block(&self) -> BlockId {
        self.block
    }

    #[must_use]
    pub const fn cleanup(&self) -> Option<MirCleanupBlock> {
        self.cleanup
    }

    #[must_use]
    pub fn arguments(&self) -> &[MirBlockArgument] {
        &self.arguments
    }

    #[must_use]
    pub fn instructions(&self) -> &[MirInstruction] {
        &self.instructions
    }

    #[must_use]
    pub const fn terminator(&self) -> &MirTerminator {
        &self.terminator
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirCleanupBlock {
    pub(crate) scope: CleanupScopeId,
    pub(crate) reason: MirCleanupExitReason,
}

impl MirCleanupBlock {
    #[must_use]
    pub const fn scope(self) -> CleanupScopeId {
        self.scope
    }

    #[must_use]
    pub const fn reason(self) -> MirCleanupExitReason {
        self.reason
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirCleanupExitReason {
    Normal,
    Return,
    ResultFailure,
    Break,
    Continue,
    Unwind,
    Cancellation,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirBlockArgument {
    pub(crate) value: ValueId,
    pub(crate) type_id: TypeId,
    pub(crate) span: SourceSpan,
}

impl MirBlockArgument {
    #[must_use]
    pub const fn value(self) -> ValueId {
        self.value
    }

    #[must_use]
    pub const fn type_id(self) -> TypeId {
        self.type_id
    }

    #[must_use]
    pub const fn span(self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirInstruction {
    pub(crate) result: ValueId,
    pub(crate) result_type: Option<TypeId>,
    pub(crate) kind: MirInstructionKind,
    pub(crate) effects: MirEffectSummary,
    pub(crate) effects_explicit: bool,
    pub(crate) unwind: MirUnwindAction,
    pub(crate) span: SourceSpan,
}

impl MirInstruction {
    #[must_use]
    pub const fn result(&self) -> ValueId {
        self.result
    }

    #[must_use]
    /// Returns the SSA result type of a value-producing instruction.
    ///
    /// # Panics
    ///
    /// Panics when called for an explicit effect instruction with no result.
    /// Use [`Self::optional_result_type`] when either form is valid.
    pub const fn result_type(&self) -> TypeId {
        self.result_type
            .expect("value-producing MIR instruction has a result type")
    }

    #[must_use]
    pub const fn optional_result_type(&self) -> Option<TypeId> {
        self.result_type
    }

    #[must_use]
    pub const fn has_result(&self) -> bool {
        self.result_type.is_some()
    }

    #[must_use]
    pub const fn kind(&self) -> &MirInstructionKind {
        &self.kind
    }

    /// Returns the ordinary SSA operands read by this instruction.
    ///
    /// `GcSafePoint` roots are stack-map metadata and are intentionally not
    /// ordinary operands. `CallForeign` transition roots are semantic uses
    /// held across native execution and therefore are ordinary operands.
    #[must_use]
    pub fn operands(&self) -> Vec<ValueId> {
        instruction_operands(&self.kind)
    }

    #[must_use]
    pub const fn effects(&self) -> MirEffectSummary {
        self.effects
    }

    #[must_use]
    pub const fn unwind_action(&self) -> MirUnwindAction {
        match &self.kind {
            MirInstructionKind::CallDirect { unwind, .. }
            | MirInstructionKind::CallForeign { unwind, .. }
            | MirInstructionKind::CallReferenced { unwind, .. }
            | MirInstructionKind::CallDirectMethod { unwind, .. }
            | MirInstructionKind::CallInterface { unwind, .. }
            | MirInstructionKind::CallIndirect { unwind, .. } => *unwind,
            MirInstructionKind::CallScopedBorrow { unwind, .. } => *unwind,
            _ => self.unwind,
        }
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

/// The only compiler-proven non-owning view families accepted by ADR 0093.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirViewKind {
    Bytes,
    Text,
}

impl MirViewKind {
    #[must_use]
    pub const fn range_unit(self) -> MirViewRangeUnit {
        match self {
            Self::Bytes => MirViewRangeUnit::Bytes,
            Self::Text => MirViewRangeUnit::UnicodeScalars,
        }
    }

    #[must_use]
    pub const fn boundary_proof(self) -> MirViewBoundaryProof {
        match self {
            Self::Bytes => MirViewBoundaryProof::NotApplicable,
            Self::Text => MirViewBoundaryProof::Utf8Scalar,
        }
    }
}

/// Unit used by one checked view range.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirViewRangeUnit {
    Bytes,
    UnicodeScalars,
}

/// Closed endpoint proof attached to the descriptor's checked range.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirViewBoundaryProof {
    NotApplicable,
    Utf8Scalar,
}

/// Stable source of the immutable storage designated by a view.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirViewLender {
    Allocation { site: AllocationSiteId },
    Parameter { index: u32 },
    Constant { fingerprint: [u8; 32] },
}

/// Exact caller-owned borrow created from a callable result alias.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirCallViewResult {
    pub(crate) kind: MirViewKind,
    pub(crate) source_argument: u16,
    pub(crate) borrow_lifetime: LifetimeId,
}

impl MirCallViewResult {
    #[must_use]
    pub const fn new(kind: MirViewKind, source_argument: u16, borrow_lifetime: LifetimeId) -> Self {
        Self {
            kind,
            source_argument,
            borrow_lifetime,
        }
    }

    #[must_use]
    pub const fn kind(self) -> MirViewKind {
        self.kind
    }

    #[must_use]
    pub const fn source_argument(self) -> u16 {
        self.source_argument
    }

    #[must_use]
    pub const fn borrow_lifetime(self) -> LifetimeId {
        self.borrow_lifetime
    }
}

impl MirViewLender {
    #[must_use]
    pub const fn parameter_index(self) -> Option<u32> {
        match self {
            Self::Parameter { index } => Some(index),
            Self::Allocation { .. } | Self::Constant { .. } => None,
        }
    }
}

/// The sole checked trap selected by first-release slicing.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum MirViewTrap {
    BoundsViolation,
}

/// One callee-local borrow identity aligned to a view parameter.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct MirViewParameterBorrow {
    kind: MirViewKind,
    lender_provenance: MirViewLender,
    borrow_lifetime: LifetimeId,
}

impl MirViewParameterBorrow {
    #[must_use]
    pub const fn new(
        kind: MirViewKind,
        lender_provenance: MirViewLender,
        borrow_lifetime: LifetimeId,
    ) -> Self {
        Self {
            kind,
            lender_provenance,
            borrow_lifetime,
        }
    }

    #[must_use]
    pub const fn kind(self) -> MirViewKind {
        self.kind
    }

    #[must_use]
    pub const fn lender_provenance(self) -> MirViewLender {
        self.lender_provenance
    }

    #[must_use]
    pub const fn borrow_lifetime(self) -> LifetimeId {
        self.borrow_lifetime
    }
}

impl fmt::Display for MirViewTrap {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::BoundsViolation => "BoundsViolation",
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirInstructionKind {
    IntegerConstant(IntegerValue),
    FloatConstant(FloatValue),
    StringConstant(String),
    StringConcat {
        left: ValueId,
        right: ValueId,
    },
    StringFormat {
        kind: pop_types::StringFormatKind,
        value: ValueId,
    },
    BooleanConstant(bool),
    NilConstant,
    OptionalIsPresent {
        optional: ValueId,
    },
    OptionalGet {
        optional: ValueId,
    },
    ResultMake {
        result: BuiltinTypeId,
        case: ResultCaseId,
        arguments: Vec<ValueId>,
    },
    IterationMake {
        iteration: BuiltinTypeId,
        case: IterationCaseId,
        arguments: Vec<ValueId>,
    },
    ErrorMake {
        error: ErrorId,
        case: ErrorCaseId,
        arguments: Vec<ValueId>,
    },
    ResultIsOk {
        result: ValueId,
        definition: BuiltinTypeId,
    },
    ResultGetOk {
        result: ValueId,
        definition: BuiltinTypeId,
    },
    ResultGetError {
        result: ValueId,
        definition: BuiltinTypeId,
    },
    EnumConstant {
        definition: SymbolId,
        case: EnumCaseId,
        discriminant: u32,
    },
    /// One closed compiler-known `Codec.Error` case (ADR 0092).
    CodecErrorConstant {
        case: EnumCaseId,
    },
    FunctionReference(SymbolId),
    /// Compiler-originated sealed `Codec.Schema<T>` value. The referenced
    /// adapter is verified in the Bubble's closed generated-adapter catalog.
    GeneratedCodecSchema(SymbolId),
    CodecEncode {
        adapter: SymbolId,
        value: ValueId,
        writer: ValueId,
        result: BuiltinTypeId,
        success: ResultCaseId,
        failure: ResultCaseId,
    },
    CodecDecode {
        adapter: SymbolId,
        reader: ValueId,
        result: BuiltinTypeId,
        success: ResultCaseId,
        failure: ResultCaseId,
    },
    TaskCreate {
        dispatch: MirTaskDispatch,
        arguments: Vec<ValueId>,
        completion_type: TypeId,
        object_map: ObjectMap,
    },
    CancelSourceCreate,
    CancelSourceToken {
        source: ValueId,
    },
    CancelRequest {
        source: ValueId,
    },
    TaskGroupCreate {
        cancel: ValueId,
        body: ValueId,
        completion_type: TypeId,
        object_map: ObjectMap,
    },
    TaskStart {
        group: ValueId,
        task: ValueId,
    },
    TupleMake(Vec<ValueId>),
    TupleGet {
        tuple: ValueId,
        index: u32,
    },
    ArrayMake {
        elements: Vec<ValueId>,
        element_map: ArrayElementMap,
    },
    ArrayCreate {
        length: ValueId,
        initial_value: ValueId,
        element_map: ArrayElementMap,
    },
    TableMake {
        entries: Vec<(ValueId, ValueId)>,
        key_map: ArrayElementMap,
        value_map: ArrayElementMap,
    },
    TableGet {
        table: ValueId,
        key: ValueId,
    },
    TableSet {
        table: ValueId,
        key: ValueId,
        value: ValueId,
        key_map: ArrayElementMap,
        value_map: ArrayElementMap,
    },
    ArrayGet {
        array: ValueId,
        index: ValueId,
    },
    ArrayLength {
        array: ValueId,
    },
    ArrayGetChecked {
        array: ValueId,
        index: ValueId,
    },
    ArraySet {
        array: ValueId,
        index: ValueId,
        value: ValueId,
        element_map: ArrayElementMap,
    },
    ArrayFill {
        array: ValueId,
        value: ValueId,
        element_map: ArrayElementMap,
    },
    ListCreate {
        capacity: Option<ValueId>,
        element_map: ArrayElementMap,
    },
    ListLength {
        list: ValueId,
    },
    ListGet {
        list: ValueId,
        index: ValueId,
    },
    ListGetChecked {
        list: ValueId,
        index: ValueId,
    },
    ListSet {
        list: ValueId,
        index: ValueId,
        value: ValueId,
        element_map: ArrayElementMap,
    },
    ListAdd {
        list: ValueId,
        value: ValueId,
        element_map: ArrayElementMap,
    },
    RangeCreate {
        first: ValueId,
        last: ValueId,
        step: ValueId,
    },
    CheckedIntegerAdd {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerSubtract {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerMultiply {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerDivide {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CheckedIntegerRemainder {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    FloatAdd {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    FloatSubtract {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    FloatMultiply {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    FloatDivide {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    BooleanNot {
        operand: ValueId,
    },
    IntegerNegate {
        kind: IntegerKind,
        operand: ValueId,
    },
    FloatNegate {
        kind: FloatKind,
        operand: ValueId,
    },
    ConvertInteger {
        source: IntegerKind,
        target: IntegerKind,
        operand: ValueId,
    },
    ConvertIntegerToFloat {
        source: IntegerKind,
        target: FloatKind,
        operand: ValueId,
    },
    ConvertFloatToInteger {
        source: FloatKind,
        target: IntegerKind,
        operand: ValueId,
    },
    ConvertFloat {
        source: FloatKind,
        target: FloatKind,
        operand: ValueId,
    },
    BooleanAnd {
        left: ValueId,
        right: ValueId,
    },
    BooleanOr {
        left: ValueId,
        right: ValueId,
    },
    CompareEqual {
        left: ValueId,
        right: ValueId,
    },
    CompareNotEqual {
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerLess {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerLessOrEqual {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerGreater {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareIntegerGreaterOrEqual {
        kind: IntegerKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatLess {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatLessOrEqual {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatGreater {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CompareFloatGreaterOrEqual {
        kind: FloatKind,
        left: ValueId,
        right: ValueId,
    },
    CallDirect {
        function: SymbolId,
        arguments: Vec<ValueId>,
        lifetime_summary: CallableLifetimeSummary,
        view_result: Option<MirCallViewResult>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallForeign {
        function: SymbolId,
        arguments: Vec<ValueId>,
        safe_point: SafePointId,
        roots: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallReferenced {
        function: SymbolIdentity,
        arguments: Vec<ValueId>,
        lifetime_summary: CallableLifetimeSummary,
        view_result: Option<MirCallViewResult>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallStandard {
        function: StandardFunctionId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
    },
    CallDirectMethod {
        method: MethodId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallInterface {
        interface: InterfaceId,
        method: InterfaceMethodId,
        slot: u32,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallBuiltinInterface {
        interface: BuiltinTypeId,
        method: IterationProtocolMethodId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallIndirect {
        callee: ValueId,
        arguments: Vec<ValueId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    CallScopedBorrow {
        owner: SymbolId,
        function: NestedFunctionId,
        captures: Vec<MirClosureCapture>,
        arguments: Vec<ValueId>,
        region: BorrowRegionId,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    FfiCallbackOpenScoped {
        callback: ValueId,
        callback_type: TypeId,
        owner: SymbolId,
        function: NestedFunctionId,
        site: FfiCallbackSiteId,
        region: BorrowRegionId,
    },
    FfiCallbackOpenOwned {
        callback: ValueId,
        callback_type: TypeId,
        owner: SymbolId,
        function: NestedFunctionId,
        site: FfiCallbackSiteId,
        thread: FfiCallbackThread,
        result: BuiltinTypeId,
        success: ResultCaseId,
        failure: ResultCaseId,
    },
    CallCallbackPair {
        callback: ValueId,
        signature: MirFfiCallbackSignature,
        owner: SymbolId,
        function: NestedFunctionId,
        captures: Vec<MirClosureCapture>,
        region: BorrowRegionId,
        lifetime: FfiCallbackLifetime,
        result: Option<BuiltinTypeId>,
        success: Option<ResultCaseId>,
        failure: Option<ResultCaseId>,
        declared_effects: MirEffectSummary,
        unwind: MirUnwindAction,
    },
    FfiCallbackCloseScoped {
        callback: ValueId,
        region: BorrowRegionId,
    },
    FfiCallbackCloseOwned {
        callback: ValueId,
        result: BuiltinTypeId,
        success: ResultCaseId,
        failure: ResultCaseId,
    },
    RecordMake {
        record: SymbolId,
        fields: Vec<(FieldId, ValueId)>,
    },
    ClassMake {
        class: ClassId,
        fields: Vec<(FieldId, ValueId)>,
        object_map: ObjectMap,
    },
    RecordUpdate {
        record: SymbolId,
        base: ValueId,
        fields: Vec<(FieldId, ValueId)>,
    },
    FieldGet {
        base: ValueId,
        field: FieldId,
    },
    FieldSet {
        base: ValueId,
        field: FieldId,
        value: ValueId,
    },
    UnionMake {
        union: SymbolId,
        case: UnionCaseId,
        arguments: Vec<ValueId>,
    },
    IterationIsItem {
        iteration: ValueId,
        definition: BuiltinTypeId,
        item_case: IterationCaseId,
        end_case: IterationCaseId,
    },
    IterationGetItem {
        iteration: ValueId,
        definition: BuiltinTypeId,
        item_case: IterationCaseId,
    },
    InterfaceUpcast {
        value: ValueId,
        interface: NominalInterfaceId,
    },
    CheckedDowncast {
        value: ValueId,
        source_interface: InterfaceId,
        source_type: TypeId,
        target_class: ClassId,
        target_type: TypeId,
    },
    ViewCreate {
        kind: MirViewKind,
        lender: ValueId,
        lender_provenance: MirViewLender,
        range_unit: MirViewRangeUnit,
        boundary: MirViewBoundaryProof,
        borrow_lifetime: LifetimeId,
    },
    ViewSlice {
        kind: MirViewKind,
        view: ValueId,
        start: ValueId,
        length: ValueId,
        lender_provenance: MirViewLender,
        range_unit: MirViewRangeUnit,
        boundary: MirViewBoundaryProof,
        parent_lifetime: LifetimeId,
        borrow_lifetime: LifetimeId,
        bounds_trap: MirViewTrap,
    },
    ViewLength {
        kind: MirViewKind,
        view: ValueId,
    },
    ViewGetByte {
        view: ValueId,
        index: ValueId,
    },
    ViewMaterialize {
        kind: MirViewKind,
        view: ValueId,
        allocation_site: AllocationSiteId,
    },
    ViewEnd {
        borrow_lifetime: LifetimeId,
    },
    CaptureCellAllocate {
        binding: BindingId,
        initial: ValueId,
        value_type: TypeId,
        object_map: ObjectMap,
    },
    CaptureCellLoad {
        cell: ValueId,
    },
    CaptureCellStore {
        cell: ValueId,
        value: ValueId,
    },
    ClosureEnvironmentAllocate {
        owner: SymbolId,
        function: NestedFunctionId,
        captures: Vec<MirClosureCapture>,
        object_map: ObjectMap,
    },
    CaptureLoad {
        capture: CaptureId,
        slot: u32,
        mode: MirCaptureMode,
    },
    CaptureCellReference {
        capture: CaptureId,
        slot: u32,
    },
    CaptureStore {
        capture: CaptureId,
        slot: u32,
        value: ValueId,
    },
    GcSafePoint {
        safe_point: SafePointId,
        roots: Vec<ValueId>,
        stack_map: StackMap,
    },
    RetainRoot {
        value: ValueId,
    },
    ReleaseRoot {
        handle: ValueId,
    },
    FfiHandleOpen {
        value: ValueId,
    },
    FfiHandleGet {
        handle: ValueId,
    },
    FfiHandleClose {
        handle: ValueId,
    },
    FfiBufferOpen {
        length: ValueId,
        element: TypeId,
        layout: FfiAbiLayoutId,
        element_size: u64,
        alignment: u64,
        result: BuiltinTypeId,
        success: ResultCaseId,
        failure: ResultCaseId,
    },
    FfiBufferLength {
        buffer: ValueId,
        layout: FfiAbiLayoutId,
    },
    FfiBufferRead {
        buffer: ValueId,
        index: ValueId,
        layout: FfiAbiLayoutId,
    },
    FfiBufferWrite {
        buffer: ValueId,
        index: ValueId,
        value: ValueId,
        layout: FfiAbiLayoutId,
    },
    FfiBufferBorrow {
        buffer: ValueId,
        expected_length: ValueId,
        layout: FfiAbiLayoutId,
        region: BorrowRegionId,
    },
    FfiBufferEndBorrow {
        buffer: ValueId,
        region: BorrowRegionId,
    },
    FfiBufferClose {
        buffer: ValueId,
    },
    FfiBytesBorrow {
        bytes: ValueId,
        region: BorrowRegionId,
    },
    FfiBytesBorrowLength {
        bytes: ValueId,
        region: BorrowRegionId,
    },
    FfiBytesEndBorrow {
        bytes: ValueId,
        region: BorrowRegionId,
    },
    FfiPointerNone,
    FfiPointerToOptional {
        pointer: ValueId,
    },
    FfiPointerReadOnly {
        pointer: ValueId,
    },
    FfiPointerIsPresent {
        pointer: ValueId,
    },
    FfiPointerRequire {
        pointer: ValueId,
        result: BuiltinTypeId,
        success: ResultCaseId,
        failure: ResultCaseId,
    },
    FfiUnsafeLoad {
        pointer: ValueId,
        layout: FfiAbiLayoutId,
    },
    FfiUnsafeStore {
        pointer: ValueId,
        value: ValueId,
        layout: FfiAbiLayoutId,
    },
    FfiUnsafeAdvance {
        pointer: ValueId,
        elements: ValueId,
        layout: FfiAbiLayoutId,
        read_only: bool,
    },
    FfiUnsafeCopy {
        source: ValueId,
        destination: ValueId,
        count: ValueId,
        layout: FfiAbiLayoutId,
    },
    FfiUnsafeAddress {
        pointer: ValueId,
        layout: FfiAbiLayoutId,
    },
    FfiUnsafePointerFromAddress {
        address: ValueId,
        layout: FfiAbiLayoutId,
    },
    Pin {
        value: ValueId,
    },
    Unpin {
        handle: ValueId,
    },
    WriteBarrier {
        owner: ValueId,
        slot: ObjectSlot,
        previous: Option<ValueId>,
        value: Option<ValueId>,
        proof: Option<BarrierElisionProof>,
    },
}

/// Closed backend-neutral reasons why a managed store needs no runtime barrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierElisionProof {
    UnpublishedOwner,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirTaskDispatch {
    Direct(SymbolId),
    Referenced(SymbolIdentity),
    Indirect(ValueId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirFrameSlot {
    pub(crate) value: ValueId,
    pub(crate) type_id: TypeId,
}

impl MirFrameSlot {
    #[must_use]
    pub const fn value(self) -> ValueId {
        self.value
    }

    #[must_use]
    pub const fn type_id(self) -> TypeId {
        self.type_id
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirLiveFrame {
    pub(crate) state: CoroutineStateId,
    pub(crate) slots: Vec<MirFrameSlot>,
    pub(crate) stack_map: StackMap,
}

impl MirLiveFrame {
    #[must_use]
    pub const fn state(&self) -> CoroutineStateId {
        self.state
    }

    #[must_use]
    pub fn slots(&self) -> &[MirFrameSlot] {
        &self.slots
    }

    #[must_use]
    pub const fn stack_map(&self) -> &StackMap {
        &self.stack_map
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirSuspendOperation {
    Task { task: ValueId, result_type: TypeId },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirCancellationMode {
    Observe,
    Masked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirClosureCapture {
    pub(crate) capture: CaptureId,
    pub(crate) binding: BindingId,
    pub(crate) slot: u32,
    pub(crate) value: ValueId,
    pub(crate) self_reference: bool,
    pub(crate) type_id: TypeId,
    pub(crate) mode: MirCaptureMode,
}

impl MirClosureCapture {
    #[must_use]
    pub const fn capture(self) -> CaptureId {
        self.capture
    }
    #[must_use]
    pub const fn binding(self) -> BindingId {
        self.binding
    }
    #[must_use]
    pub const fn slot(self) -> u32 {
        self.slot
    }
    #[must_use]
    pub const fn value(self) -> ValueId {
        self.value
    }
    #[must_use]
    pub const fn self_reference(self) -> bool {
        self.self_reference
    }
    #[must_use]
    pub const fn type_id(self) -> TypeId {
        self.type_id
    }
    #[must_use]
    pub const fn mode(self) -> MirCaptureMode {
        self.mode
    }
}

impl MirInstructionKind {
    #[must_use]
    pub fn possible_traps(&self) -> Vec<pop_runtime_interface::TrapKind> {
        use pop_runtime_interface::TrapKind;
        match self {
            Self::CheckedIntegerAdd { .. }
            | Self::CheckedIntegerSubtract { .. }
            | Self::CheckedIntegerMultiply { .. }
            | Self::IntegerNegate { .. } => vec![TrapKind::IntegerOverflow],
            Self::CheckedIntegerDivide { .. } | Self::CheckedIntegerRemainder { .. } => {
                vec![TrapKind::IntegerOverflow, TrapKind::DivisionByZero]
            }
            Self::ConvertInteger { source, target, .. }
                if pop_types::NumericConversionKind::IntegerToInteger {
                    source: *source,
                    target: *target,
                }
                .may_trap() =>
            {
                vec![TrapKind::NumericConversion]
            }
            Self::ConvertFloatToInteger { .. } => vec![TrapKind::NumericConversion],
            Self::ViewSlice { .. } => vec![TrapKind::BoundsViolation],
            _ => Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirTerminator {
    Missing,
    Branch {
        target: BlockId,
        arguments: Vec<ValueId>,
    },
    ConditionalBranch {
        condition: ValueId,
        when_true: BlockId,
        when_false: BlockId,
    },
    UnionSwitch {
        scrutinee: ValueId,
        union: SymbolId,
        arms: Vec<MirUnionSwitchArm>,
    },
    ErrorSwitch {
        scrutinee: ValueId,
        error: ErrorId,
        arms: Vec<MirErrorSwitchArm>,
    },
    CodecErrorSwitch {
        scrutinee: ValueId,
        arms: Vec<MirCodecErrorSwitchArm>,
    },
    Suspend {
        operation: MirSuspendOperation,
        resume: BlockId,
        cancellation: BlockId,
        cancellation_mode: MirCancellationMode,
        unwind: MirUnwindAction,
        safe_point: SafePointId,
        live_frame: MirLiveFrame,
    },
    Return {
        values: Vec<ValueId>,
    },
    Trap(Trap),
    Panic(PanicPayload),
    ContinueUnwind(UnwindReason),
    ResumeUnwind,
    Unreachable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirErrorSwitchArm {
    pub(crate) case: ErrorCaseId,
    pub(crate) target: BlockId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirCodecErrorSwitchArm {
    pub(crate) case: EnumCaseId,
    pub(crate) target: BlockId,
}

impl MirCodecErrorSwitchArm {
    #[must_use]
    pub const fn case(self) -> EnumCaseId {
        self.case
    }
    #[must_use]
    pub const fn target(self) -> BlockId {
        self.target
    }
}

impl MirErrorSwitchArm {
    #[must_use]
    pub const fn case(self) -> ErrorCaseId {
        self.case
    }
    #[must_use]
    pub const fn target(self) -> BlockId {
        self.target
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirUnionSwitchArm {
    pub(crate) case: UnionCaseId,
    pub(crate) target: BlockId,
}

impl MirUnionSwitchArm {
    #[must_use]
    pub const fn case(self) -> UnionCaseId {
        self.case
    }
    #[must_use]
    pub const fn target(self) -> BlockId {
        self.target
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirVerificationError {
    DuplicateFunction(SymbolId),
    InvalidCallableLifetimeSummary(SymbolId),
    InvalidForeignFunction(SymbolId),
    GenericSpecializationBudgetExceeded {
        limit: usize,
    },
    UnknownGenericTemplate(SymbolId),
    InvalidGenericSpecialization(SymbolId),
    MissingFfiLayoutFingerprint,
    InvalidFfiLayoutCatalog,
    InvalidType(TypeId),
    DuplicateValue(ValueId),
    UnknownValue(ValueId),
    ValueUsedBeforeDefinition(ValueId),
    ValueNotDominated {
        value: ValueId,
        definition: BlockId,
        use_block: BlockId,
    },
    InvalidBlock(BlockId),
    DuplicateBlock(BlockId),
    MissingTerminator(BlockId),
    EntryParameterArity {
        expected: usize,
        found: usize,
    },
    EntryParameterType {
        index: usize,
        expected: TypeId,
        found: TypeId,
    },
    EdgeArgumentArity {
        block: BlockId,
        target: BlockId,
        expected: usize,
        found: usize,
    },
    EdgeArgumentType {
        block: BlockId,
        target: BlockId,
        index: usize,
        expected: TypeId,
        found: TypeId,
    },
    ConditionalBranchConditionType {
        block: BlockId,
        found: TypeId,
    },
    WrongReturnArity {
        expected: usize,
        found: usize,
    },
    WrongReturnType {
        expected: TypeId,
        found: TypeId,
    },
    SuspendOutsideAsync(BlockId),
    InvalidSuspendTask(BlockId),
    InvalidSuspendResume(BlockId),
    InvalidSuspendCancellation(BlockId),
    InvalidSuspendCancellationMode(BlockId),
    InvalidSuspendFrame(BlockId),
    DuplicateSafePoint(SafePointId),
    DuplicateCoroutineState(CoroutineStateId),
    UnknownFunction(SymbolId),
    InvalidGeneratedCodecSchema(SymbolId),
    UnknownReferencedFunction(SymbolIdentity),
    UnknownMethod(MethodId),
    InvalidInstructionType {
        instruction: ValueId,
        result_type: TypeId,
    },
    OptionalGetWithoutPresence {
        instruction: ValueId,
        optional: ValueId,
    },
    WrongOperandType {
        instruction: ValueId,
        operand: ValueId,
        expected: TypeId,
        found: TypeId,
    },
    InvalidCollectionOperand {
        instruction: ValueId,
        operand: ValueId,
        found: TypeId,
    },
    InvalidCallableOperand {
        instruction: ValueId,
        operand: ValueId,
        found: TypeId,
    },
    InvalidCallSignature {
        instruction: ValueId,
        expected_arguments: usize,
        found_arguments: usize,
        expected_results: usize,
        found_results: usize,
    },
    InvalidForeignCall {
        instruction: ValueId,
        function: SymbolId,
    },
    InvalidForeignRoots {
        instruction: ValueId,
    },
    InvalidFfiHandleOperation {
        instruction: ValueId,
    },
    InvalidFfiBufferOperation {
        instruction: ValueId,
    },
    InvalidFfiBytesOperation {
        instruction: ValueId,
    },
    InvalidFfiCallbackOperation {
        instruction: ValueId,
    },
    InvalidFfiPointerOperation {
        instruction: ValueId,
    },
    InvalidFfiUnsafeOperation {
        instruction: ValueId,
    },
    InvalidFfiBufferBorrowRegion {
        region: BorrowRegionId,
    },
    InvalidTaskOperation {
        instruction: ValueId,
    },
    InvalidComparisonOperands {
        instruction: ValueId,
        left: TypeId,
        right: TypeId,
    },
    DuplicateDeclaration(SymbolId),
    DuplicateReferencedFunction(SymbolIdentity),
    DuplicateClass(ClassId),
    DuplicateDeclaredField(FieldId),
    DuplicateUnionCase {
        union: SymbolId,
        case: UnionCaseId,
    },
    DuplicateErrorCase {
        error: ErrorId,
        case: ErrorCaseId,
    },
    InvalidDeclarationType {
        symbol: SymbolId,
        type_id: TypeId,
    },
    UnknownRecord {
        instruction: ValueId,
        record: SymbolId,
    },
    UnknownClass {
        instruction: ValueId,
        class: ClassId,
    },
    UnknownUnion {
        instruction: ValueId,
        union: SymbolId,
    },
    UnknownField {
        instruction: ValueId,
        field: FieldId,
    },
    UnknownUnionCase {
        instruction: ValueId,
        union: SymbolId,
        case: UnionCaseId,
    },
    InvalidResultOperation {
        instruction: ValueId,
    },
    InvalidIterationOperation {
        instruction: ValueId,
    },
    InvalidBuiltinInterfaceImplementation {
        class: ClassId,
        interface: BuiltinTypeId,
    },
    InvalidInterfaceImplementation {
        class: ClassId,
        interface: InterfaceId,
    },
    InvalidInterfaceUpcast {
        instruction: ValueId,
        interface: NominalInterfaceId,
        source: TypeId,
        target: TypeId,
    },
    InvalidCheckedDowncast {
        instruction: ValueId,
        source_interface: InterfaceId,
        source: TypeId,
        target_class: ClassId,
        target: TypeId,
        result: TypeId,
    },
    InvalidNominalReference(SymbolIdentity),
    InvalidViewOperation {
        instruction: ValueId,
    },
    InvalidViewLifetime {
        lifetime: LifetimeId,
    },
    InvalidViewEscape {
        value: ValueId,
    },
    InvalidViewRoot {
        lifetime: LifetimeId,
        lender: ValueId,
    },
    InvalidClassAncestry {
        class: ClassId,
        base: Option<ClassId>,
    },
    InvalidErrorOperation {
        instruction: ValueId,
        error: ErrorId,
    },
    InvalidUnionSwitch {
        union: SymbolId,
    },
    InvalidErrorSwitch {
        error: ErrorId,
    },
    InvalidCodecErrorSwitch,
    UnknownInterface(InterfaceId),
    UnknownInterfaceMethod(InterfaceMethodId),
    UnknownStandardFunction(StandardFunctionId),
    WrongInterfaceMethodSlot {
        method: InterfaceMethodId,
        expected: u32,
        found: u32,
    },
    MissingDeclaredField {
        instruction: ValueId,
        field: FieldId,
    },
    WrongFieldOwner {
        instruction: ValueId,
        field: FieldId,
        expected: TypeId,
        found: TypeId,
    },
    ImmutableFieldSet {
        instruction: ValueId,
        field: FieldId,
    },
    FunctionEffectMismatch {
        function: SymbolId,
        expected: MirEffectSummary,
        found: MirEffectSummary,
    },
    InstructionEffectMismatch {
        instruction: ValueId,
        expected: MirEffectSummary,
        found: MirEffectSummary,
    },
    IncompleteStackMap {
        instruction: ValueId,
        expected: usize,
        found: usize,
    },
    InvalidStackMapRoot {
        instruction: ValueId,
        root: ValueId,
    },
    DuplicateRetain {
        instruction: ValueId,
        value: ValueId,
    },
    ReleaseWithoutRetain {
        instruction: ValueId,
        value: ValueId,
    },
    UnreleasedRoot {
        block: BlockId,
        value: ValueId,
    },
    RootStateMismatch(BlockId),
    InvalidPinnedReference {
        instruction: ValueId,
        value: ValueId,
    },
    UnpinWithoutPin {
        instruction: ValueId,
        value: ValueId,
    },
    UnreleasedPin {
        block: BlockId,
        value: ValueId,
    },
    PinStateMismatch(BlockId),
    InvalidObjectMap {
        instruction: ValueId,
    },
    MissingWriteBarrier {
        instruction: ValueId,
        field: FieldId,
    },
    UnexpectedWriteBarrier {
        instruction: ValueId,
    },
    InvalidBarrierElisionProof {
        instruction: ValueId,
        proof: BarrierElisionProof,
    },
    InvalidUnwindAction {
        instruction: ValueId,
    },
    ResumeOutsideCleanup {
        block: BlockId,
    },
    InvalidCleanupBlock {
        block: BlockId,
    },
    MissingGcSafePoint {
        instruction: ValueId,
    },
    MissingBackedgeSafePoint(BlockId),
}
