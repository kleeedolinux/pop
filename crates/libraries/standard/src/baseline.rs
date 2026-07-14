//! Versioned machine-readable `Pop.Standard` compatibility baseline.

use std::collections::BTreeSet;

const EMBEDDED_BASELINE: &str =
    include_str!("../../../../libraries/standard/bootstrap/api-baseline.tsv");

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ApiKind {
    Primitive,
    Type,
    Attribute,
    Namespace,
    Function,
    Api,
}

impl ApiKind {
    const fn rank(self) -> u8 {
        match self {
            Self::Primitive => 0,
            Self::Type => 1,
            Self::Attribute => 2,
            Self::Namespace => 3,
            Self::Function => 4,
            Self::Api => 5,
        }
    }

    const fn identity_prefix(self) -> &'static str {
        match self {
            Self::Primitive => "primitive",
            Self::Type => "type",
            Self::Attribute => "attribute",
            Self::Namespace => "namespace",
            Self::Function => "function",
            Self::Api => "api",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiTier {
    Prelude,
    Standard,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiStatus {
    Implemented,
    Prototype,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StandardApiEntry<'text> {
    identity: &'text str,
    kind: ApiKind,
    owner_bubble: &'text str,
    namespace: &'text str,
    name: &'text str,
    signature: &'text str,
    tier: ApiTier,
    status: ApiStatus,
    prelude: bool,
    documentation: &'text str,
}

impl<'text> StandardApiEntry<'text> {
    #[must_use]
    pub const fn identity(self) -> &'text str {
        self.identity
    }

    #[must_use]
    pub const fn kind(self) -> ApiKind {
        self.kind
    }

    #[must_use]
    pub const fn owner_bubble(self) -> &'text str {
        self.owner_bubble
    }

    #[must_use]
    pub const fn namespace(self) -> &'text str {
        self.namespace
    }

    #[must_use]
    pub const fn name(self) -> &'text str {
        self.name
    }

    #[must_use]
    pub const fn signature(self) -> &'text str {
        self.signature
    }

    #[must_use]
    pub const fn tier(self) -> ApiTier {
        self.tier
    }

    #[must_use]
    pub const fn status(self) -> ApiStatus {
        self.status
    }

    #[must_use]
    pub const fn prelude(self) -> bool {
        self.prelude
    }

    #[must_use]
    pub const fn documentation(self) -> &'text str {
        self.documentation
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StandardApiBaseline<'text> {
    schema_version: u32,
    entries: Vec<StandardApiEntry<'text>>,
}

impl<'text> StandardApiBaseline<'text> {
    #[must_use]
    pub const fn schema_version(&self) -> u32 {
        self.schema_version
    }

    #[must_use]
    pub fn entries(&self) -> &[StandardApiEntry<'text>] {
        &self.entries
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApiBaselineError {
    InvalidHeader,
    InvalidEntry,
}

/// Loads the embedded, versioned `Pop.Standard` compatibility snapshot.
///
/// # Errors
///
/// Returns [`ApiBaselineError`] if the repository baseline is malformed.
pub fn standard_api_baseline() -> Result<StandardApiBaseline<'static>, ApiBaselineError> {
    parse_standard_api_baseline(EMBEDDED_BASELINE)
}

/// Parses the canonical bounded API-baseline vocabulary.
///
/// # Errors
///
/// Rejects unsupported schemas, malformed fields, duplicate identities or
/// signatures, and noncanonical kind/identifier order.
pub fn parse_standard_api_baseline(
    text: &str,
) -> Result<StandardApiBaseline<'_>, ApiBaselineError> {
    if !text.ends_with('\n') {
        return Err(ApiBaselineError::InvalidHeader);
    }
    let mut lines = text.lines();
    if lines.next() != Some("schemaVersion\t1")
        || lines.next()
            != Some(
                "identity\tkind\townerBubble\tnamespace\tname\tsignature\ttier\tstatus\tprelude\tdocumentation",
            )
    {
        return Err(ApiBaselineError::InvalidHeader);
    }

    let mut identities = BTreeSet::new();
    let mut signatures = BTreeSet::new();
    let mut previous_order = None;
    let mut entries = Vec::new();
    for line in lines {
        let fields = line.split('\t').collect::<Vec<_>>();
        if fields.len() != 10 || fields.iter().any(|field| field.is_empty()) {
            return Err(ApiBaselineError::InvalidEntry);
        }
        let kind = parse_kind(fields[1])?;
        let identity_number = parse_identity(fields[0], kind)?;
        let order = (kind.rank(), identity_number);
        if previous_order.is_some_and(|previous| previous >= order) {
            return Err(ApiBaselineError::InvalidEntry);
        }
        previous_order = Some(order);
        let tier = match fields[6] {
            "prelude" => ApiTier::Prelude,
            "standard" => ApiTier::Standard,
            _ => return Err(ApiBaselineError::InvalidEntry),
        };
        let status = match fields[7] {
            "implemented" => ApiStatus::Implemented,
            "prototype" => ApiStatus::Prototype,
            _ => return Err(ApiBaselineError::InvalidEntry),
        };
        let prelude = match fields[8] {
            "true" => true,
            "false" => false,
            _ => return Err(ApiBaselineError::InvalidEntry),
        };
        if !matches!(fields[2], "Pop.Internal" | "Pop.Standard")
            || !fields[3].starts_with("Pop")
            || !fields[9].starts_with("architecture/")
            || (prelude && tier != ApiTier::Prelude)
            || !identities.insert(fields[0])
            || !signatures.insert((fields[2], fields[3], fields[4], fields[5]))
        {
            return Err(ApiBaselineError::InvalidEntry);
        }
        entries.push(StandardApiEntry {
            identity: fields[0],
            kind,
            owner_bubble: fields[2],
            namespace: fields[3],
            name: fields[4],
            signature: fields[5],
            tier,
            status,
            prelude,
            documentation: fields[9],
        });
    }
    if entries.is_empty() {
        return Err(ApiBaselineError::InvalidEntry);
    }
    Ok(StandardApiBaseline {
        schema_version: 1,
        entries,
    })
}

fn parse_kind(value: &str) -> Result<ApiKind, ApiBaselineError> {
    match value {
        "Primitive" => Ok(ApiKind::Primitive),
        "Type" => Ok(ApiKind::Type),
        "Attribute" => Ok(ApiKind::Attribute),
        "Namespace" => Ok(ApiKind::Namespace),
        "Function" => Ok(ApiKind::Function),
        "Api" => Ok(ApiKind::Api),
        _ => Err(ApiBaselineError::InvalidEntry),
    }
}

fn parse_identity(value: &str, kind: ApiKind) -> Result<u32, ApiBaselineError> {
    let (prefix, number) = value
        .split_once(':')
        .ok_or(ApiBaselineError::InvalidEntry)?;
    if prefix != kind.identity_prefix() {
        return Err(ApiBaselineError::InvalidEntry);
    }
    number.parse().map_err(|_| ApiBaselineError::InvalidEntry)
}
