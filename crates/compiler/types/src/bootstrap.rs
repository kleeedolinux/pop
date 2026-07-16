use std::error::Error;
use std::fmt;

use pop_foundation::{
    AttributeId, BuiltinTypeId, IterationCaseId, IterationProtocolMethodId, StandardFunctionId,
};

use crate::PrimitiveType;

const PRIMITIVES: &str = include_str!("../../../../libraries/internal/bootstrap/primitives.tsv");
const INTRINSICS: &str = include_str!("../../../../libraries/internal/bootstrap/intrinsics.tsv");
const INTERNAL_TYPES: &str = include_str!("../../../../libraries/internal/bootstrap/types.tsv");
const STANDARD_TYPES: &str =
    include_str!("../../../../libraries/standard/bootstrap/prelude-types.tsv");
const STANDARD_COMPILER_ATTRIBUTES: &str =
    include_str!("../../../../libraries/standard/bootstrap/compiler-attributes.tsv");
const STANDARD_FUNCTIONS: &str =
    include_str!("../../../../libraries/standard/bootstrap/functions.tsv");
const FFI_TYPES: &str = include_str!("../../../extensions/ffi/bootstrap/types.tsv");
const FFI_COMPILER_ATTRIBUTES: &str =
    include_str!("../../../extensions/ffi/bootstrap/compiler-attributes.tsv");

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapStandardFunctionEntry {
    id: StandardFunctionId,
    source_name: &'static str,
    owner_bubble: &'static str,
    parameter_types: Vec<&'static str>,
    result_types: Vec<&'static str>,
    effects: Vec<&'static str>,
    prelude: bool,
}

impl BootstrapStandardFunctionEntry {
    #[must_use]
    pub const fn id(&self) -> StandardFunctionId {
        self.id
    }

