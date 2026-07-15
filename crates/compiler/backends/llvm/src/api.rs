//! Public LLVM artifact options, verified emission, and typed backend errors.
//!
//! The public boundary exposes artifacts and structured failures, never
//! Inkwell values or the backend-private lowering representation.

use std::fmt;
use std::num::NonZeroU32;
use std::path::Path;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine,
};

use inkwell::targets::TargetTriple;

use pop_backend_api::{
    BackendCapabilities, BackendGcCapability, RuntimeProfile, RuntimeProfileError,
};
use pop_foundation::{FieldId, FunctionId, SymbolId, TypeId, ValueId};
use pop_runtime_native_abi::NativeAbiVersion;
use pop_target::TargetSpec;

use crate::lowering::PrivateModule;

const LLVM_OPTIMIZATION_PIPELINE: &str = "default<O3>";
const DEFAULT_GC_POLL_INTERVAL: NonZeroU32 =
    NonZeroU32::new(16_384).expect("the default GC poll interval is nonzero");

/// Returns the closed GC capability inventory proved by LLVM conformance.
#[must_use]
pub fn llvm_backend_capabilities() -> BackendCapabilities {
    BackendCapabilities::new([
        BackendGcCapability::PreciseRoots,
        BackendGcCapability::RelocatingManagedReferences,
    ])
}

