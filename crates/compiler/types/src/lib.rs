//! Static types, constraints, inference, and normalization.
//!
//! This first contract encodes the accepted primitive and nominal type model.
//! It deliberately has no operational unknown or dynamic fallback type.

// The type checker predates the repository-wide Rust 1.96 clippy gate. Keep
// the baseline explicit until these large modules are split deliberately.
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    clippy::elidable_lifetime_names,
    clippy::format_collect,
    clippy::match_same_arms,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::question_mark,
    clippy::redundant_closure_for_method_calls,
    clippy::single_match_else,
    clippy::too_many_lines,
    clippy::unnecessary_wraps,
    clippy::wildcard_imports
)]

use pop_foundation::{
    AttributeId, BuiltinTypeId, ClassId, InterfaceId, OpaqueId, ParameterId, SourceSpan, SymbolId,
    TypeId,
};
use serde::{Deserialize, Serialize};

mod aggregate_checking;
mod arena;
mod attributes;
mod body_checking;
mod bootstrap;
mod call_checking;
mod capture_analysis;
mod classes;
mod field_defaults;
mod inference;
mod interfaces;
mod numeric;
mod operator_checking;
mod required_constants;
mod signature_resolution;
mod statement_checking;
mod typed_body;

pub use arena::{TypeArena, TypeArenaError};
pub use attributes::{
    AttributeAttachmentError, AttributeAttachmentResult, AttributeAttachmentSet, AttributeConstant,
    AttributeContractError, AttributeDefinition, AttributeDefinitionResult,
    AttributeParameterDefinition, AttributeQueryError, AttributeQueryIndex,
    AttributeQueryIndexError, AttributeQuerySubject, AttributeQueryValue, AttributeTarget,
    AttributeUsage, AttributeValidator, ResolvedAttribute, ResolvedAttributeArgument,
    ResolvedAttributeResult,
};
pub use body_checking::{BodyChecker, RuntimeConstant};
pub use bootstrap::{
    AttributeIdentity, BootstrapCompilerAttributeEntry, BootstrapIntrinsicEntry,
    BootstrapIterationProtocol, BootstrapPrimitiveEntry, BootstrapSchema, BootstrapSchemaError,
    BootstrapStandardFunctionEntry, BootstrapTypeEntry, BootstrapTypeRole, CompilerAttributeId,
    CompilerAttributeRole, CompilerAttributeTarget, FFI_ALLOCATION_ERROR_TYPE_ID,
    FFI_BUFFER_TYPE_ID, FFI_CALLBACK_CLOSED_ERROR_TYPE_ID, FFI_CALLBACK_CONTEXT_TYPE_ID,
    FFI_CALLBACK_IN_USE_ERROR_TYPE_ID, FFI_CALLBACK_OPEN_ERROR_TYPE_ID,
    FFI_CALLBACK_THREAD_TYPE_ID, FFI_FUNCTION_TYPE_ID, FFI_HANDLE_TYPE_ID,
    FFI_NULL_POINTER_ERROR_TYPE_ID, FFI_OPTIONAL_POINTER_TYPE_ID,
    FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID, FFI_POINTER_TYPE_ID, FFI_READ_ONLY_POINTER_TYPE_ID,
    FFI_REGISTERED_CALLBACK_TYPE_ID, FfiCIntegerKind, embedded_bootstrap_schema,
    ffi_c_integer_kind, is_ffi_abi_builtin_type, is_ffi_function_type_constructor,
    is_ffi_integer_abi_builtin_type, is_ffi_pointer_type_constructor,
};
pub use classes::{
    ClassDefinition, ClassDefinitionResult, ClassFieldDefinition, ClassMethodDefinition,
    ClassMethodDispatch,
};
pub use field_defaults::FieldDefault;
pub use inference::{InferenceContext, InferenceError, InferenceType, InferenceVariableId};
pub use interfaces::{
    BuiltinInterfaceMethodImplementation, ClassBuiltinInterfaceImplementation,
    ClassInterfaceImplementation, InterfaceDefinition, InterfaceDefinitionResult,
    InterfaceMethodDefinition, InterfaceMethodImplementation,
};
pub use numeric::{FloatKind, FloatValue, IntegerValue, NumericConversionKind, NumericError};
pub use required_constants::{
    AttributeParameterId, PendingConstantExpression, RequiredConstantError, RequiredConstantTarget,
};
pub use signature_resolution::{
    EnumCaseDefinition, EnumDefinition, EnumDefinitionResult, ErrorCaseDefinition, ErrorDefinition,
    ErrorDefinitionResult, RecordDefinition, RecordDefinitionResult, RecordFieldDefinition,
    ResolvedFunctionParameter, ResolvedFunctionSignature, ResolvedSignatureResult, ResolvedType,
    ResolvedTypeKind, ResolvedTypeParameter, SignatureResolver, UnionCaseDefinition,
    UnionDefinition, UnionDefinitionResult,
};
pub use typed_body::{
    CaptureMode, CaptureSource, StringFormatKind, TypedAssignmentTarget, TypedBinaryOperator,
    TypedBody, TypedBodyResult, TypedCall, TypedCallDispatch, TypedCapture, TypedClosure,
    TypedClosureParameter, TypedCompoundOperator, TypedErrorMatchArm, TypedExpression,
    TypedExpressionKind, TypedExpressionResult, TypedFieldValue, TypedIterationSource,
    TypedMatchArm, TypedMatchBinding, TypedResultMatchArm, TypedStatement, TypedStatementKind,
    TypedTableEntry, TypedUnaryOperator,
};