    #[must_use]
    pub const fn source_name(&self) -> &'static str {
        self.source_name
    }

    #[must_use]
    pub fn parameter_types(&self) -> &[&'static str] {
        &self.parameter_types
    }

    #[must_use]
    pub fn result_types(&self) -> &[&'static str] {
        &self.result_types
    }

    #[must_use]
    pub fn effects(&self) -> &[&'static str] {
        &self.effects
    }

    #[must_use]
    pub const fn owner_bubble(&self) -> &'static str {
        self.owner_bubble
    }

    #[must_use]
    pub const fn is_in_prelude(&self) -> bool {
        self.prelude
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CompilerAttributeId(u32);

impl CompilerAttributeId {
    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AttributeIdentity {
    Compiler(CompilerAttributeId),
    User(AttributeId),
}

impl From<AttributeId> for AttributeIdentity {
    fn from(attribute: AttributeId) -> Self {
        Self::User(attribute)
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CompilerAttributeRole {
    CompileTime,
    AttributeUsage,
    AttributeValidator,
    FfiLink,
    FfiForeign,
    FfiNonblocking,
    FfiCLayout,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum CompilerAttributeTarget {
    Function,
    Attribute,
    Namespace,
    Record,
}

#[derive(Clone, Copy)]
struct ExpectedCompilerAttribute {
    role: CompilerAttributeRole,
    id: u32,
    source_name: &'static str,
    owner_bubble: &'static str,
    argument_count: u16,
    target: CompilerAttributeTarget,
    prelude: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapCompilerAttributeEntry {
    id: CompilerAttributeId,
    source_name: &'static str,
    owner_bubble: &'static str,
    argument_count: u16,
    target: CompilerAttributeTarget,
    role: CompilerAttributeRole,
    prelude: bool,
}

impl BootstrapCompilerAttributeEntry {
    #[must_use]
    pub const fn id(self) -> CompilerAttributeId {
        self.id
    }

    #[must_use]
    pub const fn identity(self) -> AttributeIdentity {
        AttributeIdentity::Compiler(self.id)
    }

    #[must_use]
    pub const fn source_name(self) -> &'static str {
        self.source_name
    }

    #[must_use]
    pub const fn owner_bubble(self) -> &'static str {
        self.owner_bubble
    }

    #[must_use]
    pub const fn argument_count(self) -> u16 {
        self.argument_count
    }

    #[must_use]
    pub const fn target(self) -> CompilerAttributeTarget {
        self.target
    }

    #[must_use]
    pub const fn role(self) -> CompilerAttributeRole {
        self.role
    }

    #[must_use]
    pub const fn is_in_prelude(self) -> bool {
        self.prelude
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapPrimitiveEntry {
    source_name: &'static str,
    canonical_name: &'static str,
    runtime_role: &'static str,
}

impl BootstrapPrimitiveEntry {
    #[must_use]
    pub const fn source_name(self) -> &'static str {
        self.source_name
    }

    #[must_use]
    pub const fn canonical_name(self) -> &'static str {
        self.canonical_name
    }

    #[must_use]
    pub const fn runtime_role(self) -> &'static str {
        self.runtime_role
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapIntrinsicEntry {
    intrinsic_id: &'static str,
    owner: &'static str,
    signature: &'static str,
    lowering_kind: &'static str,
    required_capabilities: Vec<&'static str>,
}

impl BootstrapIntrinsicEntry {
    #[must_use]
    pub const fn intrinsic_id(&self) -> &'static str {
        self.intrinsic_id
    }

    #[must_use]
    pub const fn owner(&self) -> &'static str {
        self.owner
    }

    #[must_use]
    pub const fn signature(&self) -> &'static str {
        self.signature
    }

    #[must_use]
    pub const fn lowering_kind(&self) -> &'static str {
        self.lowering_kind
    }

    #[must_use]
    pub fn required_capabilities(&self) -> &[&'static str] {
        &self.required_capabilities
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapTypeEntry {
    id: BuiltinTypeId,
    source_name: &'static str,
    owner_bubble: &'static str,
    arity: u16,
    role: BootstrapTypeRole,
    prelude: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BootstrapIterationProtocol {
    iteration: BuiltinTypeId,
    iterable: BuiltinTypeId,
    iterator: BuiltinTypeId,
    list: BuiltinTypeId,
    range: BuiltinTypeId,
}

impl BootstrapIterationProtocol {
    #[must_use]
    pub const fn iteration(self) -> BuiltinTypeId {
        self.iteration
    }

    #[must_use]
    pub const fn iterable(self) -> BuiltinTypeId {
        self.iterable
    }

    #[must_use]
    pub const fn iterator(self) -> BuiltinTypeId {
        self.iterator
    }

    #[must_use]
    pub const fn list(self) -> BuiltinTypeId {
        self.list
    }

    #[must_use]
    pub const fn range(self) -> BuiltinTypeId {
        self.range
    }

    #[must_use]
    pub const fn item_case(self) -> IterationCaseId {
        IterationCaseId::from_raw(0)
    }

    #[must_use]
    pub const fn end_case(self) -> IterationCaseId {
        IterationCaseId::from_raw(1)
    }

    #[must_use]
    pub const fn iterator_method(self) -> IterationProtocolMethodId {
        IterationProtocolMethodId::from_raw(0)
    }

    #[must_use]
    pub const fn next_method(self) -> IterationProtocolMethodId {
        IterationProtocolMethodId::from_raw(1)
    }
}

impl BootstrapTypeEntry {
    #[must_use]
    pub const fn id(self) -> BuiltinTypeId {
        self.id
    }

    #[must_use]
    pub const fn source_name(self) -> &'static str {
        self.source_name
    }

    #[must_use]
    pub const fn owner_bubble(self) -> &'static str {
        self.owner_bubble
    }

    #[must_use]
    pub const fn arity(self) -> u16 {
        self.arity
    }

    #[must_use]
    pub const fn role(self) -> BootstrapTypeRole {
        self.role
    }

    #[must_use]
    pub const fn is_in_prelude(self) -> bool {
        self.prelude
    }
}

/// Returns whether an exact bootstrap identity belongs to the closed Pop.Ffi
/// ABI vocabulary. These values have scalar native representations and are
/// never managed-reference tokens.
#[must_use]
pub const fn is_ffi_abi_builtin_type(id: BuiltinTypeId) -> bool {
    matches!(id.raw(), 200..=206 | 210..=223)
}

/// Returns whether an identity is one of the closed C integer ABI scalars.
#[must_use]
pub const fn is_ffi_integer_abi_builtin_type(id: BuiltinTypeId) -> bool {
    matches!(id.raw(), 210..=222)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FfiCIntegerKind {
    Char,
    SignedChar,
    UnsignedChar,
    Short,
    UnsignedShort,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    LongLong,
    UnsignedLongLong,
    Size,
    PointerDifference,
}

/// Returns the closed C integer kind for its stable bootstrap identity.
#[must_use]
pub const fn ffi_c_integer_kind(id: BuiltinTypeId) -> Option<FfiCIntegerKind> {
    Some(match id.raw() {
        210 => FfiCIntegerKind::Char,
        211 => FfiCIntegerKind::SignedChar,
        212 => FfiCIntegerKind::UnsignedChar,
        213 => FfiCIntegerKind::Short,
        214 => FfiCIntegerKind::UnsignedShort,
        215 => FfiCIntegerKind::Int,
        216 => FfiCIntegerKind::UnsignedInt,
        217 => FfiCIntegerKind::Long,
        218 => FfiCIntegerKind::UnsignedLong,
        219 => FfiCIntegerKind::LongLong,
        220 => FfiCIntegerKind::UnsignedLongLong,
        221 => FfiCIntegerKind::Size,
        222 => FfiCIntegerKind::PointerDifference,
        _ => return None,
    })
}

/// Returns whether an identity is a mutable or read-only FFI pointer
/// constructor, including its optional form.
#[must_use]
pub const fn is_ffi_pointer_type_constructor(id: BuiltinTypeId) -> bool {
    matches!(id.raw(), 200 | 201 | 205 | 206)
}

/// Returns whether an identity is an FFI function-pointer constructor,
/// including its optional form.
#[must_use]
pub const fn is_ffi_function_type_constructor(id: BuiltinTypeId) -> bool {
    matches!(id.raw(), 202 | 203)
}

/// Stable bootstrap identity of the exact `Ffi.Handle<T>` type constructor.
pub const FFI_HANDLE_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(204);
/// Stable bootstrap identity of the required `Ffi.Function<TSignature>` type constructor.
pub const FFI_FUNCTION_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(202);
/// Stable bootstrap identity of the exact `Ffi.Pointer<T>` type constructor.
pub const FFI_POINTER_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(200);
/// Stable bootstrap identity of the exact `Ffi.OptionalPointer<T>` type constructor.
pub const FFI_OPTIONAL_POINTER_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(201);
/// Stable bootstrap identity of the exact `Ffi.ReadOnlyPointer<T>` type constructor.
pub const FFI_READ_ONLY_POINTER_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(205);
/// Stable bootstrap identity of the exact `Ffi.OptionalReadOnlyPointer<T>` type constructor.
pub const FFI_OPTIONAL_READ_ONLY_POINTER_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(206);
/// Stable bootstrap identity of the exact `Ffi.Buffer<T>` type constructor.
pub const FFI_BUFFER_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(207);
/// Stable bootstrap identity of `Ffi.NullPointerError`.
pub const FFI_NULL_POINTER_ERROR_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(208);
/// Stable bootstrap identity of `Ffi.AllocationError`.
pub const FFI_ALLOCATION_ERROR_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(209);
/// Stable bootstrap identity of the opaque `Ffi.CallbackContext` ABI type.
pub const FFI_CALLBACK_CONTEXT_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(223);
/// Stable bootstrap identity of `Ffi.RegisteredCallback<TSignature>`.
pub const FFI_REGISTERED_CALLBACK_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(224);
/// Stable bootstrap identity of the closed `Ffi.CallbackThread` enum.
pub const FFI_CALLBACK_THREAD_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(225);
/// Stable bootstrap identity of `Ffi.CallbackOpenError`.
pub const FFI_CALLBACK_OPEN_ERROR_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(226);
/// Stable bootstrap identity of `Ffi.CallbackInUseError`.
pub const FFI_CALLBACK_IN_USE_ERROR_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(227);
/// Stable bootstrap identity of `Ffi.CallbackClosedError`.
pub const FFI_CALLBACK_CLOSED_ERROR_TYPE_ID: BuiltinTypeId = BuiltinTypeId::from_raw(228);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootstrapTypeRole {
    Array,
    Table,
    Nominal,
    Interface,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapSchema {
    version: u32,
    primitives: Vec<BootstrapPrimitiveEntry>,
    types: Vec<BootstrapTypeEntry>,
    intrinsics: Vec<BootstrapIntrinsicEntry>,
    compiler_attributes: Vec<BootstrapCompilerAttributeEntry>,
    standard_functions: Vec<BootstrapStandardFunctionEntry>,
}

impl BootstrapSchema {
    #[must_use]
    pub const fn version(&self) -> u32 {
        self.version
    }

    #[must_use]
    pub fn primitives(&self) -> &[BootstrapPrimitiveEntry] {
        &self.primitives
    }

    #[must_use]
    pub fn types(&self) -> &[BootstrapTypeEntry] {
        &self.types
    }

    #[must_use]
    pub fn type_by_source_name(&self, name: &str) -> Option<&BootstrapTypeEntry> {
        self.types.iter().find(|entry| entry.source_name == name)
    }

    #[must_use]
    pub fn type_by_id(&self, id: BuiltinTypeId) -> Option<&BootstrapTypeEntry> {
        self.types.iter().find(|entry| entry.id == id)
    }

    #[must_use]
    pub fn iteration_protocol(&self) -> Option<BootstrapIterationProtocol> {
        let iteration = self.type_by_source_name("Iteration")?;
        let iterable = self.type_by_source_name("Iterable")?;
        let iterator = self.type_by_source_name("Iterator")?;
        let list = self.type_by_source_name("List")?;
        let range = self.type_by_source_name("Range")?;
        (iteration.arity == 1
            && iteration.role == BootstrapTypeRole::Nominal
            && iterable.arity == 1
            && iterable.role == BootstrapTypeRole::Interface
            && iterator.arity == 1
            && iterator.role == BootstrapTypeRole::Interface
            && list.arity == 1
            && list.role == BootstrapTypeRole::Nominal
            && range.arity == 1
            && range.role == BootstrapTypeRole::Nominal)
            .then_some(BootstrapIterationProtocol {
                iteration: iteration.id,
                iterable: iterable.id,
                iterator: iterator.id,
                list: list.id,
                range: range.id,
            })
    }

    #[must_use]
    pub fn intrinsics(&self) -> &[BootstrapIntrinsicEntry] {
        &self.intrinsics
    }

    #[must_use]
    pub fn compiler_attributes(&self) -> &[BootstrapCompilerAttributeEntry] {
        &self.compiler_attributes
    }

    #[must_use]
    pub fn standard_functions(&self) -> &[BootstrapStandardFunctionEntry] {
        &self.standard_functions
    }

    pub fn standard_functions_by_source_name<'a>(
        &'a self,
        name: &'a str,
    ) -> impl Iterator<Item = &'a BootstrapStandardFunctionEntry> + 'a {
        self.standard_functions
            .iter()
            .filter(move |entry| entry.prelude && entry.source_name == name)
    }

    /// Finds a trusted prelude compiler-attribute candidate by source name.
    ///
    /// The returned entry carries a compiler identity distinct from every user
    /// [`AttributeId`]. Callers must still apply ordinary declaration/prelude
    /// resolution priority before selecting this fallback candidate.
    #[must_use]
    pub fn compiler_attribute_by_source_name(
        &self,
        name: &str,
    ) -> Option<&BootstrapCompilerAttributeEntry> {
        self.compiler_attributes
            .iter()
            .find(|entry| entry.source_name == name)
    }

    #[must_use]
    pub fn compiler_attribute_by_role(
        &self,
        role: CompilerAttributeRole,
    ) -> Option<&BootstrapCompilerAttributeEntry> {
        self.compiler_attributes
            .iter()
            .find(|entry| entry.role == role)
    }

    #[must_use]
    pub fn compiler_attribute(
        &self,
        identity: AttributeIdentity,
    ) -> Option<&BootstrapCompilerAttributeEntry> {
        let AttributeIdentity::Compiler(id) = identity else {
            return None;
        };
        self.compiler_attributes.iter().find(|entry| entry.id == id)
    }

    #[must_use]
    pub fn compiler_attribute_role(
        &self,
        identity: AttributeIdentity,
    ) -> Option<CompilerAttributeRole> {
        self.compiler_attribute(identity).map(|entry| entry.role)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootstrapSchemaError {
    document: &'static str,
    line: usize,
    reason: &'static str,
}

impl fmt::Display for BootstrapSchemaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid {} bootstrap schema at line {}: {}",
            self.document, self.line, self.reason
        )
    }
}

impl Error for BootstrapSchemaError {}

/// Loads and cross-validates the embedded `Pop.Internal` bootstrap schemas.
///
/// # Errors
///
/// Returns [`BootstrapSchemaError`] if either schema is malformed, versions
/// disagree, identifiers repeat, or primitive metadata diverges from the
/// semantic type contract.
pub fn embedded_bootstrap_schema() -> Result<BootstrapSchema, BootstrapSchemaError> {
    let (primitive_version, primitives) = parse_primitives()?;
    let (intrinsic_version, intrinsics) = parse_intrinsics()?;
    let (internal_type_version, mut types) = parse_types("internal type", INTERNAL_TYPES)?;
    let (standard_type_version, standard_types) = parse_types("standard type", STANDARD_TYPES)?;
    let (ffi_type_version, ffi_types) = parse_types("FFI type", FFI_TYPES)?;
    let (standard_compiler_attribute_version, mut compiler_attributes) =
        parse_compiler_attributes("standard compiler attribute", STANDARD_COMPILER_ATTRIBUTES)?;
    let (ffi_compiler_attribute_version, ffi_compiler_attributes) =
        parse_compiler_attributes("FFI compiler attribute", FFI_COMPILER_ATTRIBUTES)?;
    let (standard_function_version, standard_functions) = parse_standard_functions()?;
    types.extend(standard_types);
    types.extend(ffi_types);
    compiler_attributes.extend(ffi_compiler_attributes);
    if [
        intrinsic_version,
        internal_type_version,
        standard_type_version,
        ffi_type_version,
        standard_compiler_attribute_version,
        ffi_compiler_attribute_version,
        standard_function_version,
    ]
    .into_iter()
    .any(|version| version != primitive_version)
    {
        return Err(error("combined", 1, "schema versions disagree"));
    }
    validate_primitives(&primitives)?;
    validate_types(&types)?;
    validate_intrinsics(&intrinsics)?;
    validate_compiler_attributes(&compiler_attributes)?;
    validate_standard_functions(&standard_functions)?;
    Ok(BootstrapSchema {
        version: primitive_version,
        primitives,
        types,
        intrinsics,
        compiler_attributes,
        standard_functions,
    })
}

fn parse_standard_functions()
-> Result<(u32, Vec<BootstrapStandardFunctionEntry>), BootstrapSchemaError> {
    let mut lines = STANDARD_FUNCTIONS.lines();
    let version = parse_version("standard function", lines.next())?;
    if lines.next()
        != Some(
            "functionId\tsourceName\townerBubble\tparameterTypes\tresultTypes\teffects\tprelude",
        )
    {
        return Err(error("standard function", 2, "unexpected header"));
    }
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 7 {
            return Err(error(
                "standard function",
                index + 3,
                "expected seven fields",
            ));
        }
        let id = fields[0]
            .parse()
            .map(StandardFunctionId::from_raw)
            .map_err(|_| error("standard function", index + 3, "invalid function ID"))?;
        entries.push(BootstrapStandardFunctionEntry {
            id,
            source_name: fields[1],
            owner_bubble: fields[2],
            parameter_types: schema_list(fields[3]),
            result_types: schema_list(fields[4]),
            effects: schema_list(fields[5]),
            prelude: match fields[6] {
                "true" => true,
                "false" => false,
                _ => {
                    return Err(error(
                        "standard function",
                        index + 3,
                        "invalid prelude flag",
                    ));
                }
            },
        });
    }
    Ok((version, entries))
}

fn schema_list(field: &'static str) -> Vec<&'static str> {
    if field == "-" {
        Vec::new()
    } else {
        field.split(',').collect()
    }
}

fn parse_types(
    document: &'static str,
    text: &'static str,
) -> Result<(u32, Vec<BootstrapTypeEntry>), BootstrapSchemaError> {
    let mut lines = text.lines();
    let version = parse_version(document, lines.next())?;
    if lines.next() != Some("typeId\tsourceName\townerBubble\tarity\trole\tprelude") {
        return Err(error(document, 2, "unexpected header"));
    }
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 6 {
            return Err(error(document, index + 3, "expected six fields"));
        }
        let id = fields[0]
            .parse()
            .map(BuiltinTypeId::from_raw)
            .map_err(|_| error(document, index + 3, "invalid type ID"))?;
        let arity = fields[3]
            .parse()
            .map_err(|_| error(document, index + 3, "invalid type arity"))?;
        let role = match fields[4] {
            "Array" => BootstrapTypeRole::Array,
            "Table" => BootstrapTypeRole::Table,
            "Nominal" => BootstrapTypeRole::Nominal,
            "Interface" => BootstrapTypeRole::Interface,
            _ => return Err(error(document, index + 3, "invalid type role")),
        };
        let prelude = match fields[5] {
            "true" => true,
            "false" => false,
            _ => return Err(error(document, index + 3, "invalid prelude flag")),
        };
        entries.push(BootstrapTypeEntry {
            id,
            source_name: fields[1],
            owner_bubble: fields[2],
            arity,
            role,
            prelude,
        });
    }
    Ok((version, entries))
}

fn parse_compiler_attributes(
    document: &'static str,
    text: &'static str,
) -> Result<(u32, Vec<BootstrapCompilerAttributeEntry>), BootstrapSchemaError> {
    let mut lines = text.lines();
    let version = parse_version(document, lines.next())?;
    if lines.next()
        != Some(
            "attributeId\tsourceName\townerBubble\targumentCount\ttarget\tsemanticRole\tprelude",
        )
    {
        return Err(error(document, 2, "unexpected header"));
    }
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 7 {
            return Err(error(document, index + 3, "expected seven fields"));
        }
        let id = fields[0]
            .parse()
            .map(CompilerAttributeId)
            .map_err(|_| error(document, index + 3, "invalid attribute ID"))?;
        let argument_count = fields[3]
            .parse()
            .map_err(|_| error(document, index + 3, "invalid argument count"))?;
        let target = match fields[4] {
            "Function" => CompilerAttributeTarget::Function,
            "Attribute" => CompilerAttributeTarget::Attribute,
            "Namespace" => CompilerAttributeTarget::Namespace,
            "Record" => CompilerAttributeTarget::Record,
            _ => {
                return Err(error(document, index + 3, "invalid attachment target"));
            }
        };
        let role = match fields[5] {
            "CompileTime" => CompilerAttributeRole::CompileTime,
            "AttributeUsage" => CompilerAttributeRole::AttributeUsage,
            "AttributeValidator" => CompilerAttributeRole::AttributeValidator,
            "FfiLink" => CompilerAttributeRole::FfiLink,
            "FfiForeign" => CompilerAttributeRole::FfiForeign,
            "FfiNonblocking" => CompilerAttributeRole::FfiNonblocking,
            "FfiCLayout" => CompilerAttributeRole::FfiCLayout,
            _ => {
                return Err(error(document, index + 3, "invalid semantic role"));
            }
        };
        let prelude = match fields[6] {
            "true" => true,
            "false" => false,
            _ => {
                return Err(error(document, index + 3, "invalid prelude flag"));
            }
        };
        entries.push(BootstrapCompilerAttributeEntry {
            id,
            source_name: fields[1],
            owner_bubble: fields[2],
            argument_count,
            target,
            role,
            prelude,
        });
    }
    Ok((version, entries))
}

