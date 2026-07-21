//! Closed retained-metadata requests and canonical typed `.popc` artifacts.
//!
//! This module deliberately has no JSON representation and never performs
//! runtime lookup. Its values are compiler-verified inputs to generated typed
//! adapter HIR.

use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt::{self, Write};

use pop_foundation::{
    BubbleId, BuiltinTypeId, Diagnostic, FileId, ModuleId, SourceSpan, SymbolId, SymbolIdentity,
    TextRange, TextSize, TypeId,
};
use pop_hir::{
    HirDeclaration, HirDeclarationKind, HirGeneratedCodecAdapter, HirGeneratedCodecEntry,
    HirGeneratedCodecEntryBody, HirGeneratedCodecEntryIdentity, HirGeneratedCodecEntryRole,
    HirGeneratedCodecMember, HirGeneratedCodecMemberId, HirGeneratedCodecProvenance,
};
use pop_resolve::{DeclarationIndex, SymbolSpace, Visibility};
use pop_types::{
    Effect, EffectSummary, IntegerKind, PrimitiveType, SemanticType, SignatureResolver, TypeArena,
};
use sha2::{Digest, Sha256};

use crate::api::{ReferenceMetadata, ReferenceRetainedAdapter, ReferenceRetainedAdapterIdentity};

const DESCRIPTOR_SCHEMA_VERSION: u16 = 1;
const ADAPTER_PROTOCOL_VERSION: u16 = 1;
const MAX_DESCRIPTOR_BYTES: usize = 4 * 1024 * 1024;
const MAX_ADAPTERS: usize = 4_096;
const MAX_MEMBERS: usize = 256;
const MAX_PROJECTION_DEPTH: usize = 32;
const MAX_PROJECTION_NODES: usize = 65_536;
const MAX_LABEL_BYTES: usize = 1024 * 1024;
const MAX_TEXT_BYTES: usize = 1024;
const METADATA_USE_DEFINITION: BuiltinTypeId = BuiltinTypeId::from_raw(117);
const CODEC_SCHEMA_DEFINITION: BuiltinTypeId = BuiltinTypeId::from_raw(118);
const CODEC_WRITER_DEFINITION: BuiltinTypeId = BuiltinTypeId::from_raw(119);
const CODEC_READER_DEFINITION: BuiltinTypeId = BuiltinTypeId::from_raw(120);
const CODEC_ERROR_DEFINITION: BuiltinTypeId = BuiltinTypeId::from_raw(121);
const RESULT_DEFINITION: BuiltinTypeId = BuiltinTypeId::from_raw(100);

