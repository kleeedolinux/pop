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
) -> Result<Vec<NativeLinkInput>, NativeLinkResolutionError> {
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
    for (library, package_root) in libraries.into_values() {
        match library.kind() {
            NativeLibraryKind::System => {
                let name = library
                    .name()
                    .ok_or(NativeLinkResolutionError::InvalidPlan)?;
                if library.discovery() == Some(NativeLibraryDiscovery::PackageConfiguration) {
                    inputs.extend(resolve_package_configuration(
                        name,
                        library.version_requirement(),
                    )?);
                } else {
                    inputs.push(NativeLinkInput::SystemLibrary(name.to_owned()));
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
                inputs.push(NativeLinkInput::File(
                    package_root.join(
                        library
                            .path()
                            .ok_or(NativeLinkResolutionError::InvalidPlan)?,
                    ),
                ));
            }
        }
    }
    Ok(inputs)
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
) -> Result<Vec<NativeLinkInput>, NativeLinkResolutionError> {
    let version = package_configuration_output("--modversion", package)?;
    if requirement.is_some_and(|requirement| !version_satisfies(version.trim(), requirement)) {
        return Err(NativeLinkResolutionError::ProviderVersionMismatch);
    }
    if !package_configuration_output("--libs-only-other", package)?
        .trim()
        .is_empty()
    {
        return Err(NativeLinkResolutionError::UnsupportedProvider);
    }
    let mut inputs = Vec::new();
    for token in package_configuration_output("--libs-only-L", package)?.split_ascii_whitespace() {
        let path = token
            .strip_prefix("-L")
            .filter(|path| !path.is_empty() && !path.chars().any(char::is_control))
            .ok_or(NativeLinkResolutionError::InvalidProviderOutput)?;
        inputs.push(NativeLinkInput::SearchPath(PathBuf::from(path)));
    }
    for token in package_configuration_output("--libs-only-l", package)?.split_ascii_whitespace() {
        let name = token
            .strip_prefix("-l")
            .filter(|name| valid_provider_library_name(name))
            .ok_or(NativeLinkResolutionError::InvalidProviderOutput)?;
        inputs.push(NativeLinkInput::SystemLibrary(name.to_owned()));
    }
    Ok(inputs)
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
