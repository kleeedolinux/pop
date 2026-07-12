//! Verified MIR backend and artifact contracts.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use pop_target::{TargetCapability, TargetSpec};

/// Closed runtime profiles selectable by a compiler driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeProfile {
    /// Precise roots with ABI 1.x stable managed-reference handles.
    BootstrapStableHandles,
    /// The production concurrent generational runtime contract.
    ProductionGenerational,
}

/// GC behavior that a backend's lowering has proved it can preserve.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum BackendGcCapability {
    /// Emits complete precise roots at every collecting safe point.
    PreciseRoots,
    /// Reloads every relocated live managed reference after safe points.
    RelocatingManagedReferences,
}

/// Capability facts belonging to a backend implementation, not a target.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BackendCapabilities {
    garbage_collector: BTreeSet<BackendGcCapability>,
}

impl BackendCapabilities {
    #[must_use]
    pub fn new(garbage_collector: impl IntoIterator<Item = BackendGcCapability>) -> Self {
        Self {
            garbage_collector: garbage_collector.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn supports(&self, capability: BackendGcCapability) -> bool {
        self.garbage_collector.contains(&capability)
    }

    /// Validates that backend, target, and native ABI facts all satisfy a
    /// requested runtime profile.
    ///
    /// # Errors
    ///
    /// Returns a typed closed error for the first missing capability or an
    /// incompatible native ABI major version.
    pub fn validate_runtime_profile(
        &self,
        profile: RuntimeProfile,
        target: &TargetSpec,
        native_abi_major: u16,
    ) -> Result<(), RuntimeProfileError> {
        self.require_backend(BackendGcCapability::PreciseRoots)?;
        Self::require_target(target, TargetCapability::PreciseStackMaps)?;

        let expected_abi_major = match profile {
            RuntimeProfile::BootstrapStableHandles => 1,
            RuntimeProfile::ProductionGenerational => {
                self.require_backend(BackendGcCapability::RelocatingManagedReferences)?;
                Self::require_target(target, TargetCapability::RelocatingNursery)?;
                2
            }
        };

        if native_abi_major != expected_abi_major {
            return Err(RuntimeProfileError::IncompatibleNativeAbi {
                profile,
                major: native_abi_major,
            });
        }
        Ok(())
    }

    fn require_backend(&self, capability: BackendGcCapability) -> Result<(), RuntimeProfileError> {
        if self.supports(capability) {
            Ok(())
        } else {
            Err(RuntimeProfileError::MissingBackendCapability(capability))
        }
    }

    fn require_target(
        target: &TargetSpec,
        capability: TargetCapability,
    ) -> Result<(), RuntimeProfileError> {
        if target.supports(capability) {
            Ok(())
        } else {
            Err(RuntimeProfileError::MissingTargetCapability(capability))
        }
    }
}

/// Closed reasons why a runtime profile cannot be selected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeProfileError {
    MissingBackendCapability(BackendGcCapability),
    MissingTargetCapability(TargetCapability),
    IncompatibleNativeAbi { profile: RuntimeProfile, major: u16 },
}

impl fmt::Display for RuntimeProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBackendCapability(capability) => {
                write!(formatter, "backend lacks runtime capability {capability:?}")
            }
            Self::MissingTargetCapability(capability) => {
                write!(formatter, "target lacks runtime capability {capability:?}")
            }
            Self::IncompatibleNativeAbi { profile, major } => write!(
                formatter,
                "native ABI major {major} is incompatible with runtime profile {profile:?}",
            ),
        }
    }
}

impl Error for RuntimeProfileError {}
