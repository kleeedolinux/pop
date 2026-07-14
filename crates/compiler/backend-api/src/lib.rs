//! Verified MIR backend and artifact contracts.

use std::collections::BTreeSet;
use std::error::Error;
use std::fmt;

use pop_foundation::{FunctionId, ValueId};
use pop_mir::{MirBubble, MirEffect, MirInstructionKind};
use pop_target::{TargetCapability, TargetSpec};

/// Closed runtime profiles selectable by a compiler driver.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeProfile {
    /// Precise roots with ABI 1.x stable managed-reference handles.
    BootstrapStableHandles,
    /// The production concurrent generational runtime contract.
    ProductionGenerational,
    /// Minimal Linux eBPF runtime-contract profile.
    LinuxEbpf,
}

impl RuntimeProfile {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::BootstrapStableHandles => "bootstrap-stable-handles",
            Self::ProductionGenerational => "production-generational",
            Self::LinuxEbpf => "linux-ebpf",
        }
    }

    /// Parses a user-facing runtime profile name.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeProfileSelectionError::UnknownRuntimeProfile`] when the
    /// name is not part of the current profile inventory.
    pub fn parse(name: &str) -> Result<Self, RuntimeProfileSelectionError> {
        match name {
            "bootstrap-stable-handles" => Ok(Self::BootstrapStableHandles),
            "production-generational" => Ok(Self::ProductionGenerational),
            "linux-ebpf" => Ok(Self::LinuxEbpf),
            _ => Err(RuntimeProfileSelectionError::UnknownRuntimeProfile(
                name.to_owned(),
            )),
        }
    }

    #[must_use]
    pub fn provided_contracts(self) -> RuntimeContractSet {
        match self {
            Self::BootstrapStableHandles | Self::ProductionGenerational => {
                RuntimeContractSet::new([
                    RuntimeContract::ManagedAllocator,
                    RuntimeContract::GarbageCollector,
                    RuntimeContract::ExceptionRuntime,
                    RuntimeContract::CoroutineScheduler,
                    RuntimeContract::ThreadRuntime,
                    RuntimeContract::DynamicLoader,
                    RuntimeContract::RuntimeReflection,
                    RuntimeContract::FixedStackStorage,
                    RuntimeContract::IntegerOperations,
                    RuntimeContract::DirectCalls,
                    RuntimeContract::StaticData,
                    RuntimeContract::StandardLibraryAdapters,
                    RuntimeContract::InterfaceDispatch,
                    RuntimeContract::ClosureEnvironment,
                ])
            }
            Self::LinuxEbpf => RuntimeContractSet::new([
                RuntimeContract::FixedStackStorage,
                RuntimeContract::IntegerOperations,
                RuntimeContract::DirectCalls,
                RuntimeContract::StaticData,
            ]),
        }
    }

    #[must_use]
    pub fn is_compatible_with_target(self, target: &TargetSpec) -> bool {
        match self {
            Self::LinuxEbpf => {
                matches!(target.triple(), "bpfel-unknown-none" | "bpfeb-unknown-none")
            }
            Self::BootstrapStableHandles | Self::ProductionGenerational => {
                !matches!(target.triple(), "bpfel-unknown-none" | "bpfeb-unknown-none")
            }
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeProfileSelectionError {
    UnknownRuntimeProfile(String),
    IncompatibleTarget {
        profile: RuntimeProfile,
        target: String,
    },
}

impl fmt::Display for RuntimeProfileSelectionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownRuntimeProfile(profile) => {
                write!(formatter, "unknown runtime profile `{profile}`")
            }
            Self::IncompatibleTarget { profile, target } => write!(
                formatter,
                "runtime profile `{}` is incompatible with target `{target}`",
                profile.name()
            ),
        }
    }
}

impl Error for RuntimeProfileSelectionError {}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum RuntimeContract {
    ManagedAllocator,
    GarbageCollector,
    ExceptionRuntime,
    CoroutineScheduler,
    ThreadRuntime,
    DynamicLoader,
    RuntimeReflection,
    FixedStackStorage,
    IntegerOperations,
    DirectCalls,
    StaticData,
    StandardLibraryAdapters,
    InterfaceDispatch,
    ClosureEnvironment,
    KernelHelpers,
    BpfMaps,
    RingBuffer,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RuntimeContractSet {
    contracts: BTreeSet<RuntimeContract>,
}

impl RuntimeContractSet {
    #[must_use]
    pub fn new(contracts: impl IntoIterator<Item = RuntimeContract>) -> Self {
        Self {
            contracts: contracts.into_iter().collect(),
        }
    }

