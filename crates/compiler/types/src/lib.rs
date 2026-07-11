//! Static types, constraints, inference, and normalization.
//!
//! This first contract encodes the accepted primitive and nominal type model.
//! It deliberately has no operational unknown or dynamic fallback type.

use pop_foundation::{
    AttributeId, BuiltinTypeId, ClassId, InterfaceId, OpaqueId, ParameterId, TypeId,
};

mod arena;
mod attributes;
mod body_checking;
mod bootstrap;
mod classes;
mod field_defaults;
mod inference;
mod interfaces;
mod numeric;
mod required_constants;
mod signature_resolution;

pub use arena::{TypeArena, TypeArenaError};
pub use attributes::{
    AttributeAttachmentError, AttributeAttachmentResult, AttributeAttachmentSet, AttributeConstant,
    AttributeContractError, AttributeDefinition, AttributeDefinitionResult,
    AttributeParameterDefinition, AttributeQueryError, AttributeQueryIndex,
    AttributeQueryIndexError, AttributeQuerySubject, AttributeQueryValue, AttributeTarget,
    AttributeUsage, AttributeValidator, ResolvedAttribute, ResolvedAttributeArgument,
    ResolvedAttributeResult,
};
pub use body_checking::{
    BodyChecker, CaptureMode, CaptureSource, TypedBinaryOperator, TypedBody, TypedBodyResult,
    TypedCall, TypedCallDispatch, TypedCapture, TypedClosure, TypedClosureParameter,
    TypedExpression, TypedExpressionKind, TypedExpressionResult, TypedFieldValue, TypedMatchArm,
    TypedMatchBinding, TypedStatement, TypedStatementKind, TypedTableEntry, TypedUnaryOperator,
};
pub use bootstrap::{
    AttributeIdentity, BootstrapCompilerAttributeEntry, BootstrapIntrinsicEntry,
    BootstrapPrimitiveEntry, BootstrapSchema, BootstrapSchemaError, BootstrapTypeEntry,
    BootstrapTypeRole, CompilerAttributeId, CompilerAttributeRole, CompilerAttributeTarget,
    embedded_bootstrap_schema,
};
pub use classes::{
    ClassDefinition, ClassDefinitionResult, ClassFieldDefinition, ClassMethodDefinition,
    ClassMethodDispatch,
};
pub use field_defaults::FieldDefault;
pub use inference::{InferenceContext, InferenceError, InferenceType, InferenceVariableId};
pub use interfaces::{
    ClassInterfaceImplementation, InterfaceDefinition, InterfaceDefinitionResult,
    InterfaceMethodDefinition, InterfaceMethodImplementation,
};
pub use numeric::{FloatKind, FloatValue, IntegerValue, NumericError};
pub use required_constants::{
    AttributeParameterId, PendingConstantExpression, RequiredConstantError, RequiredConstantTarget,
};
pub use signature_resolution::{
    RecordDefinition, RecordDefinitionResult, RecordFieldDefinition, ResolvedFunctionParameter,
    ResolvedFunctionSignature, ResolvedSignatureResult, ResolvedType, ResolvedTypeKind,
    ResolvedTypeParameter, SignatureResolver, UnionCaseDefinition, UnionDefinition,
    UnionDefinitionResult,
};

pub type ClassFieldDefault = FieldDefault;
pub type RecordFieldDefault = FieldDefault;

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegerOverflow {
    Trap,
    WrapExplicitly,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
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

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SemanticType {
    Primitive(PrimitiveType),
    Tuple(Vec<TypeId>),
    Function {
        parameters: Vec<TypeId>,
        results: Vec<TypeId>,
        effects: EffectSummary,
    },
    Record(Vec<(String, TypeId)>),
    TaggedUnion {
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

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct EffectSummary(u16);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Effect {
    Allocates,
    WritesManagedReference,
    MayTrap,
    MayUnwind,
    Suspends,
    UnsafeMemory,
    ForeignFunction,
    AmbientIo,
    CompilerQuery,
    GcSafePoint,
    Roots,
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
