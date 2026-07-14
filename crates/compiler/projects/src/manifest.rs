use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageManifest {
    name: String,
    version: String,
    edition: String,
    dependencies: Vec<DependencyRequirement>,
    development_dependencies: Vec<DependencyRequirement>,
    platform_dependencies: Vec<PlatformDependencies>,
}

impl PackageManifest {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn version(&self) -> &str {
        &self.version
    }

    #[must_use]
    pub fn edition(&self) -> &str {
        &self.edition
    }

    #[must_use]
    pub fn dependencies(&self) -> &[DependencyRequirement] {
        &self.dependencies
    }

    #[must_use]
    pub fn development_dependencies(&self) -> &[DependencyRequirement] {
        &self.development_dependencies
    }

    #[must_use]
    pub fn platform_dependencies(&self) -> &[PlatformDependencies] {
        &self.platform_dependencies
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlatformDependencies {
    platform_target: String,
    dependencies: Vec<DependencyRequirement>,
}

impl PlatformDependencies {
    #[must_use]
    pub fn platform_target(&self) -> &str {
        &self.platform_target
    }

    #[must_use]
    pub fn dependencies(&self) -> &[DependencyRequirement] {
        &self.dependencies
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyRequirement {
    alias: String,
    version_requirement: Option<String>,
    source: DependencySource,
    bubble: Option<String>,
}

impl DependencyRequirement {
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    #[must_use]
    pub fn requirement(&self) -> &str {
        self.version_requirement.as_deref().unwrap_or("")
    }

    #[must_use]
    pub fn version_requirement(&self) -> Option<&str> {
        self.version_requirement.as_deref()
    }

    #[must_use]
    pub const fn source(&self) -> &DependencySource {
        &self.source
    }

    #[must_use]
    pub fn bubble(&self) -> Option<&str> {
        self.bubble.as_deref()
    }

    #[must_use]
    pub const fn workspace_inherited(&self) -> bool {
        matches!(self.source, DependencySource::Workspace)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DependencySource {
    Registry,
    LocalPath(String),
    ExactGit {
        repository: String,
        revision: String,
    },
    Workspace,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceManifest {
    members: Vec<String>,
    exclude: Vec<String>,
    default_members: Vec<String>,
    resolver: String,
}

impl WorkspaceManifest {
    #[must_use]
    pub fn members(&self) -> &[String] {
        &self.members
    }

    #[must_use]
    pub fn exclude(&self) -> &[String] {
        &self.exclude
    }

    #[must_use]
    pub fn default_members(&self) -> &[String] {
        &self.default_members
    }

    #[must_use]
    pub fn resolver(&self) -> &str {
        &self.resolver
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BubbleKind {
    Library,
    Binary,
    Test,
    Example,
    Benchmark,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveredBubble {
    name: String,
    kind: BubbleKind,
    root: String,
    modules: Vec<String>,
    depends_on_library: bool,
}

impl DiscoveredBubble {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn kind(&self) -> BubbleKind {
        self.kind
    }

    #[must_use]
    pub fn root(&self) -> &str {
        &self.root
    }

    #[must_use]
    pub fn modules(&self) -> &[String] {
        &self.modules
    }

    #[must_use]
    pub const fn depends_on_library(&self) -> bool {
        self.depends_on_library
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManifestError {
    MissingPackageSection,
    MissingPackageName,
    MissingVersion,
    MissingEdition,
    InvalidLine,
    InvalidStringValue,
    InvalidPackageName,
    InvalidDependencyAlias,
    InvalidVersion,
    InvalidEdition,
    UnsupportedSection,
    DuplicateKey,
    InvalidTargetName,
    DuplicateTarget,
    DuplicateSourcePath,
    InvalidDependency,
    MissingGitRevision,
    MissingWorkspaceSection,
    MissingWorkspaceMembers,
    MissingWorkspaceResolver,
    InvalidWorkspaceMember,
    DuplicateWorkspaceMember,
}

impl fmt::Display for ManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid bubble.toml: {self:?}")
    }
}

impl Error for ManifestError {}

/// Parses the architecture-authorized package/dependency manifest subset.
///
/// # Errors
///
/// Rejects missing required keys, duplicate/unknown sections, non-string
/// values, and noncanonical identities.
pub fn parse_package_manifest(text: &str) -> Result<PackageManifest, ManifestError> {
    let mut section = "";
    let mut package = BTreeMap::new();
    let mut dependencies = BTreeMap::new();
    let mut development_dependencies = BTreeMap::new();
    let mut platform_dependencies: BTreeMap<String, BTreeMap<String, DependencyRequirement>> =
        BTreeMap::new();
    let mut platform_section = None;
    let mut saw_package = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            platform_section = None;
            section = match line {
                "[package]" => {
                    saw_package = true;
                    "package"
                }
                "[dependencies]" => "dependencies",
                "[developmentDependencies]" => "developmentDependencies",
                "[workspace]"
                | "[workspace.package]"
                | "[workspace.dependencies]"
                | "[workspace.diagnostics]" => "workspace",
                _ => {
                    if let Some(platform_target) = parse_platform_dependency_section(line) {
                        platform_section = Some(platform_target);
                        "platformDependencies"
                    } else {
                        return Err(ManifestError::UnsupportedSection);
                    }
                }
            };
            continue;
        }
        let (key, raw_value) = line.split_once('=').ok_or(ManifestError::InvalidLine)?;
        let key = key.trim();
        match section {
            "package" => {
                let value = parse_string(raw_value.trim())?;
                if package.insert(key.to_owned(), value).is_some() {
                    return Err(ManifestError::DuplicateKey);
                }
            }
            "dependencies" => {
                if !valid_pascal(key) {
                    return Err(ManifestError::InvalidDependencyAlias);
                }
                let dependency = parse_dependency(key, raw_value.trim())?;
                if dependencies.insert(key.to_owned(), dependency).is_some() {
                    return Err(ManifestError::DuplicateKey);
                }
            }
            "developmentDependencies" => {
                if !valid_pascal(key) {
                    return Err(ManifestError::InvalidDependencyAlias);
                }
                let dependency = parse_dependency(key, raw_value.trim())?;
                if development_dependencies
                    .insert(key.to_owned(), dependency)
                    .is_some()
                {
                    return Err(ManifestError::DuplicateKey);
                }
            }
            "platformDependencies" => {
                if !valid_pascal(key) {
                    return Err(ManifestError::InvalidDependencyAlias);
                }
                let dependency = parse_dependency(key, raw_value.trim())?;
                let target = platform_section
                    .as_ref()
                    .ok_or(ManifestError::InvalidTargetName)?;
                if platform_dependencies
                    .entry(target.clone())
                    .or_default()
                    .insert(key.to_owned(), dependency)
                    .is_some()
                {
                    return Err(ManifestError::DuplicateKey);
                }
            }
            "workspace" => {}
            _ => return Err(ManifestError::MissingPackageSection),
        }
    }
    finish_package_manifest(
        saw_package,
        package,
        dependencies,
        development_dependencies,
        platform_dependencies,
    )
}

fn finish_package_manifest(
    saw_package: bool,
    mut package: BTreeMap<String, String>,
    dependencies: BTreeMap<String, DependencyRequirement>,
    development_dependencies: BTreeMap<String, DependencyRequirement>,
    platform_dependencies: BTreeMap<String, BTreeMap<String, DependencyRequirement>>,
) -> Result<PackageManifest, ManifestError> {
    if !saw_package {
        return Err(ManifestError::MissingPackageSection);
    }
    let name = package
        .remove("name")
        .ok_or(ManifestError::MissingPackageName)?;
    let version = package
        .remove("version")
        .ok_or(ManifestError::MissingVersion)?;
    let edition = package
        .remove("edition")
        .ok_or(ManifestError::MissingEdition)?;
    if !package.is_empty() {
        return Err(ManifestError::InvalidLine);
    }
    if !valid_qualified_pascal(&name) {
        return Err(ManifestError::InvalidPackageName);
    }
    if !valid_dotted_number(&version) {
        return Err(ManifestError::InvalidVersion);
    }
    if edition.is_empty() || !edition.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(ManifestError::InvalidEdition);
    }
    let dependencies = dependencies.into_values().collect();
    let development_dependencies = development_dependencies.into_values().collect();
    let platform_dependencies = platform_dependencies
        .into_iter()
        .map(|(platform_target, dependencies)| PlatformDependencies {
            platform_target,
            dependencies: dependencies.into_values().collect(),
        })
        .collect();
    Ok(PackageManifest {
        name,
        version,
        edition,
        dependencies,
        development_dependencies,
        platform_dependencies,
    })
}

fn parse_platform_dependency_section(line: &str) -> Option<String> {
    let platform_target = line
        .strip_prefix("[platform.\"")?
        .strip_suffix("\".dependencies]")?;
    (!platform_target.is_empty()
        && platform_target.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        }))
    .then(|| platform_target.to_owned())
}

fn parse_dependency(alias: &str, value: &str) -> Result<DependencyRequirement, ManifestError> {
    if value.starts_with('"') {
        let version = parse_string(value)?;
        if !valid_dotted_number(&version) {
            return Err(ManifestError::InvalidVersion);
        }
        return Ok(DependencyRequirement {
            alias: alias.to_owned(),
            version_requirement: Some(version),
            source: DependencySource::Registry,
            bubble: None,
        });
    }

    let fields = parse_inline_table(value)?;
    let version_requirement = fields
        .get("version")
        .map(|value| parse_string(value))
        .transpose()?;
    if version_requirement
        .as_deref()
        .is_some_and(|version| !valid_dotted_number(version))
    {
        return Err(ManifestError::InvalidVersion);
    }
    let bubble = fields
        .get("bubble")
        .map(|value| parse_string(value))
        .transpose()?;
    if bubble
        .as_deref()
        .is_some_and(|name| !valid_qualified_pascal(name))
    {
        return Err(ManifestError::InvalidDependency);
    }

    let source = match (
        fields.get("path"),
        fields.get("git"),
        fields.get("workspace"),
    ) {
        (Some(path), None, None) => {
            let path = parse_string(path)?;
            if path.is_empty() || path.starts_with('/') || path.contains('\\') {
                return Err(ManifestError::InvalidDependency);
            }
            DependencySource::LocalPath(path)
        }
        (None, Some(repository), None) => {
            let repository = parse_string(repository)?;
            let revision = fields
                .get("revision")
                .ok_or(ManifestError::MissingGitRevision)
                .and_then(|value| parse_string(value))?;
            if repository.is_empty() || revision.is_empty() {
                return Err(ManifestError::InvalidDependency);
            }
            DependencySource::ExactGit {
                repository,
                revision,
            }
        }
        (None, None, Some(value)) if value == "true" => DependencySource::Workspace,
        (None, None, None) if version_requirement.is_some() => DependencySource::Registry,
        _ => return Err(ManifestError::InvalidDependency),
    };
    let allowed = ["version", "bubble", "path", "git", "revision", "workspace"];
    if fields.keys().any(|key| !allowed.contains(&key.as_str()))
        || (fields.contains_key("revision") && !matches!(source, DependencySource::ExactGit { .. }))
        || (matches!(source, DependencySource::Workspace)
            && (version_requirement.is_some() || bubble.is_some()))
    {
        return Err(ManifestError::InvalidDependency);
    }
    Ok(DependencyRequirement {
        alias: alias.to_owned(),
        version_requirement,
        source,
        bubble,
    })
}

/// Parses the accepted deterministic `[workspace]` manifest subset.
///
/// # Errors
///
/// Rejects missing required fields, unknown sections/keys, unsafe paths, and
/// unrestricted glob syntax.
pub fn parse_workspace_manifest(text: &str) -> Result<WorkspaceManifest, ManifestError> {
    let mut section = "";
    let mut values = BTreeMap::new();
    let mut saw_workspace = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            match line {
                "[workspace]" => {
                    section = "workspace";
                    saw_workspace = true;
                }
                "[package]"
                | "[dependencies]"
                | "[developmentDependencies]"
                | "[workspace.package]"
                | "[workspace.dependencies]"
                | "[workspace.diagnostics]" => section = "ignored",
                _ if parse_platform_dependency_section(line).is_some() => section = "ignored",
                _ => return Err(ManifestError::UnsupportedSection),
            }
            continue;
        }
        if section == "ignored" {
            continue;
        }
        if section != "workspace" {
            return Err(ManifestError::MissingWorkspaceSection);
        }
        let (key, value) = line.split_once('=').ok_or(ManifestError::InvalidLine)?;
        let key = key.trim();
        if !["members", "exclude", "defaultMembers", "resolver"].contains(&key) {
            return Err(ManifestError::InvalidLine);
        }
        if values
            .insert(key.to_owned(), value.trim().to_owned())
            .is_some()
        {
            return Err(ManifestError::DuplicateKey);
        }
    }
    if !saw_workspace {
        return Err(ManifestError::MissingWorkspaceSection);
    }
    let mut members = parse_string_array(
        values
            .remove("members")
            .ok_or(ManifestError::MissingWorkspaceMembers)?
            .as_str(),
    )?;
    let mut exclude = values
        .remove("exclude")
        .map(|value| parse_string_array(&value))
        .transpose()?
        .unwrap_or_default();
    let mut default_members = values
        .remove("defaultMembers")
        .map(|value| parse_string_array(&value))
        .transpose()?
        .unwrap_or_default();
    let resolver = parse_string(
        values
            .remove("resolver")
            .ok_or(ManifestError::MissingWorkspaceResolver)?
            .as_str(),
    )?;
    if resolver != "1" {
        return Err(ManifestError::InvalidEdition);
    }
    for path in members.iter().chain(&exclude).chain(&default_members) {
        validate_workspace_pattern(path)?;
    }
    sort_unique(&mut members)?;
    sort_unique(&mut exclude)?;
    sort_unique(&mut default_members)?;
    Ok(WorkspaceManifest {
        members,
        exclude,
        default_members,
        resolver,
    })
}

/// Expands exact paths and one-component trailing `/*` patterns against a
/// caller-supplied deterministic candidate set.
///
/// # Errors
///
/// Rejects duplicate/invalid candidates and default members outside the
/// selected set.
pub fn discover_workspace_members(
    manifest: &WorkspaceManifest,
    candidates: &[impl AsRef<str>],
) -> Result<Vec<String>, ManifestError> {
    let mut candidates = candidates
        .iter()
        .map(|candidate| candidate.as_ref().to_owned())
        .collect::<Vec<_>>();
    for candidate in &candidates {
        validate_workspace_path(candidate)?;
    }
    sort_unique(&mut candidates)?;
    let mut members = candidates
        .into_iter()
        .filter(|candidate| {
            manifest
                .members
                .iter()
                .any(|pattern| path_matches(pattern, candidate))
        })
        .filter(|candidate| {
            !manifest
                .exclude
                .iter()
                .any(|pattern| path_matches(pattern, candidate))
        })
        .collect::<Vec<_>>();
    members.sort();
    if manifest
        .default_members
        .iter()
        .any(|default| !members.contains(default))
    {
        return Err(ManifestError::InvalidWorkspaceMember);
    }
    Ok(members)
}

fn parse_inline_table(value: &str) -> Result<BTreeMap<String, String>, ManifestError> {
    let inner = value
        .strip_prefix('{')
        .and_then(|value| value.strip_suffix('}'))
        .ok_or(ManifestError::InvalidDependency)?;
    let mut fields = BTreeMap::new();
    for field in split_quoted(inner, ',')? {
        let (key, value) = field
            .split_once('=')
            .ok_or(ManifestError::InvalidDependency)?;
        let key = key.trim();
        let value = value.trim();
        if key.is_empty()
            || value.is_empty()
            || fields.insert(key.to_owned(), value.to_owned()).is_some()
        {
            return Err(ManifestError::InvalidDependency);
        }
    }
    Ok(fields)
}

fn parse_string_array(value: &str) -> Result<Vec<String>, ManifestError> {
    let inner = value
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .ok_or(ManifestError::InvalidStringValue)?;
    if inner.trim().is_empty() {
        return Ok(Vec::new());
    }
    split_quoted(inner, ',')?
        .into_iter()
        .map(|value| parse_string(value.trim()))
        .collect()
}

fn split_quoted(value: &str, separator: char) -> Result<Vec<&str>, ManifestError> {
    let mut quoted = false;
    let mut start = 0;
    let mut parts = Vec::new();
    for (index, character) in value.char_indices() {
        if character == '"' {
            quoted = !quoted;
        } else if character == separator && !quoted {
            parts.push(value[start..index].trim());
            start = index + character.len_utf8();
        }
    }
    if quoted {
        return Err(ManifestError::InvalidStringValue);
    }
    parts.push(value[start..].trim());
    Ok(parts)
}

fn validate_workspace_pattern(path: &str) -> Result<(), ManifestError> {
    let plain = path.strip_suffix("/*").unwrap_or(path);
    if plain.contains('*') {
        return Err(ManifestError::InvalidWorkspaceMember);
    }
    validate_workspace_path(plain)
}

fn validate_workspace_path(path: &str) -> Result<(), ManifestError> {
    if path.is_empty()
        || path.starts_with('/')
        || path.ends_with('/')
        || path.contains('\\')
        || path
            .split('/')
            .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(ManifestError::InvalidWorkspaceMember);
    }
    Ok(())
}

fn path_matches(pattern: &str, candidate: &str) -> bool {
    pattern
        .strip_suffix("/*")
        .map_or(pattern == candidate, |prefix| {
            candidate
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('/'))
                .is_some_and(|rest| !rest.is_empty() && !rest.contains('/'))
        })
}

fn sort_unique(values: &mut [String]) -> Result<(), ManifestError> {
    values.sort();
    if values.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ManifestError::DuplicateWorkspaceMember);
    }
    Ok(())
}