pub type ClassFieldDefault = FieldDefault;
pub type RecordFieldDefault = FieldDefault;

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum IntegerKind {
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
}

impl IntegerKind {
    #[must_use]
    pub const fn bit_width(self) -> u16 {
        match self {
            Self::Int8 | Self::UInt8 => 8,
            Self::Int16 | Self::UInt16 => 16,
            Self::Int32 | Self::UInt32 => 32,
            Self::Int64 | Self::UInt64 => 64,
        }
    }

    #[must_use]
    pub const fn is_signed(self) -> bool {
        matches!(self, Self::Int8 | Self::Int16 | Self::Int32 | Self::Int64)
    }

    #[must_use]
    pub const fn default_overflow(self) -> IntegerOverflow {
        let _ = self;
        IntegerOverflow::Trap
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum IntegerOverflow {
    Trap,
    WrapExplicitly,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum PrimitiveType {
    Nil,
    Boolean,
    Integer(IntegerKind),
    Float32,
    Float64,
    String,
    Never,
}

impl PrimitiveType {
    #[must_use]
    pub fn from_source_name(name: &str) -> Option<Self> {
        Self::source_schema()
            .iter()
            .find(|entry| entry.source_name == name)
            .map(|entry| entry.primitive)
    }

    #[must_use]
    pub const fn source_schema() -> &'static [PrimitiveSchemaEntry] {
        &PRIMITIVE_SCHEMA
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrimitiveSchemaEntry {
    source_name: &'static str,
    canonical_name: &'static str,
    primitive: PrimitiveType,
}

impl PrimitiveSchemaEntry {
    #[must_use]
    pub const fn source_name(self) -> &'static str {
        self.source_name
    }

    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        self.canonical_name
    }

    #[must_use]
    pub const fn primitive(self) -> PrimitiveType {
        self.primitive
    }

    #[must_use]
    pub fn is_alias(self) -> bool {
        self.source_name != self.canonical_name
    }
}

const PRIMITIVE_SCHEMA: [PrimitiveSchemaEntry; 17] = [
    primitive("nil", "nil", PrimitiveType::Nil),
    primitive("Boolean", "Boolean", PrimitiveType::Boolean),
    primitive("Int8", "Int8", PrimitiveType::Integer(IntegerKind::Int8)),
    primitive("Int16", "Int16", PrimitiveType::Integer(IntegerKind::Int16)),
    primitive("Int32", "Int32", PrimitiveType::Integer(IntegerKind::Int32)),
    primitive("Int64", "Int64", PrimitiveType::Integer(IntegerKind::Int64)),
    primitive("UInt8", "UInt8", PrimitiveType::Integer(IntegerKind::UInt8)),
    primitive(
        "UInt16",
        "UInt16",
        PrimitiveType::Integer(IntegerKind::UInt16),
    ),
    primitive(
        "UInt32",
        "UInt32",
        PrimitiveType::Integer(IntegerKind::UInt32),
    ),
    primitive(
        "UInt64",
        "UInt64",
        PrimitiveType::Integer(IntegerKind::UInt64),
    ),
    primitive("Int", "Int64", PrimitiveType::Integer(IntegerKind::Int64)),
    primitive("Float32", "Float32", PrimitiveType::Float32),
    primitive("Float64", "Float64", PrimitiveType::Float64),
    primitive("Float", "Float64", PrimitiveType::Float64),
    primitive("Byte", "UInt8", PrimitiveType::Integer(IntegerKind::UInt8)),
    primitive("String", "String", PrimitiveType::String),
    primitive("Never", "Never", PrimitiveType::Never),
];

const fn primitive(
    source_name: &'static str,
    canonical_name: &'static str,
    primitive: PrimitiveType,
) -> PrimitiveSchemaEntry {
    PrimitiveSchemaEntry {
        source_name,
        canonical_name,
        primitive,
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum SemanticType {
    Primitive(PrimitiveType),
    Tuple(Vec<TypeId>),
    Function {
        is_async: bool,
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        effects: EffectSummary,
    },
    Record(Vec<(String, TypeId)>),
    TaggedUnion {
        definition: pop_foundation::SymbolId,
        source: pop_foundation::SymbolId,
        arguments: Vec<TypeId>,
    },
    ErrorUnion {
        definition: pop_foundation::ErrorId,
        source: pop_foundation::SymbolId,
        arguments: Vec<TypeId>,
    },
    Enum {
        definition: pop_foundation::SymbolId,
    },
    Array(TypeId),
    Table {
        key: TypeId,
        value: TypeId,
    },
    Class {
        class: ClassId,
        arguments: Vec<TypeId>,
    },
    Interface {
        interface: InterfaceId,
        arguments: Vec<TypeId>,
    },
    /// A nominal compile-time-only user-defined attribute value.
    Attribute {
        attribute: AttributeId,
        parameters: Vec<TypeId>,
    },
    Builtin {
        definition: BuiltinTypeId,
        arguments: Vec<TypeId>,
    },
    Union(Vec<TypeId>),
    Optional(TypeId),
    TypeParameter(ParameterId),
    Opaque(OpaqueId),
    Error,
}

#[derive(
    Clone, Copy, Debug, Default, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize,
)]
pub struct EffectSummary(u16);

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum Effect {
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

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ForeignAbi {
    C,
    System,
    CUnwind,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum FfiCallbackThreadPolicy {
    CallingThread,
    AttachedThread,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum FfiCallbackLifetime {
    CallScoped,
    Registered,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum FfiCallbackAbi {
    C,
    System,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum FfiCallbackConcurrency {
    Serialized,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum FfiCallbackReentrancy {
    Forbidden,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum FfiCallbackPanicPolicy {
    AbortProcess,
}

/// The closed generated contract selected by one lexical callback-pair scope.
///
/// Foreign parameter indices remain on [`FfiCallbackPairContract`]; this value
/// contains only the facts that must agree across every exact pair use in the
/// scope and that select one backend-emitted typed thunk.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FfiCallbackBindingContract {
    lifetime: FfiCallbackLifetime,
    callback_abi: FfiCallbackAbi,
    signature_fingerprint: String,
    thread: FfiCallbackThreadPolicy,
    concurrency: FfiCallbackConcurrency,
    reentrancy: FfiCallbackReentrancy,
    panic_policy: FfiCallbackPanicPolicy,
}

impl FfiCallbackBindingContract {
    #[must_use]
    pub const fn lifetime(&self) -> FfiCallbackLifetime {
        self.lifetime
    }

    #[must_use]
    pub const fn callback_abi(&self) -> FfiCallbackAbi {
        self.callback_abi
    }

    #[must_use]
    pub fn signature_fingerprint(&self) -> &str {
        &self.signature_fingerprint
    }

    #[must_use]
    pub const fn thread(&self) -> FfiCallbackThreadPolicy {
        self.thread
    }

    #[must_use]
    pub const fn concurrency(&self) -> FfiCallbackConcurrency {
        self.concurrency
    }

    #[must_use]
    pub const fn reentrancy(&self) -> FfiCallbackReentrancy {
        self.reentrancy
    }

    #[must_use]
    pub const fn panic_policy(&self) -> FfiCallbackPanicPolicy {
        self.panic_policy
    }

    #[must_use]
    pub fn has_valid_shape(&self) -> bool {
        self.signature_fingerprint.len() == 64
            && self
                .signature_fingerprint
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            && matches!(
                (self.lifetime, self.thread),
                (
                    FfiCallbackLifetime::CallScoped,
                    FfiCallbackThreadPolicy::CallingThread
                ) | (
                    FfiCallbackLifetime::Registered,
                    FfiCallbackThreadPolicy::AttachedThread
                )
            )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct FfiCallbackPairContract {
    callback_parameter_index: u16,
    context_parameter_index: u16,
    lifetime: FfiCallbackLifetime,
    callback_abi: FfiCallbackAbi,
    signature_fingerprint: String,
    thread: FfiCallbackThreadPolicy,
    concurrency: FfiCallbackConcurrency,
    reentrancy: FfiCallbackReentrancy,
    panic_policy: FfiCallbackPanicPolicy,
}

impl FfiCallbackPairContract {
    #[must_use]
    pub fn new(
        callback_parameter_index: u16,
        context_parameter_index: u16,
        lifetime: FfiCallbackLifetime,
        callback_abi: FfiCallbackAbi,
        signature_fingerprint: impl Into<String>,
        thread: FfiCallbackThreadPolicy,
    ) -> Self {
        Self {
            callback_parameter_index,
            context_parameter_index,
            lifetime,
            callback_abi,
            signature_fingerprint: signature_fingerprint.into(),
            thread,
            concurrency: FfiCallbackConcurrency::Serialized,
            reentrancy: FfiCallbackReentrancy::Forbidden,
            panic_policy: FfiCallbackPanicPolicy::AbortProcess,
        }
    }

    #[must_use]
    pub const fn callback_parameter_index(&self) -> u16 {
        self.callback_parameter_index
    }

    #[must_use]
    pub const fn context_parameter_index(&self) -> u16 {
        self.context_parameter_index
    }

    #[must_use]
    pub const fn lifetime(&self) -> FfiCallbackLifetime {
        self.lifetime
    }

    #[must_use]
    pub const fn callback_abi(&self) -> FfiCallbackAbi {
        self.callback_abi
    }

    #[must_use]
    pub fn signature_fingerprint(&self) -> &str {
        &self.signature_fingerprint
    }

    #[must_use]
    pub const fn thread(&self) -> FfiCallbackThreadPolicy {
        self.thread
    }

    #[must_use]
    pub const fn concurrency(&self) -> FfiCallbackConcurrency {
        self.concurrency
    }

    #[must_use]
    pub const fn reentrancy(&self) -> FfiCallbackReentrancy {
        self.reentrancy
    }

    #[must_use]
    pub const fn panic_policy(&self) -> FfiCallbackPanicPolicy {
        self.panic_policy
    }

    #[must_use]
    pub fn binding_contract(&self) -> FfiCallbackBindingContract {
        FfiCallbackBindingContract {
            lifetime: self.lifetime,
            callback_abi: self.callback_abi,
            signature_fingerprint: self.signature_fingerprint.clone(),
            thread: self.thread,
            concurrency: self.concurrency,
            reentrancy: self.reentrancy,
            panic_policy: self.panic_policy,
        }
    }

    #[must_use]
    pub fn has_valid_shape(&self) -> bool {
        self.callback_parameter_index != self.context_parameter_index
            && self.binding_contract().has_valid_shape()
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ForeignFunctionDeclaration {
    symbol: SymbolId,
    external_symbol: String,
    abi: ForeignAbi,
    link_aliases: Vec<String>,
    nonblocking: bool,
    effects: EffectSummary,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    callback_pairs: Vec<FfiCallbackPairContract>,
    span: SourceSpan,
}

impl ForeignFunctionDeclaration {
    #[must_use]
    pub fn new(
        symbol: SymbolId,
        external_symbol: impl Into<String>,
        abi: ForeignAbi,
        mut link_aliases: Vec<String>,
        nonblocking: bool,
        span: SourceSpan,
    ) -> Self {
        link_aliases.sort();
        link_aliases.dedup();
        let effects = Self::expected_effects(abi, nonblocking);
        Self {
            symbol,
            external_symbol: external_symbol.into(),
            abi,
            link_aliases,
            nonblocking,
            effects,
            callback_pairs: Vec::new(),
            span,
        }
    }

    #[must_use]
    pub fn with_callback_pairs(mut self, mut callback_pairs: Vec<FfiCallbackPairContract>) -> Self {
        callback_pairs.sort_by_key(FfiCallbackPairContract::callback_parameter_index);
        self.callback_pairs = callback_pairs;
        self
    }

    const fn expected_effects(abi: ForeignAbi, nonblocking: bool) -> EffectSummary {
        let mut effects = EffectSummary::empty()
            .with(Effect::ForeignFunction)
            .with(Effect::UnsafeMemory)
            .with(Effect::GcSafePoint);
        if !nonblocking {
            effects = effects.with(Effect::Blocks);
        }
        if let ForeignAbi::CUnwind = abi {
            effects = effects.with(Effect::MayUnwind);
        }
        effects
    }

    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }

    #[must_use]
    pub fn external_symbol(&self) -> &str {
        &self.external_symbol
    }

    #[must_use]
    pub const fn abi(&self) -> ForeignAbi {
        self.abi
    }

    #[must_use]
    pub fn link_aliases(&self) -> &[String] {
        &self.link_aliases
    }

    #[must_use]
    pub const fn is_nonblocking(&self) -> bool {
        self.nonblocking
    }

    #[must_use]
    pub fn has_valid_effects(&self) -> bool {
        self.effects == Self::expected_effects(self.abi, self.nonblocking)
    }

    #[must_use]
    pub const fn effects(&self) -> EffectSummary {
        self.effects
    }

    #[must_use]
    pub fn callback_pairs(&self) -> &[FfiCallbackPairContract] {
        &self.callback_pairs
    }

    #[must_use]
    pub fn has_valid_callback_pairs(&self) -> bool {
        let mut callback_indices = std::collections::BTreeSet::new();
        let mut context_indices = std::collections::BTreeSet::new();
        let mut previous = None;
        self.callback_pairs.iter().all(|pair| {
            let callback = pair.callback_parameter_index();
            let context = pair.context_parameter_index();
            let sorted = previous.is_none_or(|previous| previous < callback);
            previous = Some(callback);
            pair.has_valid_shape()
                && sorted
                && !self.nonblocking
                && callback_indices.insert(callback)
                && context_indices.insert(context)
                && !callback_indices.contains(&context)
                && !context_indices.contains(&callback)
        })
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

impl Effect {
    const fn bit(self) -> u16 {
        1_u16 << self as u16
    }
}

impl EffectSummary {
    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }
    #[must_use]
    pub const fn with(self, effect: Effect) -> Self {
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
    pub const fn contains(self, effect: Effect) -> bool {
        self.0 & effect.bit() != 0
    }
}

impl SemanticType {
    #[must_use]
    pub const fn is_valid_hir_type(&self) -> bool {
        !matches!(self, Self::Attribute { .. } | Self::Error)
    }

    #[must_use]
    pub const fn is_valid_compile_time_type(&self) -> bool {
        !matches!(self, Self::Error)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassExtensibility {
    Sealed,
    Open,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassContract {
    class: ClassId,
    base: Option<ClassId>,
    interfaces: Vec<InterfaceId>,
    extensibility: ClassExtensibility,
}

impl ClassContract {
    #[must_use]
    pub fn new(class: ClassId, base: Option<ClassId>, mut interfaces: Vec<InterfaceId>) -> Self {
        interfaces.sort_unstable();
        interfaces.dedup();
        Self {
            class,
            base,
            interfaces,
            extensibility: ClassExtensibility::Sealed,
        }
    }

    #[must_use]
    pub const fn open(mut self) -> Self {
        self.extensibility = ClassExtensibility::Open;
        self
    }

    #[must_use]
    pub const fn class(&self) -> ClassId {
        self.class
    }

    #[must_use]
    pub const fn base(&self) -> Option<ClassId> {
        self.base
    }

    #[must_use]
    pub fn interfaces(&self) -> &[InterfaceId] {
        &self.interfaces
    }

    #[must_use]
    pub const fn extensibility(&self) -> ClassExtensibility {
        self.extensibility
    }
}
