use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::BubbleKind;

const LOCK_SCHEMA_VERSION: u16 = 1;
const MAX_LOCK_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", content = "identity")]
pub enum LockedSource {
    #[serde(rename = "registry")]
    Registry(String),
    #[serde(rename = "exactGit")]
    ExactGit {
        repository: String,
        revision: String,
    },
    #[serde(rename = "localPath")]
    LocalPath(String),
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedPackage {
    name: String,
    version: String,
    source: LockedSource,
    content_sha256: String,
    features: Vec<String>,
}

impl LockedPackage {
    /// Creates one exact Package record for a lock graph.
    ///
    /// # Errors
    ///
    /// Rejects noncanonical identities, versions, sources, hashes, and feature
    /// names before they can enter canonical bytes.
    pub fn new<I, S>(
        name: impl Into<String>,
        version: impl Into<String>,
        source: LockedSource,
        content_sha256: impl Into<String>,
        features: I,
    ) -> Result<Self, LockError>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut package = Self {
            name: name.into(),
            version: version.into(),
            source,
            content_sha256: content_sha256.into(),
            features: features.into_iter().map(Into::into).collect(),
        };
        package.features.sort();
        if package.features.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(LockError::DuplicateFeature);
        }
        package.validate()?;
        Ok(package)
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub const fn source(&self) -> &LockedSource {
        &self.source
    }

    #[must_use]
    pub fn content_sha256(&self) -> &str {
        &self.content_sha256
    }

    #[must_use]
    pub fn features(&self) -> &[String] {
        &self.features
    }

