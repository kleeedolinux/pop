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
    AttributeIdentity, BYTES_TYPE_ID, BYTES_VIEW_TYPE_ID, BootstrapCodecErrorProtocol,
    BootstrapCompilerAttributeEntry, BootstrapIntrinsicEntry, BootstrapIterationProtocol,
    BootstrapPrimitiveEntry, BootstrapSchema, BootstrapSchemaError, BootstrapStandardFunctionEntry,
    BootstrapTypeEntry, BootstrapTypeRole, CODEC_ERROR_TYPE_ID, CodecErrorReason,
    CompilerAttributeId, CompilerAttributeRole, CompilerAttributeTarget,
    FFI_ALLOCATION_ERROR_TYPE_ID, FFI_BUFFER_TYPE_ID, FFI_CALLBACK_CLOSED_ERROR_TYPE_ID,
    FFI_CALLBACK_CONTEXT_TYPE_ID, FFI_CALLBACK_IN_USE_ERROR_TYPE_ID,
    FFI_CALLBACK_OPEN_ERROR_TYPE_ID, FFI_CALLBACK_THREAD_TYPE_ID, FFI_FUNCTION_TYPE_ID,
    FFI_HANDLE_TYPE_ID, FFI_NULL_POINTER_ERROR_TYPE_ID, FFI_OPTIONAL_POINTER_TYPE_ID,
    FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID, FFI_POINTER_TYPE_ID, FFI_READ_ONLY_POINTER_TYPE_ID,
    FFI_REGISTERED_CALLBACK_TYPE_ID, FfiCIntegerKind, TEXT_VIEW_TYPE_ID, embedded_bootstrap_schema,
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
        lifetime_summary: CallableLifetimeSummary,
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

/// Closed structural identity used by runtime nominal descriptors across Bubbles.
///
/// Unlike [`TypeId`], every component is stable across independent compilation.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum CanonicalTypeIdentity {
    Primitive(PrimitiveType),
    Record(pop_foundation::SymbolIdentity),
    Class(CanonicalNominalIdentity),
    Interface(CanonicalNominalIdentity),
    Tuple(Vec<Self>),
    Function {
        is_async: bool,
        parameters: Vec<Self>,
        results: Vec<Self>,
        effects: EffectSummary,
        lifetime_summary: CallableLifetimeSummary,
    },
    Array(Box<Self>),
    Table {
        key: Box<Self>,
        value: Box<Self>,
    },
    Optional(Box<Self>),
    Builtin {
        definition: BuiltinTypeId,
        arguments: Vec<Self>,
    },
    Union(Vec<Self>),
}

/// Exact stable nominal definition plus its fully applied canonical arguments.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CanonicalNominalIdentity {
    definition: pop_foundation::SymbolIdentity,
    arguments: Vec<CanonicalTypeIdentity>,
}

impl CanonicalNominalIdentity {
    #[must_use]
    pub fn new(
        definition: pop_foundation::SymbolIdentity,
        arguments: Vec<CanonicalTypeIdentity>,
    ) -> Self {
        Self {
            definition,
            arguments,
        }
    }

    #[must_use]
    pub const fn definition(&self) -> pop_foundation::SymbolIdentity {
        self.definition
    }

    #[must_use]
    pub fn arguments(&self) -> &[CanonicalTypeIdentity] {
        &self.arguments
    }

    #[must_use]
    pub fn descriptor(&self) -> String {
        let mut descriptor = format!(
            "b{}:s{}[",
            self.definition.bubble().raw(),
            self.definition.symbol().raw()
        );
        write_canonical_type_list(&mut descriptor, &self.arguments);
        descriptor.push(']');
        descriptor
    }
}

impl CanonicalTypeIdentity {
    #[must_use]
    pub fn descriptor(&self) -> String {
        let mut descriptor = String::new();
        write_canonical_type(&mut descriptor, self);
        descriptor
    }
}