/// Discovers conventional Bubble roots from a deterministic package file list.
///
/// # Errors
///
/// Rejects duplicate paths, non-camelCase additional target names, and target
/// name collisions.
pub fn discover_conventional_bubbles(
    manifest: &PackageManifest,
    files: &[impl AsRef<str>],
) -> Result<Vec<DiscoveredBubble>, ManifestError> {
    let mut paths: Vec<_> = files.iter().map(|path| path.as_ref().to_owned()).collect();
    paths.sort();
    if paths.windows(2).any(|pair| pair[0] == pair[1]) {
        return Err(ManifestError::DuplicateSourcePath);
    }
    let path_set: BTreeSet<_> = paths.iter().map(String::as_str).collect();
    let has_library = path_set.contains("src/lib.pop");
    let ordinary: Vec<_> = paths
        .iter()
        .filter(|path| {
            path.starts_with("src/")
                && is_pop_path(path)
                && path.as_str() != "src/lib.pop"
                && path.as_str() != "src/main.pop"
                && !path.starts_with("src/bin/")
        })
        .cloned()
        .collect();
    let mut bubbles = Vec::new();
    if has_library {
        let mut modules = vec!["src/lib.pop".to_owned()];
        modules.extend(ordinary.clone());
        modules.sort();
        bubbles.push(discovered(
            manifest.name(),
            BubbleKind::Library,
            "src/lib.pop",
            modules,
            false,
        ));
    }
    if path_set.contains("src/main.pop") {
        let mut modules = vec!["src/main.pop".to_owned()];
        if !has_library {
            modules.extend(ordinary);
            modules.sort();
        }
        bubbles.push(discovered(
            manifest.name(),
            BubbleKind::Binary,
            "src/main.pop",
            modules,
            has_library,
        ));
    }
    discover_bins(&paths, has_library, &mut bubbles)?;
    discover_flat(
        &paths,
        "tests/",
        BubbleKind::Test,
        has_library,
        &mut bubbles,
    )?;
    discover_flat(
        &paths,
        "examples/",
        BubbleKind::Example,
        has_library,
        &mut bubbles,
    )?;
    discover_flat(
        &paths,
        "benchmarks/",
        BubbleKind::Benchmark,
        has_library,
        &mut bubbles,
    )?;
    bubbles.sort_by(|left, right| (left.kind, &left.name).cmp(&(right.kind, &right.name)));
    if bubbles
        .windows(2)
        .any(|pair| pair[0].kind == pair[1].kind && pair[0].name == pair[1].name)
    {
        return Err(ManifestError::DuplicateTarget);
    }
    Ok(bubbles)
}