    fn validate(&self) -> Result<(), LockError> {
        if !valid_qualified_pascal(&self.name) {
            return Err(LockError::InvalidIdentity);
        }
        if !valid_dotted_number(&self.version) {
            return Err(LockError::InvalidVersion);
        }
        validate_source(&self.source)?;
        validate_sha256(&self.content_sha256)?;
        if self.features.iter().any(|feature| !valid_camel(feature)) {
            return Err(LockError::InvalidFeature);
        }
        if !is_sorted_unique(&self.features) {
            return Err(LockError::NonCanonical);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedBubbleIdentity {
    package: String,
    bubble: String,
    kind: BubbleKind,
}

impl LockedBubbleIdentity {
    #[must_use]
    pub fn new(package: impl Into<String>, bubble: impl Into<String>, kind: BubbleKind) -> Self {
        Self {
            package: package.into(),
            bubble: bubble.into(),
            kind,
        }
    }

    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    #[must_use]
    pub fn bubble(&self) -> &str {
        &self.bubble
    }

    #[must_use]
    pub const fn kind(&self) -> BubbleKind {
        self.kind
    }

    fn validate(&self) -> Result<(), LockError> {
        if valid_qualified_pascal(&self.package) && valid_qualified_pascal(&self.bubble) {
            Ok(())
        } else {
            Err(LockError::InvalidIdentity)
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LockedBubble {
    package: String,
    name: String,
    kind: BubbleKind,
    dependencies: Vec<LockedBubbleIdentity>,
}

impl LockedBubble {
    /// Creates one exact Bubble node with a deterministic direct edge list.
    ///
    /// # Errors
    ///
    /// Rejects invalid identities or duplicate dependencies.
    pub fn new<I>(
        package: impl Into<String>,
        name: impl Into<String>,
        kind: BubbleKind,
        dependencies: I,
    ) -> Result<Self, LockError>
    where
        I: IntoIterator<Item = LockedBubbleIdentity>,
    {
        let mut bubble = Self {
            package: package.into(),
            name: name.into(),
            kind,
            dependencies: dependencies.into_iter().collect(),
        };
        bubble.dependencies.sort();
        if bubble
            .dependencies
            .windows(2)
            .any(|pair| pair[0] == pair[1])
        {
            return Err(LockError::DuplicateDependency);
        }
        bubble.validate()?;
        Ok(bubble)
    }

    #[must_use]
    pub fn package(&self) -> &str {
        &self.package
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn kind(&self) -> BubbleKind {
        self.kind
    }

    #[must_use]
    pub fn dependencies(&self) -> &[LockedBubbleIdentity] {
        &self.dependencies
    }

    fn identity(&self) -> LockedBubbleIdentity {
        LockedBubbleIdentity::new(&self.package, &self.name, self.kind)
    }

    fn validate(&self) -> Result<(), LockError> {
        self.identity().validate()?;
        for dependency in &self.dependencies {
            dependency.validate()?;
        }
        if !is_sorted_unique(&self.dependencies) {
            return Err(LockError::NonCanonical);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BubbleLock {
    schema_version: u16,
    resolver: String,
    platform_target: String,
    packages: Vec<LockedPackage>,
    bubbles: Vec<LockedBubble>,
}

impl BubbleLock {
    /// Builds and verifies one canonical compile-time lock graph.
    ///
    /// # Errors
    ///
    /// Rejects malformed identities, duplicates, unknown edges, and cycles.
    pub fn new<P, B>(
        resolver: impl Into<String>,
        platform_target: impl Into<String>,
        packages: P,
        bubbles: B,
    ) -> Result<Self, LockError>
    where
        P: IntoIterator<Item = LockedPackage>,
        B: IntoIterator<Item = LockedBubble>,
    {
        let mut lock = Self {
            schema_version: LOCK_SCHEMA_VERSION,
            resolver: resolver.into(),
            platform_target: platform_target.into(),
            packages: packages.into_iter().collect(),
            bubbles: bubbles.into_iter().collect(),
        };
        lock.packages
            .sort_by(|left, right| left.name.cmp(&right.name));
        lock.bubbles.sort_by(|left, right| {
            (&left.package, &left.name, left.kind).cmp(&(&right.package, &right.name, right.kind))
        });
        lock.validate()?;
        Ok(lock)
    }

    #[must_use]
    pub fn resolver(&self) -> &str {
        &self.resolver
    }

    #[must_use]
    pub fn platform_target(&self) -> &str {
        &self.platform_target
    }

    #[must_use]
    pub fn packages(&self) -> &[LockedPackage] {
        &self.packages
    }

    #[must_use]
    pub fn bubbles(&self) -> &[LockedBubble] {
        &self.bubbles
    }

    fn validate(&self) -> Result<(), LockError> {
        if self.schema_version != LOCK_SCHEMA_VERSION {
            return Err(LockError::UnsupportedSchema);
        }
        if self.resolver != "1" {
            return Err(LockError::InvalidResolver);
        }
        if self.platform_target.is_empty()
            || !self.platform_target.chars().all(|character| {
                character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
            })
        {
            return Err(LockError::InvalidPlatformTarget);
        }
        for package in &self.packages {
            package.validate()?;
        }
        if !self
            .packages
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name)
        {
            return Err(LockError::DuplicatePackage);
        }
        let packages: BTreeSet<_> = self.packages.iter().map(|package| &package.name).collect();
        for bubble in &self.bubbles {
            bubble.validate()?;
            if !packages.contains(&bubble.package) {
                return Err(LockError::UnknownPackage);
            }
        }
        if !self.bubbles.windows(2).all(|pair| {
            (&pair[0].package, &pair[0].name, pair[0].kind)
                < (&pair[1].package, &pair[1].name, pair[1].kind)
        }) {
            return Err(LockError::DuplicateBubble);
        }
        let identities: BTreeSet<_> = self.bubbles.iter().map(LockedBubble::identity).collect();
        for bubble in &self.bubbles {
            if bubble
                .dependencies
                .iter()
                .any(|dependency| !identities.contains(dependency))
            {
                return Err(LockError::UnknownDependency);
            }
        }
        verify_acyclic(&self.bubbles)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockMode {
    Normal,
    Locked,
    Offline,
    Frozen,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LockError {
    InvalidIdentity,
    InvalidVersion,
    InvalidSource,
    InvalidHash,
    InvalidFeature,
    InvalidResolver,
    InvalidPlatformTarget,
    DuplicateFeature,
    DuplicateDependency,
    DuplicatePackage,
    DuplicateBubble,
    UnknownPackage,
    UnknownDependency,
    DependencyCycle,
    InvalidJson,
    NonCanonical,
    UnsupportedSchema,
    TooLarge,
    MissingLock,
    LockedChange,
    NetworkForbidden,
}

impl fmt::Display for LockError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid bubble.lock: {self:?}")
    }
}

impl Error for LockError {}

/// Encodes one already verified lock as canonical JSON plus one newline.
///
/// # Errors
///
/// Returns a closed validation/encoding error rather than partial bytes.
pub fn encode_lock(lock: &BubbleLock) -> Result<Vec<u8>, LockError> {
    lock.validate()?;
    let mut bytes = serde_json::to_vec(lock).map_err(|_| LockError::InvalidJson)?;
    bytes.push(b'\n');
    if bytes.len() > MAX_LOCK_BYTES {
        return Err(LockError::TooLarge);
    }
    Ok(bytes)
}

/// Parses, validates, and proves the canonical byte representation of a lock.
///
/// # Errors
///
/// Rejects oversized, malformed, unsupported, invalid, and noncanonical input.
pub fn decode_lock(bytes: &[u8]) -> Result<BubbleLock, LockError> {
    if bytes.len() > MAX_LOCK_BYTES {
        return Err(LockError::TooLarge);
    }
    let lock: BubbleLock = serde_json::from_slice(bytes).map_err(|_| LockError::InvalidJson)?;
    lock.validate()?;
    if encode_lock(&lock)? != bytes {
        return Err(LockError::NonCanonical);
    }
    Ok(lock)
}

/// Applies the independent locked/offline policy to proposed canonical bytes.
///
/// The returned Boolean is true only when an atomic write is required.
///
/// # Errors
///
/// Rejects invalid canonical bytes, forbidden network access, a missing lock in
/// locked modes, or a byte change in locked modes.
pub fn apply_lock_policy(
    existing: Option<&[u8]>,
    proposed: &[u8],
    mode: LockMode,
    requires_network: bool,
) -> Result<bool, LockError> {
    decode_lock(proposed)?;
    if let Some(existing) = existing {
        decode_lock(existing)?;
    }
    let locked = matches!(mode, LockMode::Locked | LockMode::Frozen);
    let offline = matches!(mode, LockMode::Offline | LockMode::Frozen);
    if locked && existing.is_none() {
        return Err(LockError::MissingLock);
    }
    if locked && existing.is_some_and(|bytes| bytes != proposed) {
        return Err(LockError::LockedChange);
    }
    if offline && requires_network {
        return Err(LockError::NetworkForbidden);
    }
    Ok(existing != Some(proposed))
}

#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut output = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

fn verify_acyclic(bubbles: &[LockedBubble]) -> Result<(), LockError> {
    let graph: BTreeMap<_, _> = bubbles
        .iter()
        .map(|bubble| (bubble.identity(), bubble.dependencies.as_slice()))
        .collect();
    let mut state = BTreeMap::new();
    for identity in graph.keys() {
        visit(identity, &graph, &mut state)?;
    }
    Ok(())
}

fn visit(
    identity: &LockedBubbleIdentity,
    graph: &BTreeMap<LockedBubbleIdentity, &[LockedBubbleIdentity]>,
    state: &mut BTreeMap<LockedBubbleIdentity, u8>,
) -> Result<(), LockError> {
    match state.get(identity) {
        Some(1) => return Err(LockError::DependencyCycle),
        Some(2) => return Ok(()),
        _ => {}
    }
    state.insert(identity.clone(), 1);
    for dependency in graph.get(identity).copied().unwrap_or_default() {
        visit(dependency, graph, state)?;
    }
    state.insert(identity.clone(), 2);
    Ok(())
}

fn validate_source(source: &LockedSource) -> Result<(), LockError> {
    match source {
        LockedSource::Registry(identity) => {
            if identity.is_empty() || identity.chars().any(char::is_whitespace) {
                return Err(LockError::InvalidSource);
            }
        }
        LockedSource::ExactGit {
            repository,
            revision,
        } => {
            if repository.is_empty()
                || repository.chars().any(char::is_whitespace)
                || revision.is_empty()
                || !revision.bytes().all(|byte| byte.is_ascii_hexdigit())
            {
                return Err(LockError::InvalidSource);
            }
        }
        LockedSource::LocalPath(path) => {
            if path != "."
                && (path.is_empty()
                    || path.starts_with('/')
                    || path.ends_with('/')
                    || path.contains('\\')
                    || path
                        .split('/')
                        .any(|component| component.is_empty() || component == "."))
            {
                return Err(LockError::InvalidSource);
            }
        }
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), LockError> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(LockError::InvalidHash)
    }
}

fn valid_qualified_pascal(value: &str) -> bool {
    value.split('.').all(|component| {
        component.chars().next().is_some_and(char::is_uppercase)
            && component.chars().all(char::is_alphanumeric)
    })
}

fn valid_camel(value: &str) -> bool {
    value.chars().next().is_some_and(char::is_lowercase) && value.chars().all(char::is_alphanumeric)
}

fn valid_dotted_number(value: &str) -> bool {
    !value.is_empty()
        && value.split('.').all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
}

fn is_sorted_unique<T: Ord>(values: &[T]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}