fn parse_primitives() -> Result<(u32, Vec<BootstrapPrimitiveEntry>), BootstrapSchemaError> {
    let mut lines = PRIMITIVES.lines();
    let version = parse_version("primitive", lines.next())?;
    if lines.next() != Some("sourceName\tcanonicalName\truntimeRole") {
        return Err(error("primitive", 2, "unexpected header"));
    }
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 3 {
            return Err(error("primitive", index + 3, "expected three fields"));
        }
        entries.push(BootstrapPrimitiveEntry {
            source_name: fields[0],
            canonical_name: fields[1],
            runtime_role: fields[2],
        });
    }
    Ok((version, entries))
}

fn parse_intrinsics() -> Result<(u32, Vec<BootstrapIntrinsicEntry>), BootstrapSchemaError> {
    let mut lines = INTRINSICS.lines();
    let version = parse_version("intrinsic", lines.next())?;
    if lines.next() != Some("intrinsicId\towner\tsignature\tloweringKind\trequiredCapabilities") {
        return Err(error("intrinsic", 2, "unexpected header"));
    }
    let mut entries = Vec::new();
    for (index, line) in lines.enumerate() {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split('\t').collect();
        if fields.len() != 5 {
            return Err(error("intrinsic", index + 3, "expected five fields"));
        }
        let required_capabilities = if fields[4] == "-" {
            Vec::new()
        } else {
            fields[4].split(',').collect()
        };
        entries.push(BootstrapIntrinsicEntry {
            intrinsic_id: fields[0],
            owner: fields[1],
            signature: fields[2],
            lowering_kind: fields[3],
            required_capabilities,
        });
    }
    Ok((version, entries))
}