fn discover_bins(
    paths: &[String],
    has_library: bool,
    bubbles: &mut Vec<DiscoveredBubble>,
) -> Result<(), ManifestError> {
    let mut directories = BTreeSet::new();
    for path in paths.iter().filter(|path| path.starts_with("src/bin/")) {
        let rest = &path["src/bin/".len()..];
        if let Some((directory, _)) = rest.split_once('/') {
            directories.insert(directory.to_owned());
        } else if let Some(stem) = path_stem(rest) {
            bubbles.push(additional_target(
                stem,
                BubbleKind::Binary,
                path,
                vec![path.clone()],
                has_library,
            )?);
        }
    }
    for directory in directories {
        let root = format!("src/bin/{directory}/main.pop");
        if !paths.iter().any(|path| path == &root) {
            continue;
        }
        let prefix = format!("src/bin/{directory}/");
        let modules = paths
            .iter()
            .filter(|path| path.starts_with(&prefix) && is_pop_path(path))
            .cloned()
            .collect();
        bubbles.push(additional_target(
            &directory,
            BubbleKind::Binary,
            &root,
            modules,
            has_library,
        )?);
    }
    Ok(())
}

fn discover_flat(
    paths: &[String],
    prefix: &str,
    kind: BubbleKind,
    has_library: bool,
    bubbles: &mut Vec<DiscoveredBubble>,
) -> Result<(), ManifestError> {
    for path in paths.iter().filter(|path| {
        path.starts_with(prefix) && is_pop_path(path) && !path[prefix.len()..].contains('/')
    }) {
        let stem = path_stem(&path[prefix.len()..]).ok_or(ManifestError::InvalidTargetName)?;
        bubbles.push(additional_target(
            stem,
            kind,
            path,
            vec![path.clone()],
            has_library,
        )?);
    }
    Ok(())
}