    #[must_use]
    pub fn contains(&self, contract: RuntimeContract) -> bool {
        self.contracts.contains(&contract)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequirementOrigin {
    FunctionEffect {
        function: FunctionId,
        effect: MirEffect,
    },
    Instruction {
        function: FunctionId,
        value: ValueId,
    },
    Transitive {
        required_by: RuntimeContract,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeContractRequirement {
    contract: RuntimeContract,
    origin: RequirementOrigin,
}

impl RuntimeContractRequirement {
    #[must_use]
    pub const fn new(contract: RuntimeContract, origin: RequirementOrigin) -> Self {
        Self { contract, origin }
    }

    #[must_use]
    pub const fn contract(&self) -> RuntimeContract {
        self.contract
    }

    #[must_use]
    pub const fn origin(&self) -> RequirementOrigin {
        self.origin
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProgramRequirements {
    runtime: Vec<RuntimeContractRequirement>,
}

impl ProgramRequirements {
    #[must_use]
    pub fn derive_from_mir(bubble: &MirBubble) -> Self {
        let mut requirements = Self::default();
        for function in bubble.functions() {
            for effect in function.effects().iter() {
                requirements.require_effect(function.function(), effect);
            }
            for block in function.blocks() {
                for instruction in block.instructions() {
                    requirements.require_instruction(
                        function.function(),
                        instruction.result(),
                        instruction.kind(),
                    );
                }
            }
        }
        requirements.close_transitive();
        requirements
    }

    #[must_use]
    pub fn runtime_requirements(&self) -> &[RuntimeContractRequirement] {
        &self.runtime
    }

    pub fn require_runtime(&mut self, contract: RuntimeContract, origin: RequirementOrigin) {
        if !self
            .runtime
            .iter()
            .any(|requirement| requirement.contract == contract && requirement.origin == origin)
        {
            self.runtime
                .push(RuntimeContractRequirement::new(contract, origin));
        }
    }

    fn require_effect(&mut self, function: FunctionId, effect: MirEffect) {
        let origin = RequirementOrigin::FunctionEffect { function, effect };
        match effect {
            MirEffect::Allocates => self.require_runtime(RuntimeContract::ManagedAllocator, origin),
            MirEffect::WritesManagedReference | MirEffect::GcSafePoint | MirEffect::Roots => {
                self.require_runtime(RuntimeContract::GarbageCollector, origin);
            }
            MirEffect::MayUnwind => self.require_runtime(RuntimeContract::ExceptionRuntime, origin),
            MirEffect::Suspends => {
                self.require_runtime(RuntimeContract::CoroutineScheduler, origin);
            }
            MirEffect::ForeignFunction | MirEffect::AmbientIo => {
                self.require_runtime(RuntimeContract::StandardLibraryAdapters, origin);
            }
            MirEffect::UnsafeMemory | MirEffect::CompilerQuery | MirEffect::MayTrap => {}
        }
    }

    fn require_instruction(
        &mut self,
        function: FunctionId,
        value: ValueId,
        instruction: &MirInstructionKind,
    ) {
        let origin = RequirementOrigin::Instruction { function, value };
        match instruction {
            MirInstructionKind::IntegerConstant(_)
            | MirInstructionKind::CheckedIntegerAdd { .. }
            | MirInstructionKind::CheckedIntegerSubtract { .. }
            | MirInstructionKind::CheckedIntegerMultiply { .. }
            | MirInstructionKind::CheckedIntegerDivide { .. }
            | MirInstructionKind::CheckedIntegerRemainder { .. }
            | MirInstructionKind::IntegerNegate { .. }
            | MirInstructionKind::ConvertInteger { .. }
            | MirInstructionKind::CompareIntegerLess { .. }
            | MirInstructionKind::CompareIntegerLessOrEqual { .. }
            | MirInstructionKind::CompareIntegerGreater { .. }
            | MirInstructionKind::CompareIntegerGreaterOrEqual { .. } => {
                self.require_runtime(RuntimeContract::IntegerOperations, origin);
            }
            MirInstructionKind::CallDirect { .. } => {
                self.require_runtime(RuntimeContract::DirectCalls, origin);
            }
            MirInstructionKind::StringConcat { .. }
            | MirInstructionKind::StringFormat { .. }
            | MirInstructionKind::ClassMake { .. }
            | MirInstructionKind::CaptureCellAllocate { .. }
            | MirInstructionKind::ArrayMake { .. }
            | MirInstructionKind::ArrayCreate { .. }
            | MirInstructionKind::TableMake { .. }
            | MirInstructionKind::ListCreate { .. } => {
                self.require_runtime(RuntimeContract::ManagedAllocator, origin);
            }
            MirInstructionKind::CallStandard { .. }
            | MirInstructionKind::CallBuiltinInterface { .. } => {
                self.require_runtime(RuntimeContract::StandardLibraryAdapters, origin);
            }
            MirInstructionKind::GcSafePoint { .. }
            | MirInstructionKind::RetainRoot { .. }
            | MirInstructionKind::ReleaseRoot { .. }
            | MirInstructionKind::WriteBarrier { .. } => {
                self.require_runtime(RuntimeContract::GarbageCollector, origin);
            }
            MirInstructionKind::CallInterface { .. } => {
                self.require_runtime(RuntimeContract::InterfaceDispatch, origin);
            }
            MirInstructionKind::ClosureEnvironmentAllocate { .. }
            | MirInstructionKind::CaptureLoad { .. }
            | MirInstructionKind::CaptureCellReference { .. }
            | MirInstructionKind::CaptureStore { .. } => {
                self.require_runtime(RuntimeContract::ClosureEnvironment, origin);
            }
            _ => {}
        }
        if matches!(
            instruction,
            MirInstructionKind::ClosureEnvironmentAllocate { .. }
        ) {
            self.require_runtime(RuntimeContract::ManagedAllocator, origin);
        }
    }

    fn close_transitive(&mut self) {
        if self
            .runtime
            .iter()
            .any(|requirement| requirement.contract == RuntimeContract::ManagedAllocator)
        {
            self.require_runtime(
                RuntimeContract::FixedStackStorage,
                RequirementOrigin::Transitive {
                    required_by: RuntimeContract::ManagedAllocator,
                },
            );
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RuntimeContractError {
    MissingContract {
        profile: RuntimeProfile,
        target: String,
        requirement: RuntimeContractRequirement,
    },
    IncompatibleTarget {
        profile: RuntimeProfile,
        target: String,
    },
}

impl fmt::Display for RuntimeContractError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingContract {
                profile,
                target,
                requirement,
            } => write!(
                formatter,
                "runtime profile `{}` cannot satisfy contract `{:?}` required by {:?} for target `{target}`",
                profile.name(),
                requirement.contract(),
                requirement.origin()
            ),
            Self::IncompatibleTarget { profile, target } => write!(
                formatter,
                "runtime profile `{}` is incompatible with target `{target}`",
                profile.name()
            ),
        }
    }
}

impl Error for RuntimeContractError {}

/// Resolves program runtime-contract requirements against a selected runtime
/// profile and target.
///
/// # Errors
///
/// Returns the first missing contract or profile/target incompatibility.
pub fn validate_runtime_contracts(
    requirements: &ProgramRequirements,
    profile: RuntimeProfile,
    target: &TargetSpec,
) -> Result<(), RuntimeContractError> {
    if !profile.is_compatible_with_target(target) {
        return Err(RuntimeContractError::IncompatibleTarget {
            profile,
            target: target.triple().to_owned(),
        });
    }
    let provided = profile.provided_contracts();
    for requirement in requirements.runtime_requirements() {
        if !provided.contains(requirement.contract()) {
            return Err(RuntimeContractError::MissingContract {
                profile,
                target: target.triple().to_owned(),
                requirement: requirement.clone(),
            });
        }
    }
    Ok(())
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
        let expected_abi_major = match profile {
            RuntimeProfile::BootstrapStableHandles => {
                self.require_backend(BackendGcCapability::PreciseRoots)?;
                Self::require_target(target, TargetCapability::PreciseStackMaps)?;
                1
            }
            RuntimeProfile::ProductionGenerational => {
                self.require_backend(BackendGcCapability::PreciseRoots)?;
                Self::require_target(target, TargetCapability::PreciseStackMaps)?;
                self.require_backend(BackendGcCapability::RelocatingManagedReferences)?;
                Self::require_target(target, TargetCapability::RelocatingNursery)?;
                2
            }
            RuntimeProfile::LinuxEbpf => {
                Self::require_target(target, TargetCapability::LlvmBpf)?;
                0
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