fn generated_codec_entries(
    schema_symbol: SymbolId,
    schema_identity: SymbolIdentity,
    target_type: TypeId,
    provenance: HirGeneratedCodecProvenance,
    arena: &mut TypeArena,
) -> Result<(HirGeneratedCodecEntry, HirGeneratedCodecEntry), RetainedMetadataError> {
    let local_encode = schema_symbol
        .raw()
        .checked_add(1)
        .map(SymbolId::from_raw)
        .ok_or(RetainedMetadataError::LimitExceeded)?;
    let local_decode = schema_symbol
        .raw()
        .checked_add(2)
        .map(SymbolId::from_raw)
        .ok_or(RetainedMetadataError::LimitExceeded)?;
    let builtin = |arena: &mut TypeArena, definition| {
        arena
            .intern(SemanticType::Builtin {
                definition,
                arguments: Vec::new(),
            })
            .map_err(|_| RetainedMetadataError::InvalidProjection)
    };
    let writer = builtin(arena, CODEC_WRITER_DEFINITION)?;
    let reader = builtin(arena, CODEC_READER_DEFINITION)?;
    let error = builtin(arena, CODEC_ERROR_DEFINITION)?;
    let nil = arena
        .source_type("nil")
        .ok_or(RetainedMetadataError::InvalidProjection)?;
    let encode_result = arena
        .intern(SemanticType::Builtin {
            definition: RESULT_DEFINITION,
            arguments: vec![nil, error],
        })
        .map_err(|_| RetainedMetadataError::InvalidProjection)?;
    let decode_result = arena
        .intern(SemanticType::Builtin {
            definition: RESULT_DEFINITION,
            arguments: vec![target_type, error],
        })
        .map_err(|_| RetainedMetadataError::InvalidProjection)?;
    let effects = EffectSummary::empty()
        .with(Effect::Allocates)
        .with(Effect::GcSafePoint);
    Ok((
        HirGeneratedCodecEntry::new(
            HirGeneratedCodecEntryIdentity::new(
                schema_identity,
                HirGeneratedCodecEntryRole::Encode,
            ),
            local_encode,
            vec![target_type, writer],
            vec![encode_result],
            effects,
            provenance,
            HirGeneratedCodecEntryBody::CodecEncode {
                adapter: schema_symbol,
            },
        ),
        HirGeneratedCodecEntry::new(
            HirGeneratedCodecEntryIdentity::new(
                schema_identity,
                HirGeneratedCodecEntryRole::Decode,
            ),
            local_decode,
            vec![reader],
            vec![decode_result],
            effects,
            provenance,
            HirGeneratedCodecEntryBody::CodecDecode {
                adapter: schema_symbol,
            },
        ),
    ))
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct RetainedMetadataRequest {
    symbol: SymbolId,
    module: ModuleId,
    schema_version: u32,
    attachment_span: SourceSpan,
}

impl RetainedMetadataRequest {
    #[must_use]
    pub(crate) const fn new(
        symbol: SymbolId,
        module: ModuleId,
        schema_version: u32,
        attachment_span: SourceSpan,
    ) -> Self {
        Self {
            symbol,
            module,
            schema_version,
            attachment_span,
        }
    }

    #[must_use]
    pub(crate) const fn symbol(&self) -> SymbolId {
        self.symbol
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedAdapterKind {
    Record,
    Enum,
    Union,
}

impl RetainedAdapterKind {
    const fn source_name(self) -> &'static str {
        match self {
            Self::Record => "record",
            Self::Enum => "enum",
            Self::Union => "union",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RetainedProjectionType {
    Boolean,
    Int8,
    Int16,
    Int32,
    Int64,
    UInt8,
    UInt16,
    UInt32,
    UInt64,
    Float32,
    Float64,
    String,
    Bytes,
    Optional(Box<Self>),
    Tuple(Vec<Self>),
    Array(Box<Self>),
    List(Box<Self>),
    Nominal {
        target: String,
        projection_sha256: String,
    },
}

impl RetainedProjectionType {
    fn render(&self) -> String {
        match self {
            Self::Boolean => "Boolean".to_owned(),
            Self::Int8 => "Int8".to_owned(),
            Self::Int16 => "Int16".to_owned(),
            Self::Int32 => "Int32".to_owned(),
            Self::Int64 => "Int64".to_owned(),
            Self::UInt8 => "UInt8".to_owned(),
            Self::UInt16 => "UInt16".to_owned(),
            Self::UInt32 => "UInt32".to_owned(),
            Self::UInt64 => "UInt64".to_owned(),
            Self::Float32 => "Float32".to_owned(),
            Self::Float64 => "Float64".to_owned(),
            Self::String => "String".to_owned(),
            Self::Bytes => "Bytes".to_owned(),
            Self::Optional(value) => format!("{}?", value.render()),
            Self::Tuple(values) => format!(
                "({})",
                values
                    .iter()
                    .map(Self::render)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            Self::Array(value) => format!("Array<{}>", value.render()),
            Self::List(value) => format!("List<{}>", value.render()),
            Self::Nominal { target, .. } => target.clone(),
        }
    }

    fn fingerprint_token(&self, output: &mut String) {
        match self {
            Self::Optional(value) => {
                output.push_str("optional(");
                value.fingerprint_token(output);
                output.push(')');
            }
            Self::Tuple(values) => {
                output.push_str("tuple(");
                for value in values {
                    value.fingerprint_token(output);
                    output.push(';');
                }
                output.push(')');
            }
            Self::Array(value) => {
                output.push_str("array(");
                value.fingerprint_token(output);
                output.push(')');
            }
            Self::List(value) => {
                output.push_str("list(");
                value.fingerprint_token(output);
                output.push(')');
            }
            Self::Nominal {
                target,
                projection_sha256,
            } => {
                let _ = write!(output, "nominal({target},{projection_sha256})");
            }
            value => output.push_str(&value.render()),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedAdapterMember {
    ordinal: u16,
    name: String,
    projected_type: RetainedProjectionType,
    discriminant: Option<u32>,
}

impl RetainedAdapterMember {
    #[must_use]
    pub const fn ordinal(&self) -> u16 {
        self.ordinal
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn projected_type(&self) -> &RetainedProjectionType {
        &self.projected_type
    }

    #[must_use]
    pub const fn discriminant(&self) -> Option<u32> {
        self.discriminant
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedMetadataAdapter {
    target: String,
    target_name: String,
    adapter: String,
    adapter_name: String,
    source_module: String,
    visibility: Visibility,
    kind: RetainedAdapterKind,
    schema_version: u32,
    attachment_start: u32,
    attachment_end: u32,
    target_start: u32,
    target_end: u32,
    projection_sha256: String,
    members: Vec<RetainedAdapterMember>,
}

impl RetainedMetadataAdapter {
    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    #[must_use]
    pub fn target_name(&self) -> &str {
        &self.target_name
    }

    #[must_use]
    pub fn adapter(&self) -> &str {
        &self.adapter
    }

    #[must_use]
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    #[must_use]
    pub const fn visibility(&self) -> Visibility {
        self.visibility
    }

    #[must_use]
    pub const fn kind(&self) -> RetainedAdapterKind {
        self.kind
    }

    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    #[must_use]
    pub fn projection_sha256(&self) -> &str {
        &self.projection_sha256
    }

    #[must_use]
    pub fn members(&self) -> &[RetainedAdapterMember] {
        &self.members
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RetainedMetadataArtifacts {
    bubble_identity: String,
    source_fingerprint: String,
    adapters: Vec<RetainedMetadataAdapter>,
    popc: Vec<u8>,
}

impl RetainedMetadataArtifacts {
    #[must_use]
    pub fn adapters(&self) -> &[RetainedMetadataAdapter] {
        &self.adapters
    }

    #[must_use]
    pub fn popc(&self) -> &[u8] {
        &self.popc
    }

    #[must_use]
    pub fn source_fingerprint(&self) -> &str {
        &self.source_fingerprint
    }

    /// Returns the canonical public-only descriptor published by a `.poplib`.
    ///
    /// # Errors
    ///
    /// Fails if the filtered descriptor exceeds the accepted schema limits.
    pub fn public_popc(&self) -> Result<Vec<u8>, RetainedMetadataError> {
        Ok(self
            .public_artifact()?
            .map_or_else(Vec::new, |artifact| artifact.popc))
    }

    pub(crate) fn public_artifact(&self) -> Result<Option<Self>, RetainedMetadataError> {
        let adapters = self
            .adapters
            .iter()
            .filter(|adapter| adapter.visibility == Visibility::Public)
            .cloned()
            .collect::<Vec<_>>();
        if adapters.is_empty() {
            return Ok(None);
        }
        let source_fingerprint = source_fingerprint(&adapters);
        let popc = encode_popc(&self.bubble_identity, &source_fingerprint, &adapters)?;
        let artifact = Self {
            bubble_identity: self.bubble_identity.clone(),
            source_fingerprint,
            adapters,
            popc,
        };
        if decode_retained_adapters_popc(&artifact.popc)? != artifact {
            return Err(RetainedMetadataError::NonCanonical);
        }
        Ok(Some(artifact))
    }

    pub(crate) fn public_references(
        &self,
        bubble: BubbleId,
        index: &DeclarationIndex,
    ) -> Result<Vec<ReferenceRetainedAdapter>, RetainedMetadataError> {
        let Some(public) = self.public_artifact()? else {
            return Ok(Vec::new());
        };
        let descriptor_size = public.popc.len() as u64;
        let descriptor_sha256 = hash_bytes(&public.popc);
        let mut references = public
            .adapters
            .iter()
            .map(|adapter| {
                let declarations =
                    index.declaration_by_qualified_name(&adapter.target, SymbolSpace::Type);
                let [target] = declarations.as_slice() else {
                    return Err(RetainedMetadataError::InvalidProjection);
                };
                if target.bubble() != bubble || target.visibility() != Visibility::Public {
                    return Err(RetainedMetadataError::InvalidProjection);
                }
                let schemas =
                    index.declaration_by_qualified_name(&adapter.adapter, SymbolSpace::Value);
                let [schema] = schemas.as_slice() else {
                    return Err(RetainedMetadataError::InvalidProjection);
                };
                Ok(ReferenceRetainedAdapter {
                    identity: ReferenceRetainedAdapterIdentity {
                        adapter: SymbolIdentity::new(bubble, schema.symbol()),
                        target: SymbolIdentity::new(bubble, target.symbol()),
                        use_definition: METADATA_USE_DEFINITION,
                        use_case: 0,
                        adapter_protocol_version: ADAPTER_PROTOCOL_VERSION,
                    },
                    module: target.module(),
                    namespace: target.namespace().to_owned(),
                    name: adapter.adapter_name.clone(),
                    schema_definition: CODEC_SCHEMA_DEFINITION,
                    descriptor_path: "retained-adapters.popc".to_owned(),
                    descriptor_size,
                    descriptor_sha256: descriptor_sha256.clone(),
                    projection_sha256: adapter.projection_sha256.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        references.sort_by_key(ReferenceRetainedAdapter::identity);
        Ok(references)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RetainedMetadataError {
    AnalysisUnavailable,
    InvalidProjection,
    InvalidDescriptor,
    NonCanonical,
    UnsupportedVersion,
    TooLarge,
    LimitExceeded,
}

pub(crate) fn validate_public_retained_metadata(
    metadata: &ReferenceMetadata,
    popc: Option<&[u8]>,
) -> Result<(), RetainedMetadataError> {
    let references = metadata.retained_adapters();
    validate_reference_retained_metadata(metadata)?;
    if references.is_empty() {
        return popc
            .is_none()
            .then_some(())
            .ok_or(RetainedMetadataError::InvalidDescriptor);
    }
    let bytes = popc.ok_or(RetainedMetadataError::InvalidDescriptor)?;
    let decoded = decode_retained_adapters_popc(bytes)?;
    if decoded.bubble_identity != format!("bubble:{}", metadata.bubble().raw())
        || decoded.adapters.len() != references.len()
        || decoded
            .adapters
            .iter()
            .any(|adapter| adapter.visibility != Visibility::Public)
    {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    let descriptor_sha256 = hash_bytes(bytes);
    for reference in references {
        if reference.descriptor_size != bytes.len() as u64
            || reference.descriptor_sha256 != descriptor_sha256
        {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
        let target_name = reference
            .name
            .strip_suffix("Schema")
            .ok_or(RetainedMetadataError::InvalidDescriptor)?;
        let target = if reference.namespace.is_empty() {
            target_name.to_owned()
        } else {
            format!("{}.{}", reference.namespace, target_name)
        };
        let adapter = if reference.namespace.is_empty() {
            reference.name.clone()
        } else {
            format!("{}.{}", reference.namespace, reference.name)
        };
        let matching = decoded.adapters.iter().filter(|entry| {
            entry.target == target
                && entry.adapter == adapter
                && entry.projection_sha256 == reference.projection_sha256
        });
        if matching.count() != 1 {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
    }
    Ok(())
}

pub(crate) fn validate_reference_retained_metadata(
    metadata: &ReferenceMetadata,
) -> Result<(), RetainedMetadataError> {
    let references = metadata.retained_adapters();
    if references
        .windows(2)
        .any(|pair| pair[0].identity >= pair[1].identity)
    {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    for reference in references {
        if reference.identity.target.bubble() != metadata.bubble()
            || reference.identity.use_definition != METADATA_USE_DEFINITION
            || reference.identity.use_case != 0
            || reference.identity.adapter_protocol_version != ADAPTER_PROTOCOL_VERSION
            || reference.schema_definition != CODEC_SCHEMA_DEFINITION
            || reference.descriptor_path != "retained-adapters.popc"
            || reference.descriptor_size == 0
            || !valid_sha256(&reference.descriptor_sha256)
            || !valid_sha256(&reference.projection_sha256)
            || !reference.name.ends_with("Schema")
            || !valid_qualified_identity(&reference.name)
            || (!reference.namespace.is_empty() && !valid_qualified_identity(&reference.namespace))
        {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
    }
    Ok(())
}

pub(crate) fn reference_retained_declarations(
    metadata: &[ReferenceMetadata],
    descriptors: &[(BubbleId, Vec<u8>)],
) -> Result<Vec<pop_resolve::ReferencedDeclaration>, RetainedMetadataError> {
    let descriptor_by_bubble = verified_reference_descriptors(metadata, descriptors)?;
    let span = SourceSpan::new(FileId::from_raw(0), TextRange::empty(TextSize::from_u32(0)));
    let mut declarations = Vec::new();
    for metadata in metadata {
        let Some(bytes) = descriptor_by_bubble.get(&metadata.bubble()).copied() else {
            continue;
        };
        let decoded = decode_retained_adapters_popc(bytes)?;
        for reference in metadata.retained_adapters() {
            let entry = matching_reference_entry(reference, &decoded)?;
            let (namespace, name) = split_qualified(&entry.target)?;
            let declaration = match entry.kind {
                RetainedAdapterKind::Record => pop_resolve::ReferencedDeclaration::record(
                    reference.identity().target(),
                    reference.module(),
                    namespace,
                    name,
                    span,
                ),
                RetainedAdapterKind::Enum => pop_resolve::ReferencedDeclaration::enumeration(
                    reference.identity().target(),
                    reference.module(),
                    namespace,
                    name,
                    span,
                ),
                RetainedAdapterKind::Union => pop_resolve::ReferencedDeclaration::union(
                    reference.identity().target(),
                    reference.module(),
                    namespace,
                    name,
                    span,
                ),
            };
            declarations.push(declaration);
        }
    }
    Ok(declarations)
}

fn verified_reference_descriptors<'a>(
    metadata: &[ReferenceMetadata],
    descriptors: &'a [(BubbleId, Vec<u8>)],
) -> Result<BTreeMap<BubbleId, &'a [u8]>, RetainedMetadataError> {
    if descriptors.windows(2).any(|pair| pair[0].0 >= pair[1].0) {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    let by_bubble = descriptors
        .iter()
        .map(|(bubble, bytes)| (*bubble, bytes.as_slice()))
        .collect::<BTreeMap<_, _>>();
    if by_bubble
        .keys()
        .any(|bubble| !metadata.iter().any(|value| value.bubble() == *bubble))
    {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    for value in metadata {
        validate_public_retained_metadata(value, by_bubble.get(&value.bubble()).copied())?;
    }
    Ok(by_bubble)
}

fn matching_reference_entry<'a>(
    reference: &ReferenceRetainedAdapter,
    decoded: &'a RetainedMetadataArtifacts,
) -> Result<&'a RetainedMetadataAdapter, RetainedMetadataError> {
    let adapter = if reference.namespace().is_empty() {
        reference.name().to_owned()
    } else {
        format!("{}.{}", reference.namespace(), reference.name())
    };
    let mut matching = decoded.adapters.iter().filter(|entry| {
        entry.adapter == adapter && entry.projection_sha256 == reference.projection_sha256()
    });
    let entry = matching
        .next()
        .ok_or(RetainedMetadataError::InvalidDescriptor)?;
    if matching.next().is_some() {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    Ok(entry)
}

fn split_qualified(value: &str) -> Result<(String, String), RetainedMetadataError> {
    let (namespace, name) = value
        .rsplit_once('.')
        .ok_or(RetainedMetadataError::InvalidDescriptor)?;
    if namespace.is_empty() || name.is_empty() {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    Ok((namespace.to_owned(), name.to_owned()))
}

impl fmt::Display for RetainedMetadataError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid retained-adapters.popc: {self:?}")
    }
}

impl Error for RetainedMetadataError {}

pub(crate) fn build_retained_metadata_artifacts(
    requests: &[RetainedMetadataRequest],
    declarations: &[HirDeclaration],
    index: &DeclarationIndex,
    arena: &TypeArena,
    module_origins: &BTreeMap<ModuleId, (String, SourceSpan)>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Result<RetainedMetadataArtifacts, RetainedMetadataError> {
    if requests.is_empty() {
        return Ok(RetainedMetadataArtifacts {
            bubble_identity: String::new(),
            source_fingerprint: String::new(),
            adapters: Vec::new(),
            popc: Vec::new(),
        });
    }
    if requests.len() > MAX_ADAPTERS {
        add_request_diagnostic(requests[0], diagnostics, "adapter limit exceeded");
        return Err(RetainedMetadataError::LimitExceeded);
    }
    let by_symbol = declarations
        .iter()
        .map(|declaration| (declaration.symbol(), declaration))
        .collect::<BTreeMap<_, _>>();
    let by_type = declarations
        .iter()
        .filter_map(|declaration| {
            let type_id = match declaration.kind() {
                HirDeclarationKind::Record(value) => value.type_id(),
                HirDeclarationKind::Union(value) => value.type_id(),
                HirDeclarationKind::Enum(value) => value.type_id(),
                _ => return None,
            };
            Some((type_id, declaration.symbol()))
        })
        .collect::<BTreeMap<_, _>>();
    let requests_by_symbol = requests
        .iter()
        .map(|request| (request.symbol, *request))
        .collect::<BTreeMap<_, _>>();
    let mut builder = ProjectionBuilder {
        requests: &requests_by_symbol,
        declarations: &by_symbol,
        declarations_by_type: &by_type,
        index,
        arena,
        module_origins,
        complete: BTreeMap::new(),
        active: BTreeSet::new(),
        nodes: 0,
        label_bytes: 0,
    };
    for request in requests {
        if let Err(reason) = builder.build_adapter(request.symbol, 0) {
            add_request_diagnostic(*request, diagnostics, reason);
            return Err(RetainedMetadataError::InvalidProjection);
        }
    }
    let mut adapters = builder.complete.into_values().collect::<Vec<_>>();
    adapters.sort_by(|left, right| left.target.cmp(&right.target));
    let bubble = declarations
        .first()
        .map_or(0, |declaration| declaration.bubble().raw());
    let bubble_identity = format!("bubble:{bubble}");
    let source_fingerprint = source_fingerprint(&adapters);
    let popc = encode_popc(&bubble_identity, &source_fingerprint, &adapters)?;
    let artifacts = RetainedMetadataArtifacts {
        bubble_identity,
        source_fingerprint,
        adapters,
        popc,
    };
    let decoded = decode_retained_adapters_popc(&artifacts.popc)?;
    if decoded != artifacts {
        return Err(RetainedMetadataError::NonCanonical);
    }
    Ok(artifacts)
}

/// Re-loads the canonical descriptor and turns its closed resolved facts into
/// compiler-originated typed HIR. No source spelling or compiler-private
/// projection bypasses the `.popc` verification boundary.
pub(crate) fn generate_codec_adapter_hir(
    artifacts: &RetainedMetadataArtifacts,
    declarations: &[HirDeclaration],
    index: &DeclarationIndex,
    arena: &mut TypeArena,
) -> Result<Vec<HirGeneratedCodecAdapter>, RetainedMetadataError> {
    if artifacts.adapters.is_empty() {
        return Ok(Vec::new());
    }
    let verified = decode_retained_adapters_popc(artifacts.popc())?;
    let declarations = declarations
        .iter()
        .map(|declaration| (declaration.symbol(), declaration))
        .collect::<BTreeMap<_, _>>();
    let mut generated = Vec::with_capacity(verified.adapters.len());
    for adapter in verified.adapters {
        let targets = index.declaration_by_qualified_name(&adapter.target, SymbolSpace::Type);
        let [target] = targets.as_slice() else {
            return Err(RetainedMetadataError::InvalidProjection);
        };
        let schemas = index.declaration_by_qualified_name(&adapter.adapter, SymbolSpace::Value);
        let [schema] = schemas.as_slice() else {
            return Err(RetainedMetadataError::InvalidProjection);
        };
        if schema.kind() != pop_resolve::DeclarationKind::GeneratedCodecSchema
            || schema.module() != target.module()
            || schema.visibility() != target.visibility()
        {
            return Err(RetainedMetadataError::InvalidProjection);
        }
        let declaration = declarations
            .get(&target.symbol())
            .copied()
            .ok_or(RetainedMetadataError::InvalidProjection)?;
        let (target_type, members) = match declaration.kind() {
            HirDeclarationKind::Record(record) => {
                if adapter.kind != RetainedAdapterKind::Record
                    || adapter.members.len() != record.fields().len()
                {
                    return Err(RetainedMetadataError::InvalidProjection);
                }
                let members = adapter
                    .members
                    .iter()
                    .zip(record.fields())
                    .map(|(projected, field)| {
                        if projected.name != field.name()
                            || projected.ordinal as usize >= record.fields().len()
                        {
                            return Err(RetainedMetadataError::InvalidProjection);
                        }
                        Ok(HirGeneratedCodecMember::new(
                            projected.ordinal,
                            projected.name.clone(),
                            HirGeneratedCodecMemberId::Field(field.field()),
                            vec![field.field_type()],
                            None,
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (record.type_id(), members)
            }
            HirDeclarationKind::Enum(enumeration) => {
                if adapter.kind != RetainedAdapterKind::Enum
                    || adapter.members.len() != enumeration.cases().len()
                {
                    return Err(RetainedMetadataError::InvalidProjection);
                }
                let members = adapter
                    .members
                    .iter()
                    .zip(enumeration.cases())
                    .map(|(projected, case)| {
                        if projected.name != case.name()
                            || projected.discriminant != Some(case.discriminant())
                        {
                            return Err(RetainedMetadataError::InvalidProjection);
                        }
                        Ok(HirGeneratedCodecMember::new(
                            projected.ordinal,
                            projected.name.clone(),
                            HirGeneratedCodecMemberId::EnumCase(case.case()),
                            Vec::new(),
                            Some(case.discriminant()),
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (enumeration.type_id(), members)
            }
            HirDeclarationKind::Union(union) => {
                if adapter.kind != RetainedAdapterKind::Union
                    || adapter.members.len() != union.cases().len()
                {
                    return Err(RetainedMetadataError::InvalidProjection);
                }
                let members = adapter
                    .members
                    .iter()
                    .zip(union.cases())
                    .map(|(projected, case)| {
                        if projected.name != case.name() {
                            return Err(RetainedMetadataError::InvalidProjection);
                        }
                        Ok(HirGeneratedCodecMember::new(
                            projected.ordinal,
                            projected.name.clone(),
                            HirGeneratedCodecMemberId::UnionCase(case.case()),
                            case.parameters()
                                .iter()
                                .map(|value| value.type_id())
                                .collect(),
                            None,
                        ))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                (union.type_id(), members)
            }
            _ => return Err(RetainedMetadataError::InvalidProjection),
        };
        let schema_type = arena
            .intern(SemanticType::Builtin {
                definition: CODEC_SCHEMA_DEFINITION,
                arguments: vec![target_type],
            })
            .map_err(|_| RetainedMetadataError::InvalidProjection)?;
        let target_span = declaration.span();
        let provenance = HirGeneratedCodecProvenance::new(
            SourceSpan::new(
                target_span.file(),
                TextRange::new(
                    TextSize::from_u32(adapter.target_start),
                    TextSize::from_u32(adapter.target_end),
                )
                .ok_or(RetainedMetadataError::InvalidProjection)?,
            ),
            SourceSpan::new(
                target_span.file(),
                TextRange::new(
                    TextSize::from_u32(adapter.attachment_start),
                    TextSize::from_u32(adapter.attachment_end),
                )
                .ok_or(RetainedMetadataError::InvalidProjection)?,
            ),
        );
        let schema_identity = SymbolIdentity::new(target.bubble(), schema.symbol());
        let (encode_entry, decode_entry) = generated_codec_entries(
            schema.symbol(),
            schema_identity,
            target_type,
            provenance,
            arena,
        )?;
        generated.push(HirGeneratedCodecAdapter::new(
            schema.symbol(),
            SymbolIdentity::new(target.bubble(), target.symbol()),
            target.module(),
            target.visibility(),
            adapter.adapter_name,
            adapter.target_name,
            target_type,
            schema_type,
            adapter.schema_version,
            adapter.projection_sha256,
            provenance,
            encode_entry,
            decode_entry,
            members,
        ));
    }
    generated.sort_by_key(HirGeneratedCodecAdapter::symbol);
    Ok(generated)
}

/// Attaches public adapter structure imported from verified direct-dependency
/// `.poplib` descriptors. Structural facts come only from canonical `.popc`;
/// public JSON metadata supplies identities and exact full-file/entry digests.
pub(crate) fn generate_reference_codec_adapter_hir(
    metadata: &[ReferenceMetadata],
    descriptors: &[(BubbleId, Vec<u8>)],
    index: &DeclarationIndex,
    reference_record_types: &BTreeMap<SymbolIdentity, TypeId>,
    resolver: &mut SignatureResolver<'_>,
) -> Result<Vec<HirGeneratedCodecAdapter>, RetainedMetadataError> {
    let descriptor_by_bubble = verified_reference_descriptors(metadata, descriptors)?;
    let mut imported = Vec::new();
    for metadata in metadata {
        let Some(bytes) = descriptor_by_bubble.get(&metadata.bubble()).copied() else {
            continue;
        };
        let decoded = decode_retained_adapters_popc(bytes)?;
        for reference in metadata.retained_adapters() {
            imported.push((
                reference.clone(),
                matching_reference_entry(reference, &decoded)?.clone(),
            ));
        }
    }
    let mut target_types = reference_record_types.clone();
    let mut types_by_name = metadata
        .iter()
        .flat_map(ReferenceMetadata::records)
        .filter_map(|record| {
            reference_record_types
                .get(&record.identity())
                .copied()
                .map(|type_id| {
                    let name = if record.namespace().is_empty() {
                        record.name().to_owned()
                    } else {
                        format!("{}.{}", record.namespace(), record.name())
                    };
                    (name, type_id)
                })
        })
        .collect::<BTreeMap<_, _>>();
    let mut pending = imported
        .iter()
        .map(|(reference, entry)| (reference, entry))
        .collect::<Vec<_>>();
    while !pending.is_empty() {
        let mut remaining = Vec::new();
        let mut progressed = false;
        for (reference, entry) in pending {
            let identity = reference.identity().target();
            if target_types.contains_key(&identity) {
                continue;
            }
            let target = index
                .declaration_by_reference_identity(identity)
                .ok_or(RetainedMetadataError::InvalidProjection)?;
            let definition = match entry.kind {
                RetainedAdapterKind::Record => {
                    let Some(fields) = entry
                        .members
                        .iter()
                        .map(|member| {
                            imported_projection_type(
                                &member.projected_type,
                                &types_by_name,
                                resolver.arena_mut(),
                            )
                            .ok()
                            .map(|type_id| (member.name.clone(), type_id))
                        })
                        .collect::<Option<Vec<_>>>()
                    else {
                        remaining.push((reference, entry));
                        continue;
                    };
                    resolver
                        .define_referenced_record(target.symbol(), fields, false, target.span())
                        .map(|definition| definition.type_id())
                }
                RetainedAdapterKind::Enum => resolver
                    .define_referenced_enum(
                        target.symbol(),
                        entry
                            .members
                            .iter()
                            .map(|member| {
                                member
                                    .discriminant
                                    .map(|value| (member.name.clone(), value))
                            })
                            .collect::<Option<Vec<_>>>()
                            .ok_or(RetainedMetadataError::InvalidProjection)?,
                        target.span(),
                    )
                    .map(|definition| definition.type_id()),
                RetainedAdapterKind::Union => {
                    let Some(cases) = entry
                        .members
                        .iter()
                        .map(|member| {
                            let RetainedProjectionType::Tuple(payloads) = &member.projected_type
                            else {
                                return None;
                            };
                            payloads
                                .iter()
                                .enumerate()
                                .map(|(index, payload)| {
                                    imported_projection_type(
                                        payload,
                                        &types_by_name,
                                        resolver.arena_mut(),
                                    )
                                    .ok()
                                    .map(|type_id| (format!("payload{index}"), type_id))
                                })
                                .collect::<Option<Vec<_>>>()
                                .map(|payloads| (member.name.clone(), payloads))
                        })
                        .collect::<Option<Vec<_>>>()
                    else {
                        remaining.push((reference, entry));
                        continue;
                    };
                    resolver
                        .define_referenced_union(target.symbol(), cases, target.span())
                        .map(|definition| definition.type_id())
                }
            };
            let type_id = definition.ok_or(RetainedMetadataError::InvalidProjection)?;
            target_types.insert(identity, type_id);
            types_by_name.insert(entry.target.clone(), type_id);
            progressed = true;
        }
        if !progressed && !remaining.is_empty() {
            return Err(RetainedMetadataError::InvalidProjection);
        }
        pending = remaining;
    }

    let mut generated = Vec::new();
    for (reference, entry) in imported {
        let target_identity = reference.identity().target();
        let target_type = target_types
            .get(&target_identity)
            .copied()
            .ok_or(RetainedMetadataError::InvalidProjection)?;
        let target = index
            .declaration_by_reference_identity(target_identity)
            .ok_or(RetainedMetadataError::InvalidProjection)?;
        let schema = index
            .declaration_by_reference_identity(reference.identity().adapter())
            .ok_or(RetainedMetadataError::InvalidProjection)?;
        let members = match entry.kind {
            RetainedAdapterKind::Record => resolver
                .record_definition(target.symbol())
                .filter(|definition| definition.fields().len() == entry.members.len())
                .ok_or(RetainedMetadataError::InvalidProjection)?
                .fields()
                .iter()
                .zip(&entry.members)
                .map(|(field, member)| {
                    Ok(HirGeneratedCodecMember::new(
                        member.ordinal,
                        member.name.clone(),
                        HirGeneratedCodecMemberId::Field(field.field()),
                        vec![field.field_type()],
                        None,
                    ))
                })
                .collect::<Result<Vec<_>, RetainedMetadataError>>()?,
            RetainedAdapterKind::Enum => resolver
                .enum_definition(target.symbol())
                .filter(|definition| definition.cases().len() == entry.members.len())
                .ok_or(RetainedMetadataError::InvalidProjection)?
                .cases()
                .iter()
                .zip(&entry.members)
                .map(|(case, member)| {
                    Ok(HirGeneratedCodecMember::new(
                        member.ordinal,
                        member.name.clone(),
                        HirGeneratedCodecMemberId::EnumCase(case.case()),
                        Vec::new(),
                        Some(case.discriminant()),
                    ))
                })
                .collect::<Result<Vec<_>, RetainedMetadataError>>()?,
            RetainedAdapterKind::Union => resolver
                .union_definition(target.symbol())
                .filter(|definition| definition.cases().len() == entry.members.len())
                .ok_or(RetainedMetadataError::InvalidProjection)?
                .cases()
                .iter()
                .zip(&entry.members)
                .map(|(case, member)| {
                    Ok(HirGeneratedCodecMember::new(
                        member.ordinal,
                        member.name.clone(),
                        HirGeneratedCodecMemberId::UnionCase(case.case()),
                        case.parameters()
                            .iter()
                            .map(|(_, type_id, _)| *type_id)
                            .collect(),
                        None,
                    ))
                })
                .collect::<Result<Vec<_>, RetainedMetadataError>>()?,
        };
        let schema_type = resolver
            .arena_mut()
            .intern(SemanticType::Builtin {
                definition: reference.schema_definition(),
                arguments: vec![target_type],
            })
            .map_err(|_| RetainedMetadataError::InvalidProjection)?;
        let target_span = target.span();
        let provenance = HirGeneratedCodecProvenance::new(
            SourceSpan::new(
                target_span.file(),
                TextRange::new(
                    TextSize::from_u32(entry.target_start),
                    TextSize::from_u32(entry.target_end),
                )
                .ok_or(RetainedMetadataError::InvalidProjection)?,
            ),
            SourceSpan::new(
                target_span.file(),
                TextRange::new(
                    TextSize::from_u32(entry.attachment_start),
                    TextSize::from_u32(entry.attachment_end),
                )
                .ok_or(RetainedMetadataError::InvalidProjection)?,
            ),
        );
        let schema_identity = reference.identity().adapter();
        let (encode_entry, decode_entry) = generated_codec_entries(
            schema.symbol(),
            schema_identity,
            target_type,
            provenance,
            resolver.arena_mut(),
        )?;
        generated.push(HirGeneratedCodecAdapter::new(
            schema.symbol(),
            target_identity,
            reference.module(),
            Visibility::Public,
            reference.name().to_owned(),
            entry.target_name,
            target_type,
            schema_type,
            entry.schema_version,
            entry.projection_sha256,
            provenance,
            encode_entry,
            decode_entry,
            members,
        ));
    }
    generated.sort_by_key(HirGeneratedCodecAdapter::symbol);
    Ok(generated)
}

fn imported_projection_type(
    projected: &RetainedProjectionType,
    record_types: &BTreeMap<String, TypeId>,
    arena: &mut TypeArena,
) -> Result<TypeId, RetainedMetadataError> {
    let source = |name: &str, arena: &TypeArena| {
        arena
            .source_type(name)
            .ok_or(RetainedMetadataError::InvalidProjection)
    };
    match projected {
        RetainedProjectionType::Boolean => source("Boolean", arena),
        RetainedProjectionType::Int8 => source("Int8", arena),
        RetainedProjectionType::Int16 => source("Int16", arena),
        RetainedProjectionType::Int32 => source("Int32", arena),
        RetainedProjectionType::Int64 => source("Int", arena),
        RetainedProjectionType::UInt8 => source("Byte", arena),
        RetainedProjectionType::UInt16 => source("UInt16", arena),
        RetainedProjectionType::UInt32 => source("UInt32", arena),
        RetainedProjectionType::UInt64 => source("UInt64", arena),
        RetainedProjectionType::Float32 => source("Float32", arena),
        RetainedProjectionType::Float64 => source("Float64", arena),
        RetainedProjectionType::String => source("String", arena),
        RetainedProjectionType::Bytes => arena
            .intern(SemanticType::Builtin {
                definition: BuiltinTypeId::from_raw(0),
                arguments: Vec::new(),
            })
            .map_err(|_| RetainedMetadataError::InvalidProjection),
        RetainedProjectionType::Optional(inner) => {
            let inner = imported_projection_type(inner, record_types, arena)?;
            arena
                .optional(inner)
                .map_err(|_| RetainedMetadataError::InvalidProjection)
        }
        RetainedProjectionType::Tuple(values) => {
            let values = values
                .iter()
                .map(|value| imported_projection_type(value, record_types, arena))
                .collect::<Result<Vec<_>, _>>()?;
            arena
                .intern(SemanticType::Tuple(values))
                .map_err(|_| RetainedMetadataError::InvalidProjection)
        }
        RetainedProjectionType::Array(inner) => {
            let inner = imported_projection_type(inner, record_types, arena)?;
            arena
                .intern(SemanticType::Array(inner))
                .map_err(|_| RetainedMetadataError::InvalidProjection)
        }
        RetainedProjectionType::List(inner) => {
            let inner = imported_projection_type(inner, record_types, arena)?;
            arena
                .intern(SemanticType::Builtin {
                    definition: BuiltinTypeId::from_raw(101),
                    arguments: vec![inner],
                })
                .map_err(|_| RetainedMetadataError::InvalidProjection)
        }
        RetainedProjectionType::Nominal { target, .. } => record_types
            .get(target)
            .copied()
            .ok_or(RetainedMetadataError::InvalidProjection),
    }
}

struct ProjectionBuilder<'a> {
    requests: &'a BTreeMap<SymbolId, RetainedMetadataRequest>,
    declarations: &'a BTreeMap<SymbolId, &'a HirDeclaration>,
    declarations_by_type: &'a BTreeMap<TypeId, SymbolId>,
    index: &'a DeclarationIndex,
    arena: &'a TypeArena,
    module_origins: &'a BTreeMap<ModuleId, (String, SourceSpan)>,
    complete: BTreeMap<SymbolId, RetainedMetadataAdapter>,
    active: BTreeSet<SymbolId>,
    nodes: usize,
    label_bytes: usize,
}

impl ProjectionBuilder<'_> {
    fn build_adapter(
        &mut self,
        symbol: SymbolId,
        depth: usize,
    ) -> Result<RetainedMetadataAdapter, &'static str> {
        if let Some(adapter) = self.complete.get(&symbol) {
            return Ok(adapter.clone());
        }
        if depth > MAX_PROJECTION_DEPTH || !self.active.insert(symbol) {
            return Err("RetainMetadata recursive nominal schema cycle or depth limit");
        }
        let request = self
            .requests
            .get(&symbol)
            .copied()
            .ok_or("RetainMetadata nested nominal type has no compatible request")?;
        let declaration = self
            .declarations
            .get(&symbol)
            .copied()
            .ok_or("RetainMetadata target declaration is unavailable")?;
        let indexed = self
            .index
            .declaration(symbol)
            .ok_or("RetainMetadata target identity is unavailable")?;
        let source_module = self
            .module_origins
            .get(&request.module)
            .map(|(path, _)| path.clone())
            .ok_or("RetainMetadata source provenance is unavailable")?;
        check_text(&source_module)?;
        let target = indexed.qualified_name();
        check_text(&target)?;
        let target_name = declaration.name().to_owned();
        let adapter_name = format!("{target_name}Schema");
        let adapter = if indexed.namespace().is_empty() {
            adapter_name.clone()
        } else {
            format!("{}.{}", indexed.namespace(), adapter_name)
        };
        check_text(&adapter)?;
        let (kind, members) = match declaration.kind() {
            HirDeclarationKind::Record(record) => {
                Self::check_member_count(record.fields().len())?;
                let mut members = Vec::new();
                for (ordinal, field) in record.fields().iter().enumerate() {
                    self.add_label(field.name())?;
                    members.push(RetainedAdapterMember {
                        ordinal: u16::try_from(ordinal)
                            .map_err(|_| "RetainMetadata ordinal limit")?,
                        name: field.name().to_owned(),
                        projected_type: self.project_type(
                            field.field_type(),
                            declaration,
                            depth + 1,
                        )?,
                        discriminant: None,
                    });
                }
                (RetainedAdapterKind::Record, members)
            }
            HirDeclarationKind::Enum(enumeration) => {
                Self::check_member_count(enumeration.cases().len())?;
                let mut members = Vec::new();
                for (ordinal, case) in enumeration.cases().iter().enumerate() {
                    self.add_label(case.name())?;
                    members.push(RetainedAdapterMember {
                        ordinal: u16::try_from(ordinal)
                            .map_err(|_| "RetainMetadata ordinal limit")?,
                        name: case.name().to_owned(),
                        projected_type: RetainedProjectionType::Tuple(Vec::new()),
                        discriminant: Some(case.discriminant()),
                    });
                }
                (RetainedAdapterKind::Enum, members)
            }
            HirDeclarationKind::Union(union) => {
                Self::check_member_count(union.cases().len())?;
                let payload_count = union
                    .cases()
                    .iter()
                    .try_fold(0_usize, |total, case| {
                        total.checked_add(case.parameters().len())
                    })
                    .ok_or("RetainMetadata payload limit")?;
                if payload_count > MAX_MEMBERS {
                    return Err("RetainMetadata payload limit exceeded");
                }
                let mut members = Vec::new();
                for (ordinal, case) in union.cases().iter().enumerate() {
                    self.add_label(case.name())?;
                    let payloads = case
                        .parameters()
                        .iter()
                        .map(|parameter| {
                            self.project_type(parameter.type_id(), declaration, depth + 1)
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    members.push(RetainedAdapterMember {
                        ordinal: u16::try_from(ordinal)
                            .map_err(|_| "RetainMetadata ordinal limit")?,
                        name: case.name().to_owned(),
                        projected_type: RetainedProjectionType::Tuple(payloads),
                        discriminant: None,
                    });
                }
                (RetainedAdapterKind::Union, members)
            }
            _ => return Err("RetainMetadata target is not a closed data declaration"),
        };
        let mut completed = RetainedMetadataAdapter {
            target,
            target_name,
            adapter,
            adapter_name,
            source_module,
            visibility: declaration.visibility(),
            kind,
            schema_version: request.schema_version,
            attachment_start: request.attachment_span.range().start().to_u32(),
            attachment_end: request.attachment_span.range().end().to_u32(),
            target_start: declaration.span().range().start().to_u32(),
            target_end: declaration.span().range().end().to_u32(),
            projection_sha256: String::new(),
            members,
        };
        completed.projection_sha256 = projection_fingerprint(
            &format!("bubble:{}", declaration.bubble().raw()),
            &completed,
        );
        self.active.remove(&symbol);
        self.complete.insert(symbol, completed.clone());
        Ok(completed)
    }

    fn project_type(
        &mut self,
        type_id: TypeId,
        owner: &HirDeclaration,
        depth: usize,
    ) -> Result<RetainedProjectionType, &'static str> {
        self.nodes = self
            .nodes
            .checked_add(1)
            .ok_or("RetainMetadata projection node limit")?;
        if self.nodes > MAX_PROJECTION_NODES || depth > MAX_PROJECTION_DEPTH {
            return Err("RetainMetadata projection node or depth limit exceeded");
        }
        let semantic = self
            .arena
            .get(type_id)
            .ok_or("RetainMetadata field type is unavailable")?;
        match semantic {
            SemanticType::Primitive(PrimitiveType::Boolean) => Ok(RetainedProjectionType::Boolean),
            SemanticType::Primitive(PrimitiveType::Integer(kind)) => Ok(match kind {
                IntegerKind::Int8 => RetainedProjectionType::Int8,
                IntegerKind::Int16 => RetainedProjectionType::Int16,
                IntegerKind::Int32 => RetainedProjectionType::Int32,
                IntegerKind::Int64 => RetainedProjectionType::Int64,
                IntegerKind::UInt8 => RetainedProjectionType::UInt8,
                IntegerKind::UInt16 => RetainedProjectionType::UInt16,
                IntegerKind::UInt32 => RetainedProjectionType::UInt32,
                IntegerKind::UInt64 => RetainedProjectionType::UInt64,
            }),
            SemanticType::Primitive(PrimitiveType::Float32) => Ok(RetainedProjectionType::Float32),
            SemanticType::Primitive(PrimitiveType::Float64) => Ok(RetainedProjectionType::Float64),
            SemanticType::Primitive(PrimitiveType::String) => Ok(RetainedProjectionType::String),
            SemanticType::Tuple(values) => Ok(RetainedProjectionType::Tuple(
                values
                    .iter()
                    .map(|value| self.project_type(*value, owner, depth + 1))
                    .collect::<Result<Vec<_>, _>>()?,
            )),
            SemanticType::Array(value) => Ok(RetainedProjectionType::Array(Box::new(
                self.project_type(*value, owner, depth + 1)?,
            ))),
            SemanticType::Optional(value) => Ok(RetainedProjectionType::Optional(Box::new(
                self.project_type(*value, owner, depth + 1)?,
            ))),
            SemanticType::Union(values)
                if values.len() == 2
                    && self
                        .arena
                        .source_type("nil")
                        .is_some_and(|nil| values.contains(&nil)) =>
            {
                let nil = self
                    .arena
                    .source_type("nil")
                    .ok_or("RetainMetadata optional nil type is unavailable")?;
                let value = values
                    .iter()
                    .copied()
                    .find(|value| *value != nil)
                    .ok_or("RetainMetadata optional payload type is unavailable")?;
                Ok(RetainedProjectionType::Optional(Box::new(
                    self.project_type(value, owner, depth + 1)?,
                )))
            }
            SemanticType::Builtin {
                definition,
                arguments,
            } if definition.raw() == 0 && arguments.is_empty() => Ok(RetainedProjectionType::Bytes),
            SemanticType::Builtin {
                definition,
                arguments,
            } if definition.raw() == 101 && arguments.len() == 1 => {
                Ok(RetainedProjectionType::List(Box::new(self.project_type(
                    arguments[0],
                    owner,
                    depth + 1,
                )?)))
            }
            SemanticType::Record(_)
            | SemanticType::TaggedUnion { .. }
            | SemanticType::Enum { .. } => {
                let nested_symbol = self
                    .declarations_by_type
                    .get(&type_id)
                    .copied()
                    .ok_or("RetainMetadata nominal field has no local typed declaration")?;
                let nested_declaration = self
                    .declarations
                    .get(&nested_symbol)
                    .copied()
                    .ok_or("RetainMetadata nominal field declaration is unavailable")?;
                if !visibility_covers(owner, nested_declaration) {
                    return Err("RetainMetadata nested nominal visibility is too narrow");
                }
                let nested = self.build_adapter(nested_symbol, depth + 1)?;
                Ok(RetainedProjectionType::Nominal {
                    target: nested.target,
                    projection_sha256: nested.projection_sha256,
                })
            }
            SemanticType::Builtin { .. } => {
                Err("RetainMetadata schema contains an unsupported built-in type")
            }
            SemanticType::Union(_) => {
                Err("RetainMetadata schema contains an unsupported explicit union type")
            }
            SemanticType::Primitive(_) => {
                Err("RetainMetadata schema contains an unsupported primitive type")
            }
            SemanticType::Function { .. }
            | SemanticType::Table { .. }
            | SemanticType::ErrorUnion { .. }
            | SemanticType::Class { .. }
            | SemanticType::Interface { .. }
            | SemanticType::Attribute { .. }
            | SemanticType::TypeParameter(_)
            | SemanticType::Opaque(_)
            | SemanticType::Error => Err("RetainMetadata schema contains an unsupported type"),
        }
    }

    fn check_member_count(count: usize) -> Result<(), &'static str> {
        (count <= MAX_MEMBERS)
            .then_some(())
            .ok_or("RetainMetadata field or case limit exceeded")
    }

    fn add_label(&mut self, label: &str) -> Result<(), &'static str> {
        check_text(label)?;
        self.label_bytes = self
            .label_bytes
            .checked_add(label.len())
            .ok_or("RetainMetadata label limit")?;
        (self.label_bytes <= MAX_LABEL_BYTES)
            .then_some(())
            .ok_or("RetainMetadata label limit exceeded")
    }
}

fn visibility_covers(owner: &HirDeclaration, nested: &HirDeclaration) -> bool {
    match owner.visibility() {
        Visibility::Public => nested.visibility() == Visibility::Public,
        Visibility::Internal => nested.visibility() != Visibility::Private,
        Visibility::Private => {
            nested.visibility() != Visibility::Private || owner.module() == nested.module()
        }
    }
}

fn add_request_diagnostic(
    request: RetainedMetadataRequest,
    diagnostics: &mut Vec<Diagnostic>,
    reason: &'static str,
) {
    diagnostics.push(
        pop_diagnostics::compile_time::ineligible_constant_expression(
            request.attachment_span,
            reason,
        ),
    );
}

fn check_text(value: &str) -> Result<(), &'static str> {
    (!value.is_empty() && value.len() <= MAX_TEXT_BYTES && !value.contains(['\n', '\r']))
        .then_some(())
        .ok_or("RetainMetadata identifier or path limit")
}

fn projection_fingerprint(bubble_identity: &str, adapter: &RetainedMetadataAdapter) -> String {
    let mut tokens = format!(
        "Pop.Metadata.CodecProjection/1|{}|{}|Metadata.Use.Codec|{}|{}|{}|{}|",
        bubble_identity,
        adapter.target,
        DESCRIPTOR_SCHEMA_VERSION,
        ADAPTER_PROTOCOL_VERSION,
        visibility_name(adapter.visibility),
        adapter.kind.source_name(),
    );
    let _ = write!(tokens, "{}|", adapter.schema_version);
    for member in &adapter.members {
        let _ = write!(
            tokens,
            "{}:{}:{}:",
            member.ordinal,
            member.name.len(),
            member.name
        );
        member.projected_type.fingerprint_token(&mut tokens);
        let _ = write!(tokens, ":{:?}|", member.discriminant);
    }
    hash_text(&tokens)
}

fn hash_text(value: &str) -> String {
    hash_bytes(value.as_bytes())
}

fn hash_bytes(value: &[u8]) -> String {
    Sha256::digest(value)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn source_fingerprint(adapters: &[RetainedMetadataAdapter]) -> String {
    hash_text(
        &adapters
            .iter()
            .map(|adapter| adapter.projection_sha256.as_str())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn visibility_name(visibility: Visibility) -> &'static str {
    match visibility {
        Visibility::Public => "Public",
        Visibility::Internal => "Internal",
        Visibility::Private => "Private",
    }
}

fn encode_popc(
    bubble_identity: &str,
    source_fingerprint: &str,
    adapters: &[RetainedMetadataAdapter],
) -> Result<Vec<u8>, RetainedMetadataError> {
    let mut output = String::new();
    writeln!(output, "@Metadata.GeneratedAdapters(").unwrap();
    writeln!(output, "    schemaVersion = {DESCRIPTOR_SCHEMA_VERSION},").unwrap();
    writeln!(
        output,
        "    adapterProtocolVersion = {ADAPTER_PROTOCOL_VERSION},"
    )
    .unwrap();
    writeln!(output, "    producerName = \"pop\",").unwrap();
    writeln!(
        output,
        "    producerVersion = \"{}\",",
        env!("CARGO_PKG_VERSION")
    )
    .unwrap();
    writeln!(
        output,
        "    bubbleIdentity = \"{}\",",
        escape_string(bubble_identity)
    )
    .unwrap();
    writeln!(output, "    sourceFingerprint = \"{source_fingerprint}\",").unwrap();
    writeln!(output, ")").unwrap();
    writeln!(output, "namespace Pop.Generated.Metadata").unwrap();
    for (index, adapter) in adapters.iter().enumerate() {
        writeln!(output).unwrap();
        writeln!(output, "@Metadata.CodecSchema(").unwrap();
        writeln!(output, "    target = {},", adapter.target).unwrap();
        writeln!(output, "    adapter = {},", adapter.adapter).unwrap();
        writeln!(output, "    schemaVersion = {},", adapter.schema_version).unwrap();
        writeln!(
            output,
            "    visibility = Metadata.Visibility.{},",
            visibility_name(adapter.visibility)
        )
        .unwrap();
        writeln!(
            output,
            "    projectionSha256 = \"{}\",",
            adapter.projection_sha256
        )
        .unwrap();
        writeln!(
            output,
            "    sourceModule = \"{}\",",
            escape_string(&adapter.source_module)
        )
        .unwrap();
        writeln!(
            output,
            "    attachmentStart = {},",
            adapter.attachment_start
        )
        .unwrap();
        writeln!(output, "    attachmentEnd = {},", adapter.attachment_end).unwrap();
        writeln!(output, "    targetStart = {},", adapter.target_start).unwrap();
        writeln!(output, "    targetEnd = {},", adapter.target_end).unwrap();
        writeln!(output, ")").unwrap();
        writeln!(
            output,
            "internal {} Schema{index}",
            adapter.kind.source_name()
        )
        .unwrap();
        for member in &adapter.members {
            match adapter.kind {
                RetainedAdapterKind::Record => {
                    writeln!(
                        output,
                        "    @Metadata.Field(source = {}.{}, ordinal = {})",
                        adapter.target, member.name, member.ordinal
                    )
                    .unwrap();
                    writeln!(
                        output,
                        "    {}: {}",
                        member.name,
                        member.projected_type.render()
                    )
                    .unwrap();
                }
                RetainedAdapterKind::Enum => {
                    writeln!(
                        output,
                        "    @Metadata.Case(source = {}.{}, ordinal = {}, discriminant = {})",
                        adapter.target,
                        member.name,
                        member.ordinal,
                        member.discriminant.unwrap_or_default()
                    )
                    .unwrap();
                    writeln!(output, "    {}", member.name).unwrap();
                }
                RetainedAdapterKind::Union => {
                    writeln!(
                        output,
                        "    @Metadata.Case(source = {}.{}, ordinal = {})",
                        adapter.target, member.name, member.ordinal
                    )
                    .unwrap();
                    let RetainedProjectionType::Tuple(payloads) = &member.projected_type else {
                        return Err(RetainedMetadataError::InvalidProjection);
                    };
                    let payloads = payloads
                        .iter()
                        .enumerate()
                        .map(|(index, value)| format!("value{index}: {}", value.render()))
                        .collect::<Vec<_>>()
                        .join(", ");
                    writeln!(output, "    {}({payloads})", member.name).unwrap();
                }
            }
        }
        writeln!(output, "end").unwrap();
    }
    let bytes = output.into_bytes();
    if bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err(RetainedMetadataError::TooLarge);
    }
    Ok(bytes)
}

fn escape_string(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Parses, bounds, fingerprints, and canonicalizes a typed
/// `retained-adapters.popc` descriptor.
///
/// # Errors
///
/// Rejects malformed, oversized, unknown-version, fingerprint-mismatched, or
/// noncanonical input. JSON is not an accepted alternate representation.
pub fn decode_retained_adapters_popc(
    bytes: &[u8],
) -> Result<RetainedMetadataArtifacts, RetainedMetadataError> {
    if bytes.is_empty() || bytes.len() > MAX_DESCRIPTOR_BYTES {
        return Err(RetainedMetadataError::TooLarge);
    }
    let text = std::str::from_utf8(bytes).map_err(|_| RetainedMetadataError::InvalidDescriptor)?;
    if !text.ends_with('\n') || text.contains('\r') || text.trim_start().starts_with('{') {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    let mut lines = text.lines().peekable();
    expect_line(&mut lines, "@Metadata.GeneratedAdapters(")?;
    let descriptor_version = parse_number_line(lines.next(), "    schemaVersion = ", ",")?;
    let protocol_version = parse_number_line(lines.next(), "    adapterProtocolVersion = ", ",")?;
    if descriptor_version != u32::from(DESCRIPTOR_SCHEMA_VERSION)
        || protocol_version != u32::from(ADAPTER_PROTOCOL_VERSION)
    {
        return Err(RetainedMetadataError::UnsupportedVersion);
    }
    expect_line(&mut lines, "    producerName = \"pop\",")?;
    expect_line(
        &mut lines,
        &format!("    producerVersion = \"{}\",", env!("CARGO_PKG_VERSION")),
    )?;
    let bubble_identity = parse_string_line(lines.next(), "    bubbleIdentity = \"", "\",")?;
    let source_fingerprint = parse_string_line(lines.next(), "    sourceFingerprint = \"", "\",")?;
    if !valid_sha256(&source_fingerprint) {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    expect_line(&mut lines, ")")?;
    expect_line(&mut lines, "namespace Pop.Generated.Metadata")?;
    let mut adapters = Vec::new();
    while lines.peek().is_some() {
        expect_line(&mut lines, "")?;
        expect_line(&mut lines, "@Metadata.CodecSchema(")?;
        let target = parse_text_line(lines.next(), "    target = ", ",")?;
        let adapter = parse_text_line(lines.next(), "    adapter = ", ",")?;
        let schema_version = parse_number_line(lines.next(), "    schemaVersion = ", ",")?;
        if schema_version == 0 {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
        let visibility =
            match parse_text_line(lines.next(), "    visibility = Metadata.Visibility.", ",")?
                .as_str()
            {
                "Public" => Visibility::Public,
                "Internal" => Visibility::Internal,
                "Private" => Visibility::Private,
                _ => return Err(RetainedMetadataError::InvalidDescriptor),
            };
        let projection_sha256 =
            parse_string_line(lines.next(), "    projectionSha256 = \"", "\",")?;
        if !valid_sha256(&projection_sha256) {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
        let source_module = parse_string_line(lines.next(), "    sourceModule = \"", "\",")?;
        if !valid_module_path(&source_module) {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
        let attachment_start = parse_number_line(lines.next(), "    attachmentStart = ", ",")?;
        let attachment_end = parse_number_line(lines.next(), "    attachmentEnd = ", ",")?;
        let target_start = parse_number_line(lines.next(), "    targetStart = ", ",")?;
        let target_end = parse_number_line(lines.next(), "    targetEnd = ", ",")?;
        expect_line(&mut lines, ")")?;
        let declaration = lines
            .next()
            .ok_or(RetainedMetadataError::InvalidDescriptor)?;
        let (kind, descriptor_name) = parse_descriptor_declaration(declaration)?;
        if descriptor_name != format!("Schema{}", adapters.len())
            || !valid_qualified_identity(&target)
            || !valid_qualified_identity(&adapter)
        {
            return Err(RetainedMetadataError::NonCanonical);
        }
        let target_name = target
            .rsplit('.')
            .next()
            .ok_or(RetainedMetadataError::InvalidDescriptor)?
            .to_owned();
        let adapter_name = adapter
            .rsplit('.')
            .next()
            .ok_or(RetainedMetadataError::InvalidDescriptor)?
            .to_owned();
        if adapter_name != format!("{target_name}Schema")
            || adapter.strip_suffix(&adapter_name).is_none_or(|namespace| {
                namespace != target.strip_suffix(&target_name).unwrap_or("")
            })
        {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
        let mut members = Vec::new();
        while lines.peek().is_some_and(|line| *line != "end") {
            let attribute = lines
                .next()
                .ok_or(RetainedMetadataError::InvalidDescriptor)?;
            let declaration = lines
                .next()
                .ok_or(RetainedMetadataError::InvalidDescriptor)?;
            members.push(parse_member(kind, &target, attribute, declaration)?);
        }
        expect_line(&mut lines, "end")?;
        if members.len() > MAX_MEMBERS {
            return Err(RetainedMetadataError::LimitExceeded);
        }
        let mut labels = BTreeSet::new();
        let mut payloads = 0_usize;
        for (ordinal, member) in members.iter().enumerate() {
            if usize::from(member.ordinal) != ordinal || !labels.insert(member.name.as_str()) {
                return Err(RetainedMetadataError::NonCanonical);
            }
            if kind == RetainedAdapterKind::Enum {
                if member.discriminant.is_none() {
                    return Err(RetainedMetadataError::InvalidDescriptor);
                }
            } else if member.discriminant.is_some() {
                return Err(RetainedMetadataError::InvalidDescriptor);
            }
            if kind == RetainedAdapterKind::Union {
                let RetainedProjectionType::Tuple(values) = &member.projected_type else {
                    return Err(RetainedMetadataError::InvalidDescriptor);
                };
                payloads = payloads
                    .checked_add(values.len())
                    .ok_or(RetainedMetadataError::LimitExceeded)?;
            }
        }
        if payloads > MAX_MEMBERS {
            return Err(RetainedMetadataError::LimitExceeded);
        }
        adapters.push(RetainedMetadataAdapter {
            target,
            target_name,
            adapter,
            adapter_name,
            source_module,
            visibility,
            kind,
            schema_version,
            attachment_start,
            attachment_end,
            target_start,
            target_end,
            projection_sha256,
            members,
        });
    }
    if adapters.len() > MAX_ADAPTERS {
        return Err(RetainedMetadataError::LimitExceeded);
    }
    if adapters
        .windows(2)
        .any(|pair| pair[0].target >= pair[1].target)
    {
        return Err(RetainedMetadataError::NonCanonical);
    }
    let mut projection_nodes = 0_usize;
    let mut label_bytes = 0_usize;
    for adapter in &adapters {
        for member in &adapter.members {
            label_bytes = label_bytes
                .checked_add(member.name.len())
                .ok_or(RetainedMetadataError::LimitExceeded)?;
            count_projection_nodes(&member.projected_type, 0, &mut projection_nodes)?;
        }
    }
    if projection_nodes > MAX_PROJECTION_NODES || label_bytes > MAX_LABEL_BYTES {
        return Err(RetainedMetadataError::LimitExceeded);
    }
    let fingerprints = adapters
        .iter()
        .map(|adapter| (adapter.target.clone(), adapter.projection_sha256.clone()))
        .collect::<BTreeMap<_, _>>();
    for adapter in &mut adapters {
        for member in &mut adapter.members {
            install_nominal_fingerprints(&mut member.projected_type, &fingerprints)?;
        }
        if projection_fingerprint(&bubble_identity, adapter) != adapter.projection_sha256 {
            return Err(RetainedMetadataError::InvalidDescriptor);
        }
    }
    if self::source_fingerprint(&adapters) != source_fingerprint {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    if adapters.iter().any(|adapter| {
        adapter.attachment_end <= adapter.attachment_start
            || adapter.target_end <= adapter.target_start
    }) {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    let artifacts = RetainedMetadataArtifacts {
        bubble_identity,
        source_fingerprint,
        adapters,
        popc: bytes.to_vec(),
    };
    if encode_popc(
        &artifacts.bubble_identity,
        &artifacts.source_fingerprint,
        &artifacts.adapters,
    )? != bytes
    {
        return Err(RetainedMetadataError::NonCanonical);
    }
    Ok(artifacts)
}

fn expect_line<'a>(
    lines: &mut impl Iterator<Item = &'a str>,
    expected: &str,
) -> Result<(), RetainedMetadataError> {
    (lines.next() == Some(expected))
        .then_some(())
        .ok_or(RetainedMetadataError::InvalidDescriptor)
}

fn parse_text_line(
    line: Option<&str>,
    prefix: &str,
    suffix: &str,
) -> Result<String, RetainedMetadataError> {
    let value = line
        .and_then(|line| line.strip_prefix(prefix))
        .and_then(|line| line.strip_suffix(suffix))
        .ok_or(RetainedMetadataError::InvalidDescriptor)?;
    check_text(value).map_err(|_| RetainedMetadataError::LimitExceeded)?;
    Ok(value.to_owned())
}

fn parse_string_line(
    line: Option<&str>,
    prefix: &str,
    suffix: &str,
) -> Result<String, RetainedMetadataError> {
    let value = parse_text_line(line, prefix, suffix)?;
    if value.contains(['\\', '"']) {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    Ok(value)
}

fn parse_number_line(
    line: Option<&str>,
    prefix: &str,
    suffix: &str,
) -> Result<u32, RetainedMetadataError> {
    let value = parse_text_line(line, prefix, suffix)?;
    if value.len() > 1 && value.starts_with('0') {
        return Err(RetainedMetadataError::NonCanonical);
    }
    value
        .parse()
        .map_err(|_| RetainedMetadataError::InvalidDescriptor)
}

fn parse_descriptor_declaration(
    line: &str,
) -> Result<(RetainedAdapterKind, String), RetainedMetadataError> {
    let rest = line
        .strip_prefix("internal ")
        .ok_or(RetainedMetadataError::InvalidDescriptor)?;
    let (kind, name) = rest
        .split_once(' ')
        .ok_or(RetainedMetadataError::InvalidDescriptor)?;
    let kind = match kind {
        "record" => RetainedAdapterKind::Record,
        "enum" => RetainedAdapterKind::Enum,
        "union" => RetainedAdapterKind::Union,
        _ => return Err(RetainedMetadataError::InvalidDescriptor),
    };
    if !name.starts_with("Schema") || name[6..].parse::<usize>().is_err() {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    Ok((kind, name.to_owned()))
}

fn parse_member(
    kind: RetainedAdapterKind,
    target: &str,
    attribute: &str,
    declaration: &str,
) -> Result<RetainedAdapterMember, RetainedMetadataError> {
    let source_prefix = match kind {
        RetainedAdapterKind::Record => "    @Metadata.Field(source = ",
        RetainedAdapterKind::Enum | RetainedAdapterKind::Union => "    @Metadata.Case(source = ",
    };
    let rest = attribute
        .strip_prefix(source_prefix)
        .and_then(|value| value.strip_suffix(')'))
        .ok_or(RetainedMetadataError::InvalidDescriptor)?;
    let (source, rest) = rest
        .split_once(", ordinal = ")
        .ok_or(RetainedMetadataError::InvalidDescriptor)?;
    let name = source
        .strip_prefix(&format!("{target}."))
        .ok_or(RetainedMetadataError::InvalidDescriptor)?
        .to_owned();
    let (ordinal, discriminant) =
        if let Some((ordinal, discriminant)) = rest.split_once(", discriminant = ") {
            (
                ordinal
                    .parse::<u16>()
                    .map_err(|_| RetainedMetadataError::InvalidDescriptor)?,
                Some(
                    discriminant
                        .parse::<u32>()
                        .map_err(|_| RetainedMetadataError::InvalidDescriptor)?,
                ),
            )
        } else {
            (
                rest.parse::<u16>()
                    .map_err(|_| RetainedMetadataError::InvalidDescriptor)?,
                None,
            )
        };
    let projected_type = match kind {
        RetainedAdapterKind::Record => {
            let (declared_name, declared_type) = declaration
                .strip_prefix("    ")
                .and_then(|value| value.split_once(": "))
                .ok_or(RetainedMetadataError::InvalidDescriptor)?;
            if declared_name != name {
                return Err(RetainedMetadataError::InvalidDescriptor);
            }
            parse_projection_type(declared_type)?
        }
        RetainedAdapterKind::Enum => {
            if declaration != format!("    {name}") || discriminant.is_none() {
                return Err(RetainedMetadataError::InvalidDescriptor);
            }
            RetainedProjectionType::Tuple(Vec::new())
        }
        RetainedAdapterKind::Union => {
            let body = declaration
                .strip_prefix(&format!("    {name}("))
                .and_then(|value| value.strip_suffix(')'))
                .ok_or(RetainedMetadataError::InvalidDescriptor)?;
            let payloads = split_top_level(body)?
                .into_iter()
                .enumerate()
                .map(|(index, payload)| {
                    let prefix = format!("value{index}: ");
                    parse_projection_type(
                        payload
                            .strip_prefix(&prefix)
                            .ok_or(RetainedMetadataError::InvalidDescriptor)?,
                    )
                })
                .collect::<Result<Vec<_>, _>>()?;
            RetainedProjectionType::Tuple(payloads)
        }
    };
    Ok(RetainedAdapterMember {
        ordinal,
        name,
        projected_type,
        discriminant,
    })
}

fn parse_projection_type(value: &str) -> Result<RetainedProjectionType, RetainedMetadataError> {
    let mut nodes = 0_usize;
    parse_projection_type_at(value, 0, &mut nodes)
}

fn parse_projection_type_at(
    value: &str,
    depth: usize,
    nodes: &mut usize,
) -> Result<RetainedProjectionType, RetainedMetadataError> {
    *nodes = nodes
        .checked_add(1)
        .ok_or(RetainedMetadataError::LimitExceeded)?;
    if depth > MAX_PROJECTION_DEPTH || *nodes > MAX_PROJECTION_NODES {
        return Err(RetainedMetadataError::LimitExceeded);
    }
    let leaf = match value {
        "Boolean" => Some(RetainedProjectionType::Boolean),
        "Int8" => Some(RetainedProjectionType::Int8),
        "Int16" => Some(RetainedProjectionType::Int16),
        "Int32" => Some(RetainedProjectionType::Int32),
        "Int64" => Some(RetainedProjectionType::Int64),
        "UInt8" => Some(RetainedProjectionType::UInt8),
        "UInt16" => Some(RetainedProjectionType::UInt16),
        "UInt32" => Some(RetainedProjectionType::UInt32),
        "UInt64" => Some(RetainedProjectionType::UInt64),
        "Float32" => Some(RetainedProjectionType::Float32),
        "Float64" => Some(RetainedProjectionType::Float64),
        "String" => Some(RetainedProjectionType::String),
        "Bytes" => Some(RetainedProjectionType::Bytes),
        _ => None,
    };
    if let Some(leaf) = leaf {
        return Ok(leaf);
    }
    if let Some(inner) = value.strip_suffix('?') {
        return Ok(RetainedProjectionType::Optional(Box::new(
            parse_projection_type_at(inner, depth + 1, nodes)?,
        )));
    }
    if let Some(inner) = value
        .strip_prefix("Array<")
        .and_then(|value| value.strip_suffix('>'))
    {
        return Ok(RetainedProjectionType::Array(Box::new(
            parse_projection_type_at(inner, depth + 1, nodes)?,
        )));
    }
    if let Some(inner) = value
        .strip_prefix("List<")
        .and_then(|value| value.strip_suffix('>'))
    {
        return Ok(RetainedProjectionType::List(Box::new(
            parse_projection_type_at(inner, depth + 1, nodes)?,
        )));
    }
    if let Some(inner) = value
        .strip_prefix('(')
        .and_then(|value| value.strip_suffix(')'))
    {
        return Ok(RetainedProjectionType::Tuple(
            split_top_level(inner)?
                .into_iter()
                .map(|value| parse_projection_type_at(value, depth + 1, nodes))
                .collect::<Result<Vec<_>, _>>()?,
        ));
    }
    check_text(value).map_err(|_| RetainedMetadataError::InvalidDescriptor)?;
    if !valid_qualified_identity(value) {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    Ok(RetainedProjectionType::Nominal {
        target: value.to_owned(),
        projection_sha256: String::new(),
    })
}

fn count_projection_nodes(
    projected: &RetainedProjectionType,
    depth: usize,
    nodes: &mut usize,
) -> Result<(), RetainedMetadataError> {
    *nodes = nodes
        .checked_add(1)
        .ok_or(RetainedMetadataError::LimitExceeded)?;
    if depth > MAX_PROJECTION_DEPTH || *nodes > MAX_PROJECTION_NODES {
        return Err(RetainedMetadataError::LimitExceeded);
    }
    match projected {
        RetainedProjectionType::Optional(value)
        | RetainedProjectionType::Array(value)
        | RetainedProjectionType::List(value) => count_projection_nodes(value, depth + 1, nodes),
        RetainedProjectionType::Tuple(values) => {
            for value in values {
                count_projection_nodes(value, depth + 1, nodes)?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_qualified_identity(value: &str) -> bool {
    value.split('.').all(|component| {
        let mut bytes = component.bytes();
        bytes
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic() || byte == b'_')
            && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
    })
}

fn valid_module_path(value: &str) -> bool {
    !value.starts_with('/')
        && !value.contains(['\\', '\0'])
        && value
            .split('/')
            .all(|component| !component.is_empty() && component != "." && component != "..")
}

fn split_top_level(value: &str) -> Result<Vec<&str>, RetainedMetadataError> {
    if value.is_empty() {
        return Ok(Vec::new());
    }
    let mut depth = 0_u16;
    let mut start = 0_usize;
    let mut values = Vec::new();
    let bytes = value.as_bytes();
    for (index, byte) in bytes.iter().enumerate() {
        match byte {
            b'<' | b'(' => {
                depth = depth
                    .checked_add(1)
                    .ok_or(RetainedMetadataError::TooLarge)?;
            }
            b'>' | b')' => {
                depth = depth
                    .checked_sub(1)
                    .ok_or(RetainedMetadataError::InvalidDescriptor)?;
            }
            b',' if depth == 0 => {
                values.push(value[start..index].trim());
                start = index + 1;
            }
            _ => {}
        }
    }
    if depth != 0 {
        return Err(RetainedMetadataError::InvalidDescriptor);
    }
    values.push(value[start..].trim());
    Ok(values)
}

fn install_nominal_fingerprints(
    projected: &mut RetainedProjectionType,
    fingerprints: &BTreeMap<String, String>,
) -> Result<(), RetainedMetadataError> {
    match projected {
        RetainedProjectionType::Optional(value)
        | RetainedProjectionType::Array(value)
        | RetainedProjectionType::List(value) => install_nominal_fingerprints(value, fingerprints),
        RetainedProjectionType::Tuple(values) => {
            for value in values {
                install_nominal_fingerprints(value, fingerprints)?;
            }
            Ok(())
        }
        RetainedProjectionType::Nominal {
            target,
            projection_sha256,
        } => {
            *projection_sha256 = fingerprints
                .get(target)
                .cloned()
                .ok_or(RetainedMetadataError::InvalidDescriptor)?;
            Ok(())
        }
        _ => Ok(()),
    }
}
