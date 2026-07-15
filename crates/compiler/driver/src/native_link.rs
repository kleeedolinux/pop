//! Typed, deterministic native-link plan resolution for ADR 0081.

use std::collections::BTreeMap;
use std::error::Error;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::Command;

use pop_mir::MirBubble;
use pop_projects::{
    NativeLibrary, NativeLibraryDiscovery, NativeLibraryKind, NativeLinkPlan, NativeLinkPlanError,
};
use pop_target::{OperatingSystem, TargetSpec};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLinkPlanSource {
    package_root: PathBuf,
    plan: NativeLinkPlan,
}

impl NativeLinkPlanSource {
    #[must_use]
    pub fn new(package_root: impl AsRef<Path>, plan: NativeLinkPlan) -> Self {
        Self {
            package_root: package_root.as_ref().to_path_buf(),
            plan,
        }
    }

    #[must_use]
    pub fn package_root(&self) -> &Path {
        &self.package_root
    }

    #[must_use]
    pub const fn plan(&self) -> &NativeLinkPlan {
        &self.plan
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeLinkInput {
    SearchPath(PathBuf),
    SystemLibrary(String),
    Framework(String),
    File(PathBuf),
}

/// Canonical provider facts retained in `.poplib` without ambient host search
/// paths.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolvedNativeProvider {
    platform_target: String,
    alias: String,
    kind: NativeLibraryKind,
    identity: String,
    version: Option<String>,
    link_libraries: Vec<String>,
    sha256: Option<String>,
}

impl ResolvedNativeProvider {
    #[must_use]
    pub fn platform_target(&self) -> &str {
        &self.platform_target
    }

    #[must_use]
    pub fn alias(&self) -> &str {
        &self.alias
    }

    #[must_use]
    pub const fn kind(&self) -> NativeLibraryKind {
        self.kind
    }

    #[must_use]
    pub fn identity(&self) -> &str {
        &self.identity
    }

    #[must_use]
    pub fn version(&self) -> Option<&str> {
        self.version.as_deref()
    }

    #[must_use]
    pub fn link_libraries(&self) -> &[String] {
        &self.link_libraries
    }

    #[must_use]
    pub fn sha256(&self) -> Option<&str> {
        self.sha256.as_deref()
    }

