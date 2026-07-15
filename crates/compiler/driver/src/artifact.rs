//! Canonical, bounded physical encodings for verified compiler artifacts.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Component, Path, PathBuf};

use pop_foundation::SymbolIdentity;
use pop_hir::{HirDeclaration, HirFunction, HirMethod};
use pop_projects::{BubbleKind, NativeLinkPlan};
use pop_types::TypeArena;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::ResolvedNativeProvider;
use crate::api::ReferenceMetadata;
use crate::reference::{invalid_reference_capsule, invalid_reference_foreign_contract};

const REFERENCE_SCHEMA_VERSION: u16 = 1;
const MAX_REFERENCE_BYTES: usize = 16 * 1024 * 1024;
const POPLIB_MANIFEST_SCHEMA_VERSION: u16 = 3;
const MAX_MANIFEST_BYTES: usize = 4 * 1024 * 1024;
const MAX_ARTIFACT_FILE_BYTES: usize = 256 * 1024 * 1024;
const MAX_ARTIFACT_FILES: usize = 256;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReferenceMetadataFile {
    schema_version: u16,
    metadata: ReferenceMetadata,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceMetadataDecodeError {
    InvalidJson,
    NonCanonical,
    UnsupportedSchema,
    TooLarge,
    InvalidCapsule(SymbolIdentity),
    InvalidForeignDeclaration(SymbolIdentity),
}

impl fmt::Display for ReferenceMetadataDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid reference.metadata: {self:?}")
    }
}

impl Error for ReferenceMetadataDecodeError {}

/// Encodes verified logical reference metadata as canonical JSON.
///
/// # Errors
///
/// Rejects invalid specialization capsules or payloads above the fixed bound.
pub fn encode_reference_metadata(
    metadata: &ReferenceMetadata,
) -> Result<Vec<u8>, ReferenceMetadataDecodeError> {
    validate_metadata(metadata)?;
    let file = ReferenceMetadataFile {
        schema_version: REFERENCE_SCHEMA_VERSION,
        metadata: metadata.clone(),
    };
    let mut bytes =
        serde_json::to_vec(&file).map_err(|_| ReferenceMetadataDecodeError::InvalidJson)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_REFERENCE_BYTES {
        return Err(ReferenceMetadataDecodeError::TooLarge);
    }
    Ok(bytes)
}

/// Parses and verifies canonical reference metadata without source lookup.
///
/// # Errors
///
/// Rejects malformed, noncanonical, unsupported, oversized, or invalid input.
pub fn decode_reference_metadata(
    bytes: &[u8],
) -> Result<ReferenceMetadata, ReferenceMetadataDecodeError> {
    if bytes.len() > MAX_REFERENCE_BYTES {
        return Err(ReferenceMetadataDecodeError::TooLarge);
    }
    let file: ReferenceMetadataFile =
        serde_json::from_slice(bytes).map_err(|_| ReferenceMetadataDecodeError::InvalidJson)?;
    if file.schema_version != REFERENCE_SCHEMA_VERSION {
        return Err(ReferenceMetadataDecodeError::UnsupportedSchema);
    }
    validate_metadata(&file.metadata)?;
    let canonical = encode_reference_metadata(&file.metadata)?;
    if canonical != bytes {
        return Err(ReferenceMetadataDecodeError::NonCanonical);
    }
    Ok(file.metadata)
}

fn validate_metadata(metadata: &ReferenceMetadata) -> Result<(), ReferenceMetadataDecodeError> {
    if let Some(identity) = invalid_reference_capsule(std::slice::from_ref(metadata)) {
        return Err(ReferenceMetadataDecodeError::InvalidCapsule(identity));
    }
    if let Some(identity) = invalid_reference_foreign_contract(std::slice::from_ref(metadata)) {
        return Err(ReferenceMetadataDecodeError::InvalidForeignDeclaration(
            identity,
        ));
    }
    Ok(())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CapsulePayload<'payload> {
    root: SymbolIdentity,
    declarations: &'payload [HirDeclaration],
    functions: &'payload [HirFunction],
    methods: &'payload [HirMethod],
    source_types: &'payload TypeArena,
}

