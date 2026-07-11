use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PackageManifest {
    name: String,
    version: String,
    edition: String,
    dependencies: Vec<DependencyRequirement>,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DependencyRequirement {
    alias: String,
    requirement: String,
}

impl DependencyRequirement {
    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    #[must_use]
    pub fn requirement(&self) -> &str {
        &self.requirement
    }
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
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
    let mut saw_package = false;
    for raw_line in text.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            section = match line {
                "[package]" => {
                    saw_package = true;
                    "package"
                }
                "[dependencies]" => "dependencies",
                _ => return Err(ManifestError::UnsupportedSection),
            };
            continue;
        }
        let (key, raw_value) = line.split_once('=').ok_or(ManifestError::InvalidLine)?;
        let key = key.trim();
        let value = parse_string(raw_value.trim())?;
        let entries = match section {
            "package" => &mut package,
            "dependencies" => &mut dependencies,
            _ => return Err(ManifestError::MissingPackageSection),
        };
        if entries.insert(key.to_owned(), value).is_some() {
            return Err(ManifestError::DuplicateKey);
        }
    }
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
    let dependencies = dependencies
        .into_iter()
        .map(|(alias, requirement)| {
            if !valid_pascal(&alias) {
                return Err(ManifestError::InvalidDependencyAlias);
            }
            if !valid_dotted_number(&requirement) {
                return Err(ManifestError::InvalidVersion);
            }
            Ok(DependencyRequirement { alias, requirement })
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PackageManifest {
        name,
        version,
        edition,
        dependencies,
    })
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