fn parse_version(
    document: &'static str,
    line: Option<&'static str>,
) -> Result<u32, BootstrapSchemaError> {
    let Some((key, value)) = line.and_then(|line| line.split_once('\t')) else {
        return Err(error(document, 1, "missing schema version"));
    };
    if key != "schemaVersion" {
        return Err(error(document, 1, "unexpected version key"));
    }
    value
        .parse()
        .map_err(|_| error(document, 1, "invalid schema version"))
}

fn validate_primitives(entries: &[BootstrapPrimitiveEntry]) -> Result<(), BootstrapSchemaError> {
    let semantic = PrimitiveType::source_schema();
    if entries.len() != semantic.len() {
        return Err(error("primitive", 2, "semantic entry count differs"));
    }
    for (index, (metadata, semantic)) in entries.iter().zip(semantic).enumerate() {
        if metadata.source_name != semantic.source_name()
            || metadata.canonical_name != semantic.canonical_name()
        {
            return Err(error(
                "primitive",
                index + 3,
                "semantic primitive identity differs",
            ));
        }
    }
    Ok(())
}

fn validate_intrinsics(entries: &[BootstrapIntrinsicEntry]) -> Result<(), BootstrapSchemaError> {
    let mut identifiers: Vec<_> = entries.iter().map(|entry| entry.intrinsic_id).collect();
    identifiers.sort_unstable();
    if identifiers.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(error("intrinsic", 2, "duplicate intrinsic ID"));
    }
    if entries.iter().any(|entry| {
        entry.owner.is_empty()
            || entry.signature.is_empty()
            || entry.lowering_kind.is_empty()
            || !entry.owner.starts_with("Pop.Internal.")
    }) {
        return Err(error("intrinsic", 2, "invalid intrinsic contract"));
    }
    Ok(())
}