fn additional_target(
    source_name: &str,
    kind: BubbleKind,
    root: &str,
    modules: Vec<String>,
    depends_on_library: bool,
) -> Result<DiscoveredBubble, ManifestError> {
    if !valid_camel(source_name) {
        return Err(ManifestError::InvalidTargetName);
    }
    let mut characters = source_name.chars();
    let first = characters.next().ok_or(ManifestError::InvalidTargetName)?;
    let name: String = first.to_uppercase().chain(characters).collect();
    Ok(discovered(&name, kind, root, modules, depends_on_library))
}

fn discovered(
    name: &str,
    kind: BubbleKind,
    root: &str,
    modules: Vec<String>,
    depends_on_library: bool,
) -> DiscoveredBubble {
    DiscoveredBubble {
        name: name.to_owned(),
        kind,
        root: root.to_owned(),
        modules,
        depends_on_library,
    }
}

fn parse_string(value: &str) -> Result<String, ManifestError> {
    if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
        return Err(ManifestError::InvalidStringValue);
    }
    let inner = &value[1..value.len() - 1];
    if inner.contains('"') || inner.contains('\\') {
        return Err(ManifestError::InvalidStringValue);
    }
    Ok(inner.to_owned())
}

fn valid_qualified_pascal(value: &str) -> bool {
    value.split('.').all(valid_pascal)
}

fn valid_pascal(value: &str) -> bool {
    value.chars().next().is_some_and(char::is_uppercase) && value.chars().all(char::is_alphanumeric)
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

fn path_stem(file: &str) -> Option<&str> {
    file.strip_suffix(".pop").filter(|stem| !stem.is_empty())
}

fn is_pop_path(path: &str) -> bool {
    path.strip_suffix(".pop").is_some()
}