fn write_canonical_type(output: &mut String, identity: &CanonicalTypeIdentity) {
    use std::fmt::Write as _;
    match identity {
        CanonicalTypeIdentity::Primitive(primitive) => {
            let _ = write!(output, "P({primitive:?})");
        }
        CanonicalTypeIdentity::Record(identity) => {
            let _ = write!(
                output,
                "R(b{}:s{})",
                identity.bubble().raw(),
                identity.symbol().raw()
            );
        }
        CanonicalTypeIdentity::Class(identity) => {
            output.push('C');
            output.push('(');
            output.push_str(&identity.descriptor());
            output.push(')');
        }
        CanonicalTypeIdentity::Interface(identity) => {
            output.push('I');
            output.push('(');
            output.push_str(&identity.descriptor());
            output.push(')');
        }
        CanonicalTypeIdentity::Tuple(elements) => {
            output.push_str("T[");
            write_canonical_type_list(output, elements);
            output.push(']');
        }
        CanonicalTypeIdentity::Function {
            is_async,
            parameters,
            results,
            effects,
            lifetime_summary,
        } => {
            let _ = write!(output, "F({};[", u8::from(*is_async));
            write_canonical_type_list(output, parameters);
            output.push_str("];[");
            write_canonical_type_list(output, results);
            let _ = write!(
                output,
                "];E{};L{}:[",
                effects.bits(),
                lifetime_summary.proof_version()
            );
            for (index, retention) in lifetime_summary.parameter_retention().iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                match retention {
                    ParameterRetention::DoesNotRetain => output.push('D'),
                    ParameterRetention::MayRetain => output.push('M'),
                    ParameterRetention::StoresInto(target) => {
                        let _ = write!(output, "S{target}");
                    }
                    ParameterRetention::Captures => output.push('C'),
                    ParameterRetention::Publishes => output.push('P'),
                }
            }
            output.push_str("]:[");
            for (index, provenance) in lifetime_summary.result_provenance().iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                match provenance {
                    ResultProvenance::Independent => output.push('I'),
                    ResultProvenance::ReturnsAlias(source) => {
                        let _ = write!(output, "R{source}");
                    }
                    ResultProvenance::MayAlias => output.push('M'),
                }
            }
            output.push_str("])");
        }
        CanonicalTypeIdentity::Array(element) => {
            output.push_str("A(");
            write_canonical_type(output, element);
            output.push(')');
        }
        CanonicalTypeIdentity::Table { key, value } => {
            output.push_str("M(");
            write_canonical_type(output, key);
            output.push(';');
            write_canonical_type(output, value);
            output.push(')');
        }
        CanonicalTypeIdentity::Optional(element) => {
            output.push_str("O(");
            write_canonical_type(output, element);
            output.push(')');
        }
        CanonicalTypeIdentity::Builtin {
            definition,
            arguments,
        } => {
            let _ = write!(output, "B{}[", definition.raw());
            write_canonical_type_list(output, arguments);
            output.push(']');
        }
        CanonicalTypeIdentity::Union(elements) => {
            output.push_str("U[");
            write_canonical_type_list(output, elements);
            output.push(']');
        }
    }
}

fn write_canonical_type_list(output: &mut String, identities: &[CanonicalTypeIdentity]) {
    for (index, identity) in identities.iter().enumerate() {
        if index != 0 {
            output.push(',');
        }
        write_canonical_type(output, identity);
    }
}

pub const CALLABLE_LIFETIME_PROOF_VERSION: u16 = 1;

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ParameterRetention {
    DoesNotRetain,
    MayRetain,
    StoresInto(u16),
    Captures,
    Publishes,
}

#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ResultProvenance {
    Independent,
    ReturnsAlias(u16),
    MayAlias,
}

/// Closed, versioned retention and result-alias facts carried by every callable type.
#[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct CallableLifetimeSummary {
    proof_version: u16,
    parameter_retention: Vec<ParameterRetention>,
    result_provenance: Vec<ResultProvenance>,
}

impl CallableLifetimeSummary {
    /// Constructs one summary only when its proof version and indexed facts
    /// form a complete canonical callable contract.
    #[must_use]
    pub fn from_parts(
        proof_version: u16,
        parameter_retention: Vec<ParameterRetention>,
        result_provenance: Vec<ResultProvenance>,
    ) -> Option<Self> {
        let summary = Self {
            proof_version,
            parameter_retention,
            result_provenance,
        };
        summary
            .is_canonical_for(
                summary.parameter_retention.len(),
                summary.result_provenance.len(),
            )
            .then_some(summary)
    }