fn validate_types(entries: &[BootstrapTypeEntry]) -> Result<(), BootstrapSchemaError> {
    let mut ids: Vec<_> = entries.iter().map(|entry| entry.id).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(error("type", 2, "duplicate type ID"));
    }
    let mut names: Vec<_> = entries.iter().map(|entry| entry.source_name).collect();
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(error("type", 2, "duplicate source type name"));
    }
    if entries.iter().any(|entry| {
        !matches!(
            entry.owner_bubble,
            "Pop.Internal" | "Pop.Standard" | "Pop.Ffi"
        ) || entry.source_name.is_empty()
    }) {
        return Err(error("type", 2, "invalid foundational type contract"));
    }
    let ffi_contracts = [
        (200, "Ffi.Pointer", 1),
        (201, "Ffi.OptionalPointer", 1),
        (202, "Ffi.Function", 1),
        (203, "Ffi.OptionalFunction", 1),
        (204, "Ffi.Handle", 1),
        (205, "Ffi.ReadOnlyPointer", 1),
        (206, "Ffi.OptionalReadOnlyPointer", 1),
        (207, "Ffi.Buffer", 1),
        (208, "Ffi.NullPointerError", 0),
        (209, "Ffi.AllocationError", 0),
        (210, "Ffi.C.Char", 0),
        (211, "Ffi.C.SignedChar", 0),
        (212, "Ffi.C.UnsignedChar", 0),
        (213, "Ffi.C.Short", 0),
        (214, "Ffi.C.UnsignedShort", 0),
        (215, "Ffi.C.Int", 0),
        (216, "Ffi.C.UnsignedInt", 0),
        (217, "Ffi.C.Long", 0),
        (218, "Ffi.C.UnsignedLong", 0),
        (219, "Ffi.C.LongLong", 0),
        (220, "Ffi.C.UnsignedLongLong", 0),
        (221, "Ffi.C.Size", 0),
        (222, "Ffi.C.PointerDifference", 0),
        (223, "Ffi.CallbackContext", 0),
        (224, "Ffi.RegisteredCallback", 1),
        (225, "Ffi.CallbackThread", 0),
        (226, "Ffi.CallbackOpenError", 0),
        (227, "Ffi.CallbackInUseError", 0),
        (228, "Ffi.CallbackClosedError", 0),
    ];
    if entries
        .iter()
        .filter(|entry| entry.owner_bubble == "Pop.Ffi")
        .count()
        != ffi_contracts.len()
    {
        return Err(error("FFI type", 2, "unexpected FFI type set"));
    }
    for (id, source_name, arity) in ffi_contracts {
        let Some(entry) = entries
            .iter()
            .find(|entry| entry.owner_bubble == "Pop.Ffi" && entry.source_name == source_name)
        else {
            return Err(error("FFI type", 2, "missing required FFI type"));
        };
        if entry.id.raw() != id
            || entry.arity != arity
            || entry.role != BootstrapTypeRole::Nominal
            || entry.prelude
        {
            return Err(error("FFI type", 2, "invalid trusted FFI type contract"));
        }
    }
    Ok(())
}