    pub(crate) fn matches_library(&self, library: &NativeLibrary, target: &str) -> bool {
        self.platform_target == target
            && self.alias == library.alias()
            && self.kind == library.kind()
            && match library.kind() {
                NativeLibraryKind::System => {
                    library.name() == Some(self.identity.as_str())
                        && self.sha256.is_none()
                        && if library.discovery()
                            == Some(NativeLibraryDiscovery::PackageConfiguration)
                        {
                            self.version.as_deref().is_some_and(|version| {
                                valid_provider_version(version)
                                    && library.version_requirement().is_none_or(|requirement| {
                                        version_satisfies(version, requirement)
                                    })
                            }) && !self.link_libraries.is_empty()
                                && self
                                    .link_libraries
                                    .iter()
                                    .all(|name| valid_provider_library_name(name))
                        } else {
                            self.version.is_none()
                                && self.link_libraries == [self.identity.as_str()]
                        }
                }
                NativeLibraryKind::Framework => {
                    library.name() == Some(self.identity.as_str())
                        && self.version.is_none()
                        && self.sha256.is_none()
                        && self.link_libraries.is_empty()
                }
                NativeLibraryKind::Object
                | NativeLibraryKind::Archive
                | NativeLibraryKind::Shared
                | NativeLibraryKind::ImportLibrary => {
                    library.path() == Some(self.identity.as_str())
                        && library.sha256() == self.sha256.as_deref()
                        && self.version.is_none()
                        && self.link_libraries.is_empty()
                }
            }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct NativeLinkResolution {
    inputs: Vec<NativeLinkInput>,
    providers: Vec<ResolvedNativeProvider>,
}

impl NativeLinkResolution {
    #[must_use]
    pub fn inputs(&self) -> &[NativeLinkInput] {
        &self.inputs
    }

    #[must_use]
    pub fn providers(&self) -> &[ResolvedNativeProvider] {
        &self.providers
    }
}

impl NativeLinkInput {
    /// Appends this already-validated input as one or two typed process
    /// arguments. No input is reparsed as a command fragment.
    pub fn append_to(&self, command: &mut Command) {
        match self {
            Self::SearchPath(path) => {
                command.arg("-L").arg(path);
            }
            Self::SystemLibrary(name) => {
                command.arg(format!("-l{name}"));
            }
            Self::Framework(name) => {
                command.arg("-framework").arg(name);
            }
            Self::File(path) => {
                command.arg(path);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeLinkResolutionError {
    InvalidPlan,
    TargetMismatch,
    ConflictingAlias,
    MissingAlias,
    UnsupportedProvider,
    ProviderFailure,
    ProviderVersionMismatch,
    InvalidProviderOutput,
}

impl fmt::Display for NativeLinkResolutionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "native link resolution failed: {self:?}")
    }
}

impl Error for NativeLinkResolutionError {}

impl From<NativeLinkPlanError> for NativeLinkResolutionError {
    fn from(_: NativeLinkPlanError) -> Self {
        Self::InvalidPlan
    }
}

/// Resolves verified plans into typed linker inputs without constructing raw
/// command fragments.
///
/// # Errors
///
/// Rejects target disagreement, conflicting aliases, invalid local inputs,
/// unsupported target providers, or malformed package-configuration output.
pub fn resolve_native_link_inputs(
    sources: &[NativeLinkPlanSource],
    target: &TargetSpec,
) -> Result<NativeLinkResolution, NativeLinkResolutionError> {
    if target.operating_system() != OperatingSystem::Linux
        && sources
            .iter()
            .any(|source| !source.plan.libraries().is_empty())
    {
        return Err(NativeLinkResolutionError::UnsupportedProvider);
    }
    let mut libraries: BTreeMap<&str, (&NativeLibrary, &Path)> = BTreeMap::new();
    for source in sources {
        if source.plan.platform_target() != target.triple() {
            return Err(NativeLinkResolutionError::TargetMismatch);
        }
        source.plan.verify_local_inputs(&source.package_root)?;
        for library in source.plan.libraries() {
            match libraries.get(library.alias()) {
                Some((existing, _)) if *existing != library => {
                    return Err(NativeLinkResolutionError::ConflictingAlias);
                }
                Some(_) => {}
                None => {
                    libraries.insert(library.alias(), (library, &source.package_root));
                }
            }
        }
    }

    let mut inputs = Vec::new();
    let mut providers = Vec::new();
    for (library, package_root) in libraries.into_values() {
        match library.kind() {
            NativeLibraryKind::System => {
                let name = library
                    .name()
                    .ok_or(NativeLinkResolutionError::InvalidPlan)?;
                if library.discovery() == Some(NativeLibraryDiscovery::PackageConfiguration) {
                    let resolved =
                        resolve_package_configuration(name, library.version_requirement())?;
                    providers.push(resolved_provider(
                        target,
                        library,
                        name,
                        Some(resolved.version),
                        resolved
                            .inputs
                            .iter()
                            .filter_map(|input| match input {
                                NativeLinkInput::SystemLibrary(name) => Some(name.clone()),
                                _ => None,
                            })
                            .collect(),
                        None,
                    ));
                    inputs.extend(resolved.inputs);
                } else {
                    inputs.push(NativeLinkInput::SystemLibrary(name.to_owned()));
                    providers.push(resolved_provider(
                        target,
                        library,
                        name,
                        None,
                        vec![name.to_owned()],
                        None,
                    ));
                }
            }
            NativeLibraryKind::Framework => {
                let _ = library;
                return Err(NativeLinkResolutionError::UnsupportedProvider);
            }
            NativeLibraryKind::ImportLibrary => {
                return Err(NativeLinkResolutionError::UnsupportedProvider);
            }
            NativeLibraryKind::Object | NativeLibraryKind::Archive | NativeLibraryKind::Shared => {
                let path = library
                    .path()
                    .ok_or(NativeLinkResolutionError::InvalidPlan)?;
                inputs.push(NativeLinkInput::File(package_root.join(path)));
                providers.push(resolved_provider(
                    target,
                    library,
                    path,
                    None,
                    Vec::new(),
                    library.sha256().map(str::to_owned),
                ));
            }
        }
    }
    Ok(NativeLinkResolution { inputs, providers })
}

fn resolved_provider(
    target: &TargetSpec,
    library: &NativeLibrary,
    identity: &str,
    version: Option<String>,
    link_libraries: Vec<String>,
    sha256: Option<String>,
) -> ResolvedNativeProvider {
    ResolvedNativeProvider {
        platform_target: target.triple().to_owned(),
        alias: library.alias().to_owned(),
        kind: library.kind(),
        identity: identity.to_owned(),
        version,
        link_libraries,
        sha256,
    }
}

/// Verifies that every exact `Ffi.Link` alias used by MIR resolves in its
/// Package plan. Foreign declarations without aliases use the default C/system
/// environment.
///
/// # Errors
///
/// Returns [`NativeLinkResolutionError::MissingAlias`] for any unresolved
/// declaration alias.
pub fn validate_foreign_link_aliases(
    mir: &MirBubble,
    plan: &NativeLinkPlan,
) -> Result<(), NativeLinkResolutionError> {
    plan.validate()?;
    let aliases = plan
        .libraries()
        .iter()
        .map(NativeLibrary::alias)
        .collect::<std::collections::BTreeSet<_>>();
    if mir.foreign_functions().iter().any(|function| {
        function
            .declaration()
            .link_aliases()
            .iter()
            .any(|alias| !aliases.contains(alias.as_str()))
    }) {
        return Err(NativeLinkResolutionError::MissingAlias);
    }
    Ok(())
}

fn resolve_package_configuration(
    package: &str,
    requirement: Option<&str>,
) -> Result<PackageConfigurationResolution, NativeLinkResolutionError> {
    resolve_package_configuration_with(package, requirement, package_configuration_output)
}

fn resolve_package_configuration_with(
    package: &str,
    requirement: Option<&str>,
    output: impl Fn(&str, &str) -> Result<String, NativeLinkResolutionError>,
) -> Result<PackageConfigurationResolution, NativeLinkResolutionError> {
    let version = output("--modversion", package)?;
    let version = version.trim();
    if !valid_provider_version(version) {
        return Err(NativeLinkResolutionError::InvalidProviderOutput);
    }
    if requirement.is_some_and(|requirement| !version_satisfies(version, requirement)) {
        return Err(NativeLinkResolutionError::ProviderVersionMismatch);
    }
    if !output("--libs-only-other", package)?.trim().is_empty() {
        return Err(NativeLinkResolutionError::UnsupportedProvider);
    }
    let mut inputs = Vec::new();
    for token in output("--libs-only-L", package)?.split_ascii_whitespace() {
        let path = token
            .strip_prefix("-L")
            .filter(|path| valid_provider_search_path(Path::new(path)))
            .ok_or(NativeLinkResolutionError::InvalidProviderOutput)?;
        inputs.push(NativeLinkInput::SearchPath(PathBuf::from(path)));
    }
    for token in output("--libs-only-l", package)?.split_ascii_whitespace() {
        let name = token
            .strip_prefix("-l")
            .filter(|name| valid_provider_library_name(name))
            .ok_or(NativeLinkResolutionError::InvalidProviderOutput)?;
        inputs.push(NativeLinkInput::SystemLibrary(name.to_owned()));
    }
    Ok(PackageConfigurationResolution {
        version: version.to_owned(),
        inputs,
    })
}

struct PackageConfigurationResolution {
    version: String,
    inputs: Vec<NativeLinkInput>,
}

fn package_configuration_output(
    option: &str,
    package: &str,
) -> Result<String, NativeLinkResolutionError> {
    let output = Command::new("pkg-config")
        .args([option, package])
        .output()
        .map_err(|_| NativeLinkResolutionError::ProviderFailure)?;
    if !output.status.success() {
        return Err(NativeLinkResolutionError::ProviderFailure);
    }
    String::from_utf8(output.stdout).map_err(|_| NativeLinkResolutionError::InvalidProviderOutput)
}

fn valid_provider_library_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with(['-', '@'])
        && name.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-')
        })
}

fn valid_provider_search_path(path: &Path) -> bool {
    path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                std::path::Component::RootDir | std::path::Component::Normal(_)
            )
        })
}