/// Validates an LLVM runtime profile against target and exact native ABI facts.
///
/// # Errors
///
/// Returns the first closed backend, target, or ABI incompatibility.
pub fn validate_llvm_runtime_profile(
    profile: RuntimeProfile,
    target: &TargetSpec,
    native_abi: NativeAbiVersion,
) -> Result<(), RuntimeProfileError> {
    llvm_backend_capabilities().validate_runtime_profile(profile, target, native_abi.major())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LlvmLoweringOptions {
    pub(crate) emit_comments: bool,
    pub(crate) entry_point: Option<SymbolId>,
    pub(crate) runtime_profile: RuntimeProfile,
    pub(crate) gc_poll_interval: NonZeroU32,
}

impl Default for LlvmLoweringOptions {
    fn default() -> Self {
        Self {
            emit_comments: false,
            entry_point: None,
            runtime_profile: RuntimeProfile::BootstrapStableHandles,
            gc_poll_interval: DEFAULT_GC_POLL_INTERVAL,
        }
    }
}

impl LlvmLoweringOptions {
    #[must_use]
    pub const fn emit_comments(mut self, emit: bool) -> Self {
        self.emit_comments = emit;
        self
    }

    #[must_use]
    pub const fn with_entry_point(mut self, symbol: SymbolId) -> Self {
        self.entry_point = Some(symbol);
        self
    }

    #[must_use]
    pub const fn with_runtime_profile(mut self, profile: RuntimeProfile) -> Self {
        self.runtime_profile = profile;
        self
    }

    #[must_use]
    pub const fn with_gc_poll_interval(mut self, interval: NonZeroU32) -> Self {
        self.gc_poll_interval = interval;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlvmModule {
    pub(crate) triple: String,
    pub(crate) private: PrivateModule,
}

impl LlvmModule {
    #[must_use]
    pub fn triple(&self) -> &str {
        &self.triple
    }

    /// Parses and verifies the generated textual LLVM module.
    ///
    /// # Errors
    ///
    /// Returns an error when LLVM rejects the backend-private IR.
    pub fn verify(&self) -> Result<(), LlvmEmissionError> {
        let context = Context::create();
        let module = self.parse_module(&context)?;
        module
            .verify()
            .map_err(|error| LlvmEmissionError::InvalidModule(error.to_string()))
    }

    /// Verifies this module through LLVM and emits a native object with Inkwell.
    ///
    /// # Errors
    ///
    /// Returns an error if LLVM rejects the private emission, target setup
    /// fails, or the object cannot be written.
    pub fn emit_object(&self, path: &Path) -> Result<(), LlvmEmissionError> {
        Target::initialize_native(&InitializationConfig::default())
            .map_err(LlvmEmissionError::TargetInitialization)?;
        let context = Context::create();
        let module = self.parse_module(&context)?;
        let triple = TargetTriple::create(&self.triple);
        module.set_triple(&triple);
        let target = Target::from_triple(&triple)
            .map_err(|error| LlvmEmissionError::UnsupportedTarget(error.to_string()))?;
        let cpu = TargetMachine::get_host_cpu_name().to_string();
        let features = TargetMachine::get_host_cpu_features().to_string();
        let machine = target
            .create_target_machine(
                &triple,
                &cpu,
                &features,
                OptimizationLevel::Aggressive,
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or_else(|| LlvmEmissionError::UnsupportedTarget(self.triple.clone()))?;
        module.set_data_layout(&machine.get_target_data().get_data_layout());
        module
            .verify()
            .map_err(|error| LlvmEmissionError::InvalidModule(error.to_string()))?;

        let pass_options = PassBuilderOptions::create();
        pass_options.set_verify_each(true);
        pass_options.set_loop_interleaving(true);
        pass_options.set_loop_vectorization(true);
        pass_options.set_loop_slp_vectorization(true);
        pass_options.set_loop_unrolling(true);
        pass_options.set_merge_functions(true);
        module
            .run_passes(LLVM_OPTIMIZATION_PIPELINE, &machine, pass_options)
            .map_err(|error| LlvmEmissionError::Optimization(error.to_string()))?;
        module
            .verify()
            .map_err(|error| LlvmEmissionError::InvalidModule(error.to_string()))?;
        machine
            .write_to_file(&module, FileType::Object, path)
            .map_err(|error| LlvmEmissionError::ObjectEmission(error.to_string()))
    }

    fn parse_module<'context>(
        &self,
        context: &'context Context,
    ) -> Result<inkwell::module::Module<'context>, LlvmEmissionError> {
        let mut text = self.to_string().into_bytes();
        text.push(0);
        let buffer = MemoryBuffer::create_from_memory_range_copy(&text, "pop-module");
        context
            .create_module_from_ir(buffer)
            .map_err(|error| LlvmEmissionError::InvalidModule(error.to_string()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlvmEmissionError {
    TargetInitialization(String),
    UnsupportedTarget(String),
    InvalidModule(String),
    Optimization(String),
    ObjectEmission(String),
}

impl fmt::Display for LlvmEmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TargetInitialization(error) => {
                write!(formatter, "LLVM target initialization failed: {error}")
            }
            Self::UnsupportedTarget(error) => write!(formatter, "unsupported LLVM target: {error}"),
            Self::InvalidModule(error) => write!(formatter, "LLVM rejected generated IR: {error}"),
            Self::Optimization(error) => {
                write!(formatter, "LLVM optimization failed: {error}")
            }
            Self::ObjectEmission(error) => {
                write!(formatter, "LLVM object emission failed: {error}")
            }
        }
    }
}

impl std::error::Error for LlvmEmissionError {}

impl fmt::Display for LlvmModule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(formatter, "; Pop Lang native module")?;
        writeln!(formatter, "target triple = \"{}\"", self.triple)?;
        self.private.render(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlvmLoweringError {
    MirVerification(Vec<pop_mir::MirVerificationError>),
    RuntimeProfile(RuntimeProfileError),
    UnsupportedInstruction {
        function: FunctionId,
        value: ValueId,
    },
    StaleManagedReference {
        value: ValueId,
        location: String,
    },
    InvalidType(TypeId),
    InvalidFieldLayout(FieldId),
    InvalidEntryPoint(SymbolId),
    UnsupportedEntryPointSignature(SymbolId),
    UnsupportedAsync,
}

impl fmt::Display for LlvmLoweringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MirVerification(errors) => {
                write!(formatter, "MIR verification failed: {errors:?}")
            }
            Self::RuntimeProfile(error) => write!(formatter, "runtime profile rejected: {error}"),
            Self::UnsupportedInstruction { function, value } => write!(
                formatter,
                "LLVM backend does not support MIR instruction f{} v{}",
                function.raw(),
                value.raw()
            ),
            Self::StaleManagedReference { value, location } => write!(
                formatter,
                "LLVM ABI 2 lowering retained stale managed value v{} at {location}",
                value.raw()
            ),
            Self::InvalidType(type_id) => write!(formatter, "invalid MIR type t{}", type_id.raw()),
            Self::InvalidFieldLayout(field) => {
                write!(formatter, "no LLVM field layout for field f{}", field.raw())
            }
            Self::InvalidEntryPoint(symbol) => {
                write!(
                    formatter,
                    "entry point symbol s{} is not defined",
                    symbol.raw()
                )
            }
            Self::UnsupportedEntryPointSignature(symbol) => write!(
                formatter,
                "entry point s{} must accept () or (Array<String>) and return () or Int",
                symbol.raw()
            ),
            Self::UnsupportedAsync => write!(
                formatter,
                "LLVM backend does not yet support async task state machines"
            ),
        }
    }
}

impl std::error::Error for LlvmLoweringError {}