fn validate_compiler_attributes(
    entries: &[BootstrapCompilerAttributeEntry],
) -> Result<(), BootstrapSchemaError> {
    if entries.len() != 7 {
        return Err(error(
            "compiler attribute",
            2,
            "unexpected compiler attribute set",
        ));
    }
    let mut ids: Vec<_> = entries.iter().map(|entry| entry.id).collect();
    ids.sort_unstable();
    if ids.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(error(
            "compiler attribute",
            2,
            "duplicate compiler attribute ID",
        ));
    }
    let mut names: Vec<_> = entries.iter().map(|entry| entry.source_name).collect();
    names.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(error(
            "compiler attribute",
            2,
            "duplicate compiler attribute name",
        ));
    }
    let mut roles: Vec<_> = entries.iter().map(|entry| entry.role).collect();
    roles.sort_unstable();
    if roles.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(error(
            "compiler attribute",
            2,
            "duplicate compiler attribute role",
        ));
    }
    validate_compiler_attribute_contract(
        entries,
        ExpectedCompilerAttribute {
            role: CompilerAttributeRole::CompileTime,
            id: 0,
            source_name: "CompileTime",
            owner_bubble: "Pop.Standard",
            argument_count: 0,
            target: CompilerAttributeTarget::Function,
            prelude: true,
        },
    )?;
    validate_compiler_attribute_contract(
        entries,
        ExpectedCompilerAttribute {
            role: CompilerAttributeRole::AttributeUsage,
            id: 1,
            source_name: "AttributeUsage",
            owner_bubble: "Pop.Standard",
            argument_count: 2,
            target: CompilerAttributeTarget::Attribute,
            prelude: true,
        },
    )?;
    validate_compiler_attribute_contract(
        entries,
        ExpectedCompilerAttribute {
            role: CompilerAttributeRole::AttributeValidator,
            id: 2,
            source_name: "AttributeValidator",
            owner_bubble: "Pop.Standard",
            argument_count: 1,
            target: CompilerAttributeTarget::Attribute,
            prelude: true,
        },
    )?;
    validate_compiler_attribute_contract(
        entries,
        ExpectedCompilerAttribute {
            role: CompilerAttributeRole::FfiLink,
            id: 100,
            source_name: "Ffi.Link",
            owner_bubble: "Pop.Ffi",
            argument_count: 1,
            target: CompilerAttributeTarget::Namespace,
            prelude: false,
        },
    )?;
    validate_compiler_attribute_contract(
        entries,
        ExpectedCompilerAttribute {
            role: CompilerAttributeRole::FfiForeign,
            id: 101,
            source_name: "Ffi.Foreign",
            owner_bubble: "Pop.Ffi",
            argument_count: 2,
            target: CompilerAttributeTarget::Function,
            prelude: false,
        },
    )?;
    validate_compiler_attribute_contract(
        entries,
        ExpectedCompilerAttribute {
            role: CompilerAttributeRole::FfiNonblocking,
            id: 102,
            source_name: "Ffi.Nonblocking",
            owner_bubble: "Pop.Ffi",
            argument_count: 0,
            target: CompilerAttributeTarget::Function,
            prelude: false,
        },
    )?;
    validate_compiler_attribute_contract(
        entries,
        ExpectedCompilerAttribute {
            role: CompilerAttributeRole::FfiCLayout,
            id: 103,
            source_name: "Ffi.C.Layout",
            owner_bubble: "Pop.Ffi",
            argument_count: 0,
            target: CompilerAttributeTarget::Record,
            prelude: false,
        },
    )?;
    Ok(())
}