    #[must_use]
    pub fn conservative(parameter_count: usize, result_count: usize) -> Self {
        Self {
            proof_version: CALLABLE_LIFETIME_PROOF_VERSION,
            parameter_retention: vec![ParameterRetention::MayRetain; parameter_count],
            result_provenance: vec![ResultProvenance::MayAlias; result_count],
        }
    }

    #[must_use]
    pub fn non_retaining(parameter_count: usize, result_count: usize) -> Self {
        Self {
            proof_version: CALLABLE_LIFETIME_PROOF_VERSION,
            parameter_retention: vec![ParameterRetention::DoesNotRetain; parameter_count],
            result_provenance: vec![ResultProvenance::Independent; result_count],
        }
    }

    #[must_use]
    pub fn with_result_alias(mut self, result: usize, source_parameter: u16) -> Self {
        if let Some(provenance) = self.result_provenance.get_mut(result) {
            *provenance = ResultProvenance::ReturnsAlias(source_parameter);
        }
        self
    }

    #[must_use]
    pub fn with_parameter_retention(
        mut self,
        parameter: usize,
        retention: ParameterRetention,
    ) -> Self {
        if let Some(current) = self.parameter_retention.get_mut(parameter) {
            *current = retention;
        }
        self
    }

    #[must_use]
    pub fn with_result_provenance(mut self, result: usize, provenance: ResultProvenance) -> Self {
        if let Some(current) = self.result_provenance.get_mut(result) {
            *current = provenance;
        }
        self
    }

    #[must_use]
    pub const fn proof_version(&self) -> u16 {
        self.proof_version
    }

    #[must_use]
    pub fn parameter_retention(&self) -> &[ParameterRetention] {
        &self.parameter_retention
    }

    #[must_use]
    pub fn result_provenance(&self) -> &[ResultProvenance] {
        &self.result_provenance
    }

    #[must_use]
    pub fn is_canonical_for(&self, parameter_count: usize, result_count: usize) -> bool {
        self.proof_version == CALLABLE_LIFETIME_PROOF_VERSION
            && self.parameter_retention.len() == parameter_count
            && self.result_provenance.len() == result_count
            && self
                .parameter_retention
                .iter()
                .all(|retention| match retention {
                    ParameterRetention::StoresInto(target) => {
                        usize::from(*target) < parameter_count
                    }
                    ParameterRetention::DoesNotRetain
                    | ParameterRetention::MayRetain
                    | ParameterRetention::Captures
                    | ParameterRetention::Publishes => true,
                })
            && self
                .result_provenance
                .iter()
                .all(|provenance| match provenance {
                    ResultProvenance::ReturnsAlias(source) => {
                        usize::from(*source) < parameter_count
                    }
                    ResultProvenance::Independent | ResultProvenance::MayAlias => true,
                })
    }
}

/// The only public non-owning value families accepted by ADR 0093.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ViewKind {
    Bytes,
    Text,
}

impl ViewKind {
    #[must_use]
    pub const fn type_definition(self) -> BuiltinTypeId {
        match self {
            Self::Bytes => BYTES_VIEW_TYPE_ID,
            Self::Text => TEXT_VIEW_TYPE_ID,
        }
    }
}

/// Stable provenance of the immutable storage designated by a view.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub enum ViewLenderProvenance {
    Allocation {
        site: pop_foundation::AllocationSiteId,
    },
    Parameter {
        index: u32,
    },
    Constant {
        fingerprint: [u8; 32],
    },
}

/// One compiler-proven view value's lender and non-lexical borrow identity.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
pub struct ViewBorrow {
    lender: ViewLenderProvenance,
    lifetime: pop_foundation::LifetimeId,
}

impl ViewBorrow {
    #[must_use]
    pub const fn new(lender: ViewLenderProvenance, lifetime: pop_foundation::LifetimeId) -> Self {
        Self { lender, lifetime }
    }

    #[must_use]
    pub const fn lender(self) -> ViewLenderProvenance {
        self.lender
    }

    #[must_use]
    pub const fn lifetime(self) -> pop_foundation::LifetimeId {
        self.lifetime
    }
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
    #[must_use]
    pub const fn bits(self) -> u16 {
        self.0
    }

    #[must_use]
    pub const fn from_bits(bits: u16) -> Option<Self> {
        let supported = (1_u16 << 13) - 1;
        if bits & !supported == 0 {
            Some(Self(bits))
        } else {
            None
        }
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