pub(crate) fn capsule_sha256(
    root: SymbolIdentity,
    declarations: &[HirDeclaration],
    functions: &[HirFunction],
    methods: &[HirMethod],
    source_types: &TypeArena,
) -> Option<String> {
    let payload = serde_json::to_vec(&CapsulePayload {
        root,
        declarations,
        functions,
        methods,
        source_types,
    })
    .ok()?;
    let digest = Sha256::digest(payload);
    Some(digest.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PoplibDependency {
    package: String,
    version: String,
    source_sha256: String,
    bubble: String,
    kind: BubbleKind,
    public_api_sha256: String,
}

impl PoplibDependency {
    #[must_use]
    pub fn new(
        package: impl Into<String>,
        version: impl Into<String>,
        source_sha256: impl Into<String>,
        bubble: impl Into<String>,
        kind: BubbleKind,
        public_api_sha256: impl Into<String>,
    ) -> Self {
        Self {
            package: package.into(),
            version: version.into(),
            source_sha256: source_sha256.into(),
            bubble: bubble.into(),
            kind,
            public_api_sha256: public_api_sha256.into(),
        }
    }

    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn bubble(&self) -> &str {
        &self.bubble
    }
}

#[derive(Clone, Debug)]
pub struct PoplibEmission {
    package: String,
    version: String,
    source_sha256: String,
    bubble: String,
    kind: BubbleKind,
    edition: String,
    reference_metadata: ReferenceMetadata,
    dependencies: Vec<PoplibDependency>,
    required_capabilities: Vec<String>,
    native_link_plans: Vec<NativeLinkPlan>,
    resolved_native_providers: Vec<ResolvedNativeProvider>,
    documentation: Option<Vec<u8>>,
    target: Option<(String, Vec<u8>)>,
}

impl PoplibEmission {
    #[must_use]
    pub fn new(
        package: impl Into<String>,
        version: impl Into<String>,
        source_sha256: impl Into<String>,
        bubble: impl Into<String>,
        kind: BubbleKind,
        edition: impl Into<String>,
        reference_metadata: ReferenceMetadata,
    ) -> Self {
        Self {
            package: package.into(),
            version: version.into(),
            source_sha256: source_sha256.into(),
            bubble: bubble.into(),
            kind,
            edition: edition.into(),
            reference_metadata,
            dependencies: Vec::new(),
            required_capabilities: Vec::new(),
            native_link_plans: Vec::new(),
            resolved_native_providers: Vec::new(),
            documentation: None,
            target: None,
        }
    }

    #[must_use]
    pub fn with_dependencies(mut self, mut dependencies: Vec<PoplibDependency>) -> Self {
        dependencies.sort();
        self.dependencies = dependencies;
        self
    }

    #[must_use]
    pub fn with_required_capabilities(mut self, mut capabilities: Vec<String>) -> Self {
        capabilities.sort();
        self.required_capabilities = capabilities;
        self
    }

    #[must_use]
    pub fn with_native_link_plan(mut self, plan: NativeLinkPlan) -> Self {
        self.native_link_plans.push(plan);
        self.native_link_plans.sort();
        self
    }

    #[must_use]
    pub fn with_resolved_native_providers(
        mut self,
        providers: Vec<ResolvedNativeProvider>,
    ) -> Self {
        self.resolved_native_providers = providers;
        self.resolved_native_providers.sort();
        self
    }

    #[must_use]
    pub fn with_documentation(mut self, documentation: Vec<u8>) -> Self {
        self.documentation = Some(documentation);
        self
    }

    #[must_use]
    pub fn with_target_implementation(
        mut self,
        platform_target: impl Into<String>,
        bytes: Vec<u8>,
    ) -> Self {
        self.target = Some((platform_target.into(), bytes));
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LoadedPoplib {
    manifest: PoplibManifest,
    reference_metadata: ReferenceMetadata,
    documentation: Option<Vec<u8>>,
    target_implementation: Option<(String, Vec<u8>)>,
}

impl LoadedPoplib {
    #[must_use]
    pub fn package(&self) -> &str {
        &self.manifest.identity.package
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.manifest.identity.version
    }

    #[must_use]
    pub fn bubble(&self) -> &str {
        &self.manifest.identity.bubble
    }

    #[must_use]
    pub fn dependencies(&self) -> &[PoplibDependency] {
        &self.manifest.dependencies
    }

    #[must_use]
    pub const fn reference_metadata(&self) -> &ReferenceMetadata {
        &self.reference_metadata
    }

    #[must_use]
    pub fn public_api_sha256(&self) -> &str {
        &self.manifest.identity.public_api_sha256
    }

    #[must_use]
    pub fn documentation(&self) -> Option<&[u8]> {
        self.documentation.as_deref()
    }

    #[must_use]
    pub fn target_implementation(&self) -> Option<(&str, &[u8])> {
        self.target_implementation
            .as_ref()
            .map(|(target, bytes)| (target.as_str(), bytes.as_slice()))
    }

    #[must_use]
    pub fn native_link_plans(&self) -> &[NativeLinkPlan] {
        &self.manifest.native_link_plans
    }

    #[must_use]
    pub fn resolved_native_providers(&self) -> &[ResolvedNativeProvider] {
        &self.manifest.resolved_native_providers
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PoplibManifest {
    schema_version: u16,
    identity: PoplibIdentity,
    package_source_sha256: String,
    language_edition: String,
    reference_schema_version: u16,
    capsule_schema_version: u16,
    documentation_schema_version: u16,
    target_implementation_schema_version: u16,
    compiler_compatibility: String,
    plri_abi_minimum: String,
    plri_abi_maximum: String,
    dependencies: Vec<PoplibDependency>,
    public_namespaces: Vec<String>,
    initialization_order: Vec<String>,
    required_capabilities: Vec<String>,
    native_link_plans: Vec<NativeLinkPlan>,
    resolved_native_providers: Vec<ResolvedNativeProvider>,
    reference_only: bool,
    documentation: Option<PoplibFileReference>,
    targets: Vec<PoplibTarget>,
    files: Vec<PoplibFileReference>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PoplibIdentity {
    package: String,
    version: String,
    bubble: String,
    kind: BubbleKind,
    public_api_sha256: String,
    implementation_sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PoplibFileReference {
    path: String,
    size: u64,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PoplibTarget {
    platform_target: String,
    implementation: PoplibFileReference,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PoplibError {
    InvalidInput,
    InvalidPath,
    InvalidManifest,
    UnsupportedSchema,
    NonCanonical,
    TooLarge,
    TooManyFiles,
    MissingFile,
    UnexpectedFile,
    SizeMismatch,
    HashMismatch,
    InvalidReferenceMetadata,
    Io,
}

impl fmt::Display for PoplibError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid .poplib artifact: {self:?}")
    }
}

impl Error for PoplibError {}

/// Emits and verifies one deterministic logical `.poplib` directory.
///
/// # Errors
///
/// Fails closed before publication when inputs or the staged artifact do not
/// satisfy the version-1 manifest, reference, inventory, and hash contract.
pub fn emit_poplib(path: &Path, emission: &PoplibEmission) -> Result<(), PoplibError> {
    validate_emission(emission)?;
    let reference = encode_reference_metadata(&emission.reference_metadata)
        .map_err(|_| PoplibError::InvalidReferenceMetadata)?;
    let reference_file = file_reference("reference.metadata", &reference)?;
    let public_api_sha256 = reference_file.sha256.clone();
    let mut files = vec![reference_file];
    let documentation = emission
        .documentation
        .as_deref()
        .map(|bytes| file_reference("documentation.xml", bytes))
        .transpose()?;
    if let Some(documentation) = &documentation {
        files.push(documentation.clone());
    }
    let targets = if let Some((platform_target, bytes)) = &emission.target {
        let target_path = format!("targets/{platform_target}/native.object");
        let implementation = file_reference(&target_path, bytes)?;
        files.push(implementation.clone());
        vec![PoplibTarget {
            platform_target: platform_target.clone(),
            implementation,
        }]
    } else {
        Vec::new()
    };
    files.sort();
    let public_namespaces = emission
        .reference_metadata
        .functions()
        .iter()
        .map(crate::ReferenceFunction::namespace)
        .map(str::to_owned)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let manifest = PoplibManifest {
        schema_version: POPLIB_MANIFEST_SCHEMA_VERSION,
        identity: PoplibIdentity {
            package: emission.package.clone(),
            version: emission.version.clone(),
            bubble: emission.bubble.clone(),
            kind: emission.kind,
            public_api_sha256,
            implementation_sha256: targets
                .first()
                .map(|target| target.implementation.sha256.clone()),
        },
        package_source_sha256: emission.source_sha256.clone(),
        language_edition: emission.edition.clone(),
        reference_schema_version: REFERENCE_SCHEMA_VERSION,
        capsule_schema_version: 1,
        documentation_schema_version: 1,
        target_implementation_schema_version: 1,
        compiler_compatibility: env!("CARGO_PKG_VERSION").to_owned(),
        plri_abi_minimum: "1".to_owned(),
        plri_abi_maximum: "1".to_owned(),
        dependencies: emission.dependencies.clone(),
        public_namespaces,
        initialization_order: Vec::new(),
        required_capabilities: emission.required_capabilities.clone(),
        native_link_plans: emission.native_link_plans.clone(),
        resolved_native_providers: emission.resolved_native_providers.clone(),
        reference_only: targets.is_empty(),
        documentation,
        targets,
        files,
    };
    validate_manifest(&manifest)?;
    let manifest_bytes = encode_manifest(&manifest)?;

    let parent = path.parent().ok_or(PoplibError::InvalidPath)?;
    fs::create_dir_all(parent).map_err(|_| PoplibError::Io)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(PoplibError::InvalidPath)?;
    let staging = parent.join(format!(".{file_name}.temporary-{}", std::process::id()));
    if staging.exists() {
        fs::remove_dir_all(&staging).map_err(|_| PoplibError::Io)?;
    }
    fs::create_dir(&staging).map_err(|_| PoplibError::Io)?;
    write_artifact_file(&staging, "reference.metadata", &reference)?;
    if let Some(bytes) = &emission.documentation {
        write_artifact_file(&staging, "documentation.xml", bytes)?;
    }
    if let Some((platform_target, bytes)) = &emission.target {
        write_artifact_file(
            &staging,
            &format!("targets/{platform_target}/native.object"),
            bytes,
        )?;
    }
    write_artifact_file(&staging, "bubble.manifest", &manifest_bytes)?;
    load_poplib(&staging)?;
    publish_staged_directory(path, &staging)
}

/// Loads and fully verifies one `.poplib` directory without source lookup.
///
/// # Errors
///
/// Rejects malformed control data, symlinks, traversal, extra/missing files,
/// size/hash mismatches, and invalid portable reference metadata.
pub fn load_poplib(path: &Path) -> Result<LoadedPoplib, PoplibError> {
    let manifest_bytes = read_bounded(&path.join("bubble.manifest"), MAX_MANIFEST_BYTES)?;
    let manifest: PoplibManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|_| PoplibError::InvalidManifest)?;
    validate_manifest(&manifest)?;
    if encode_manifest(&manifest)? != manifest_bytes {
        return Err(PoplibError::NonCanonical);
    }
    let expected = manifest
        .files
        .iter()
        .map(|file| file.path.clone())
        .chain(["bubble.manifest".to_owned()])
        .collect::<BTreeSet<_>>();
    let actual = collect_artifact_files(path)?;
    if let Some(unexpected) = actual.difference(&expected).next() {
        let _ = unexpected;
        return Err(PoplibError::UnexpectedFile);
    }
    if expected.difference(&actual).next().is_some() {
        return Err(PoplibError::MissingFile);
    }
    let mut reference_bytes = None;
    let mut documentation = None;
    let mut target_implementation = None;
    for file in &manifest.files {
        let bytes = read_bounded(&path.join(&file.path), MAX_ARTIFACT_FILE_BYTES)?;
        if bytes.len() as u64 != file.size {
            return Err(PoplibError::SizeMismatch);
        }
        if artifact_sha256_hex(&bytes) != file.sha256 {
            return Err(PoplibError::HashMismatch);
        }
        if file.path == "reference.metadata" {
            reference_bytes = Some(bytes);
        } else if file.path == "documentation.xml" {
            documentation = Some(bytes);
        } else if let Some(target) = manifest
            .targets
            .iter()
            .find(|target| target.implementation.path == file.path)
        {
            target_implementation = Some((target.platform_target.clone(), bytes));
        }
    }
    let reference_bytes = reference_bytes.ok_or(PoplibError::MissingFile)?;
    if artifact_sha256_hex(&reference_bytes) != manifest.identity.public_api_sha256 {
        return Err(PoplibError::HashMismatch);
    }
    let reference_metadata = decode_reference_metadata(&reference_bytes)
        .map_err(|_| PoplibError::InvalidReferenceMetadata)?;
    Ok(LoadedPoplib {
        manifest,
        reference_metadata,
        documentation,
        target_implementation,
    })
}

fn validate_emission(emission: &PoplibEmission) -> Result<(), PoplibError> {
    if emission.package.is_empty()
        || emission.version.is_empty()
        || emission.bubble.is_empty()
        || emission.edition.is_empty()
        || !valid_sha256(&emission.source_sha256)
        || !is_sorted_unique(&emission.dependencies)
        || !is_sorted_unique(&emission.required_capabilities)
        || !is_sorted_unique(&emission.native_link_plans)
        || emission
            .native_link_plans
            .iter()
            .any(|plan| plan.validate().is_err())
        || !native_providers_match(
            &emission.native_link_plans,
            &emission.resolved_native_providers,
        )
        || emission
            .target
            .as_ref()
            .is_some_and(|(target, _)| !valid_component(target))
        || emission.native_link_plans.iter().any(|plan| {
            emission
                .target
                .as_ref()
                .is_none_or(|(target, _)| target != plan.platform_target())
        })
    {
        return Err(PoplibError::InvalidInput);
    }
    for dependency in &emission.dependencies {
        if dependency.package.is_empty()
            || dependency.version.is_empty()
            || dependency.bubble.is_empty()
            || !valid_sha256(&dependency.source_sha256)
            || !valid_sha256(&dependency.public_api_sha256)
        {
            return Err(PoplibError::InvalidInput);
        }
    }
    Ok(())
}

fn validate_manifest(manifest: &PoplibManifest) -> Result<(), PoplibError> {
    if manifest.schema_version != POPLIB_MANIFEST_SCHEMA_VERSION
        || manifest.reference_schema_version != REFERENCE_SCHEMA_VERSION
        || manifest.capsule_schema_version != 1
        || manifest.documentation_schema_version != 1
        || manifest.target_implementation_schema_version != 1
    {
        return Err(PoplibError::UnsupportedSchema);
    }
    if manifest.identity.package.is_empty()
        || manifest.identity.version.is_empty()
        || manifest.identity.bubble.is_empty()
        || !valid_sha256(&manifest.identity.public_api_sha256)
        || manifest
            .identity
            .implementation_sha256
            .as_deref()
            .is_some_and(|hash| !valid_sha256(hash))
        || !valid_sha256(&manifest.package_source_sha256)
        || !is_sorted_unique(&manifest.dependencies)
        || !is_sorted_unique(&manifest.public_namespaces)
        || !is_sorted_unique(&manifest.required_capabilities)
        || !is_sorted_unique(&manifest.native_link_plans)
        || manifest
            .native_link_plans
            .iter()
            .any(|plan| plan.validate().is_err())
        || !native_providers_match(
            &manifest.native_link_plans,
            &manifest.resolved_native_providers,
        )
        || !is_sorted_unique(&manifest.files)
        || !is_sorted_unique(&manifest.targets)
        || manifest.files.len() > MAX_ARTIFACT_FILES
        || manifest.reference_only != manifest.targets.is_empty()
    {
        return Err(PoplibError::InvalidManifest);
    }
    let mut paths = BTreeSet::new();
    for file in &manifest.files {
        if !valid_relative_path(&file.path)
            || !valid_sha256(&file.sha256)
            || file.size as usize > MAX_ARTIFACT_FILE_BYTES
            || !paths.insert(file.path.as_str())
        {
            return Err(PoplibError::InvalidManifest);
        }
    }
    if !paths.contains("reference.metadata")
        || manifest
            .documentation
            .as_ref()
            .is_some_and(|documentation| !paths.contains(documentation.path.as_str()))
        || manifest.targets.iter().any(|target| {
            !valid_component(&target.platform_target)
                || !paths.contains(target.implementation.path.as_str())
        })
        || manifest.native_link_plans.iter().any(|plan| {
            !manifest
                .targets
                .iter()
                .any(|target| target.platform_target == plan.platform_target())
        })
    {
        return Err(PoplibError::InvalidManifest);
    }
    Ok(())
}

fn native_providers_match(plans: &[NativeLinkPlan], providers: &[ResolvedNativeProvider]) -> bool {
    if !is_sorted_unique(providers) {
        return false;
    }
    let libraries = plans
        .iter()
        .flat_map(|plan| {
            plan.libraries()
                .iter()
                .map(move |library| (plan.platform_target(), library))
        })
        .collect::<Vec<_>>();
    libraries.len() == providers.len()
        && libraries.iter().all(|(target, library)| {
            providers
                .iter()
                .any(|provider| provider.matches_library(library, target))
        })
}

fn encode_manifest(manifest: &PoplibManifest) -> Result<Vec<u8>, PoplibError> {
    let mut bytes = serde_json::to_vec(manifest).map_err(|_| PoplibError::InvalidManifest)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(PoplibError::TooLarge);
    }
    Ok(bytes)
}

fn file_reference(path: &str, bytes: &[u8]) -> Result<PoplibFileReference, PoplibError> {
    if !valid_relative_path(path) || bytes.len() > MAX_ARTIFACT_FILE_BYTES {
        return Err(PoplibError::InvalidPath);
    }
    Ok(PoplibFileReference {
        path: path.to_owned(),
        size: bytes.len() as u64,
        sha256: artifact_sha256_hex(bytes),
    })
}

#[must_use]
pub fn artifact_sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn valid_component(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        && value != "."
        && value != ".."
}

fn valid_relative_path(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('\\')
        && Path::new(value).components().all(|component| {
            matches!(component, Component::Normal(name) if name.to_str().is_some_and(valid_component))
        })
}

fn is_sorted_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn write_artifact_file(root: &Path, relative: &str, bytes: &[u8]) -> Result<(), PoplibError> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|_| PoplibError::Io)?;
    }
    fs::write(path, bytes).map_err(|_| PoplibError::Io)
}

fn read_bounded(path: &Path, maximum: usize) -> Result<Vec<u8>, PoplibError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| PoplibError::MissingFile)?;
    if !metadata.file_type().is_file() || metadata.len() as usize > maximum {
        return Err(PoplibError::TooLarge);
    }
    fs::read(path).map_err(|_| PoplibError::Io)
}

fn collect_artifact_files(root: &Path) -> Result<BTreeSet<String>, PoplibError> {
    let mut pending = vec![(root.to_path_buf(), PathBuf::new())];
    let mut files = BTreeSet::new();
    while let Some((directory, relative_root)) = pending.pop() {
        let mut entries = fs::read_dir(directory)
            .map_err(|_| PoplibError::Io)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| PoplibError::Io)?;
        entries.sort_by_key(fs::DirEntry::file_name);
        for entry in entries {
            let file_type = entry.file_type().map_err(|_| PoplibError::Io)?;
            if file_type.is_symlink() {
                return Err(PoplibError::InvalidPath);
            }
            let relative = relative_root.join(entry.file_name());
            if file_type.is_dir() {
                pending.push((entry.path(), relative));
            } else if file_type.is_file() {
                let text = relative
                    .to_str()
                    .ok_or(PoplibError::InvalidPath)?
                    .replace(std::path::MAIN_SEPARATOR, "/");
                if !valid_relative_path(&text) || !files.insert(text) {
                    return Err(PoplibError::InvalidPath);
                }
                if files.len() > MAX_ARTIFACT_FILES + 1 {
                    return Err(PoplibError::TooManyFiles);
                }
            } else {
                return Err(PoplibError::InvalidPath);
            }
        }
    }
    Ok(files)
}

fn publish_staged_directory(path: &Path, staging: &Path) -> Result<(), PoplibError> {
    if !path.exists() {
        return fs::rename(staging, path).map_err(|_| PoplibError::Io);
    }
    let parent = path.parent().ok_or(PoplibError::InvalidPath)?;
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(PoplibError::InvalidPath)?;
    let backup = parent.join(format!(".{file_name}.previous-{}", std::process::id()));
    if backup.exists() {
        fs::remove_dir_all(&backup).map_err(|_| PoplibError::Io)?;
    }
    fs::rename(path, &backup).map_err(|_| PoplibError::Io)?;
    if fs::rename(staging, path).is_err() {
        let _ = fs::rename(&backup, path);
        return Err(PoplibError::Io);
    }
    fs::remove_dir_all(backup).map_err(|_| PoplibError::Io)
}