fn validate_standard_functions(
    entries: &[BootstrapStandardFunctionEntry],
) -> Result<(), BootstrapSchemaError> {
    if entries.len() != 2 {
        return Err(error(
            "standard function",
            2,
            "bootstrap requires exactly two standard functions",
        ));
    }
    for (index, (entry, parameter_type)) in entries.iter().zip(["Int", "String"]).enumerate() {
        if entry.id.raw() != u32::try_from(index).unwrap_or(u32::MAX)
            || entry.source_name != "print"
            || entry.owner_bubble != "Pop.Standard"
            || entry.parameter_types != [parameter_type]
            || !entry.result_types.is_empty()
            || entry.effects != ["AmbientIo"]
            || !entry.prelude
        {
            return Err(error(
                "standard function",
                index + 3,
                "invalid trusted print contract",
            ));
        }
    }
    Ok(())
}

fn validate_compiler_attribute_contract(
    entries: &[BootstrapCompilerAttributeEntry],
    expected: ExpectedCompilerAttribute,
) -> Result<(), BootstrapSchemaError> {
    let Some(entry) = entries.iter().find(|entry| entry.role == expected.role) else {
        return Err(error(
            "compiler attribute",
            2,
            "missing required compiler attribute role",
        ));
    };
    if entry.id.raw() != expected.id
        || entry.source_name != expected.source_name
        || entry.owner_bubble != expected.owner_bubble
        || entry.argument_count != expected.argument_count
        || entry.target != expected.target
        || entry.prelude != expected.prelude
    {
        return Err(error(
            "compiler attribute",
            2,
            "invalid trusted compiler attribute contract",
        ));
    }
    Ok(())
}

const fn error(document: &'static str, line: usize, reason: &'static str) -> BootstrapSchemaError {
    BootstrapSchemaError {
        document,
        line,
        reason,
    }
}