fn valid_provider_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 256
        && version.chars().all(|character| {
            character.is_ascii_graphic() && !matches!(character, '`' | '$' | '"' | '\'')
        })
}

fn version_satisfies(version: &str, requirement: &str) -> bool {
    requirement.split(',').all(|constraint| {
        let constraint = constraint.trim();
        for operator in [">=", "<=", ">", "<", "="] {
            if let Some(required) = constraint.strip_prefix(operator) {
                let ordering = compare_versions(version, required);
                return match operator {
                    ">=" => !ordering.is_lt(),
                    "<=" => !ordering.is_gt(),
                    ">" => ordering.is_gt(),
                    "<" => ordering.is_lt(),
                    "=" => ordering.is_eq(),
                    _ => false,
                };
            }
        }
        version == constraint
    })
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let components = |version: &str| {
        version
            .split(['.', '-', '+'])
            .map(|component| component.parse::<u64>().unwrap_or(u64::MAX))
            .collect::<Vec<_>>()
    };
    components(left).cmp(&components(right))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider_output(option: &str, _package: &str) -> Result<String, NativeLinkResolutionError> {
        Ok(match option {
            "--modversion" => "1.2.3\n",
            "--libs-only-other" => "",
            "--libs-only-L" => "-L/opt/example/lib\n",
            "--libs-only-l" => "-lexample -ldependency\n",
            _ => return Err(NativeLinkResolutionError::ProviderFailure),
        }
        .to_owned())
    }

    #[test]
    fn package_configuration_output_becomes_typed_inputs_and_exact_provider_facts() {
        let resolved =
            resolve_package_configuration_with("example", Some(">=1.2,<2"), provider_output)
                .expect("bounded provider resolves");

        assert_eq!(resolved.version, "1.2.3");
        assert_eq!(
            resolved.inputs,
            [
                NativeLinkInput::SearchPath(PathBuf::from("/opt/example/lib")),
                NativeLinkInput::SystemLibrary("example".to_owned()),
                NativeLinkInput::SystemLibrary("dependency".to_owned()),
            ]
        );
    }

    #[test]
    fn package_configuration_rejects_relative_search_paths_and_raw_flags() {
        let relative = |option: &str, package: &str| {
            if option == "--libs-only-L" {
                Ok("-Lrelative/lib".to_owned())
            } else {
                provider_output(option, package)
            }
        };
        assert_eq!(
            resolve_package_configuration_with("example", None, relative)
                .map(|resolved| resolved.inputs),
            Err(NativeLinkResolutionError::InvalidProviderOutput)
        );

        let raw_flag = |option: &str, package: &str| {
            if option == "--libs-only-other" {
                Ok("-Wl,--as-needed".to_owned())
            } else {
                provider_output(option, package)
            }
        };
        assert_eq!(
            resolve_package_configuration_with("example", None, raw_flag)
                .map(|resolved| resolved.inputs),
            Err(NativeLinkResolutionError::UnsupportedProvider)
        );
    }
}
