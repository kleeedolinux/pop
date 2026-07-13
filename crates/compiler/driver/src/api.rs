//! Immutable front-end inputs, results, and native/reference boundary types.
//!
//! This module is the driver-facing data contract. It contains no phase
//! orchestration and cannot redefine semantic compiler behavior.

use std::collections::BTreeSet;

use pop_compile_time::{CompileTimeValue, EvaluationFailure, EvaluationResult};
use pop_foundation::{
    BubbleId, Diagnostic, ModuleId, NamespaceId, SourceSpan, SymbolId, SymbolIdentity, TypeId,
};
use pop_hir::HirBubble;
use pop_library_bridge::{FoundationBubble, NativeEffect, NativeExport, PopAbiType};
use pop_source::SourceFile;
use pop_types::{AttributeQueryIndex, BootstrapSchema, PrimitiveType, TypeArena};

use crate::front_end::diagnostic_snapshot;

#[derive(Clone, Debug)]
pub struct FrontEndModule {
    pub(crate) module: ModuleId,
    pub(crate) source: SourceFile,
}

impl FrontEndModule {
    #[must_use]
    pub const fn new(module: ModuleId, source: SourceFile) -> Self {
        Self { module, source }
    }

    #[must_use]
    pub const fn module(&self) -> ModuleId {
        self.module
    }

    #[must_use]
    pub const fn source(&self) -> &SourceFile {
        &self.source
    }
}

#[derive(Clone, Debug)]
pub struct FrontEndBubbleInput {
    pub(crate) bubble: BubbleId,
    pub(crate) namespace: NamespaceId,
    pub(crate) dependencies: Vec<BubbleId>,
    pub(crate) modules: Vec<FrontEndModule>,
    pub(crate) implicit_main_module: Option<ModuleId>,
    pub(crate) reference_metadata: Vec<ReferenceMetadata>,
}

impl FrontEndBubbleInput {
    #[must_use]
    pub fn new(
        bubble: BubbleId,
        namespace: NamespaceId,
        mut dependencies: Vec<BubbleId>,
        mut modules: Vec<FrontEndModule>,
    ) -> Self {
        dependencies.sort_unstable();
        dependencies.dedup();
        modules.sort_by_key(FrontEndModule::module);
        Self {
            bubble,
            namespace,
            dependencies,
            modules,
            implicit_main_module: None,
            reference_metadata: Vec::new(),
        }
    }

    /// Allows the binary-root `function main(...)` shorthand for one Module.
    /// Library and ordinary analysis inputs use default internal visibility.
    #[must_use]
    pub const fn with_implicit_main_entry(mut self, module: ModuleId) -> Self {
        self.implicit_main_module = Some(module);
        self
    }

    /// Supplies verified public metadata for direct dependency Bubbles.
    #[must_use]
    pub fn with_reference_metadata(mut self, mut metadata: Vec<ReferenceMetadata>) -> Self {
        metadata.sort_by_key(ReferenceMetadata::bubble);
        self.reference_metadata = metadata;
        self
    }
}

#[derive(Clone, Debug)]
pub struct FrontEndResult {
    pub(crate) hir: Option<HirBubble>,
    pub(crate) types: TypeArena,
    pub(crate) attribute_queries: AttributeQueryIndex,
    pub(crate) compile_time_evaluations: Vec<FrontEndCompileTimeEvaluation>,
    pub(crate) constants: Vec<FrontEndConstant>,
    pub(crate) diagnostics: Vec<Diagnostic>,
    pub(crate) reference_metadata: Result<ReferenceMetadata, ReferenceMetadataError>,
}

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ReferenceType {
    Primitive(PrimitiveType),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceFunctionParameter {
    pub(crate) name: String,
    pub(crate) parameter_type: ReferenceType,
}

impl ReferenceFunctionParameter {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn parameter_type(&self) -> ReferenceType {
        self.parameter_type
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceFunction {
    pub(crate) identity: SymbolIdentity,
    pub(crate) module: ModuleId,
    pub(crate) namespace: String,
    pub(crate) name: String,
    pub(crate) parameters: Vec<ReferenceFunctionParameter>,
    pub(crate) results: Vec<ReferenceType>,
    pub(crate) effects: pop_types::EffectSummary,
    pub(crate) span: SourceSpan,
}

impl ReferenceFunction {
    #[must_use]
    pub const fn identity(&self) -> SymbolIdentity {
        self.identity
    }

    #[must_use]
    pub const fn module(&self) -> ModuleId {
        self.module
    }

    #[must_use]
    pub fn namespace(&self) -> &str {
        &self.namespace
    }

    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub fn parameters(&self) -> &[ReferenceFunctionParameter] {
        &self.parameters
    }

    #[must_use]
    pub fn results(&self) -> &[ReferenceType] {
        &self.results
    }

    #[must_use]
    pub const fn effects(&self) -> pop_types::EffectSummary {
        self.effects
    }

    #[must_use]
    pub const fn span(&self) -> SourceSpan {
        self.span
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceMetadata {
    pub(crate) bubble: BubbleId,
    pub(crate) functions: Vec<ReferenceFunction>,
}

impl ReferenceMetadata {
    #[must_use]
    pub const fn bubble(&self) -> BubbleId {
        self.bubble
    }

    #[must_use]
    pub fn functions(&self) -> &[ReferenceFunction] {
        &self.functions
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceMetadataError {
    AnalysisUnavailable,
    MissingDeclaration(SymbolIdentity),
    UnsupportedPublicType {
        function: SymbolIdentity,
        type_id: TypeId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum NativeExportValidationError {
    ExportCount {
        expected: usize,
        actual: usize,
    },
    WrongBubble {
        native_symbol: &'static str,
    },
    WrongNamespace {
        native_symbol: &'static str,
        namespace: &'static str,
    },
    DuplicateBinding {
        namespace: &'static str,
        name: &'static str,
    },
    DuplicateNativeSymbol {
        native_symbol: &'static str,
    },
    MissingBinding {
        name: &'static str,
        parameter_types: Vec<&'static str>,
    },
}

/// Verifies that native Standard adapters bind exactly to trusted bootstrap
/// metadata before either contract is used for analysis or linking.
///
/// # Errors
///
/// Returns a closed validation error for a missing, duplicate, or mismatched
/// adapter binding.
pub fn validate_standard_native_exports(
    bootstrap: &BootstrapSchema,
    exports: &[NativeExport],
) -> Result<(), NativeExportValidationError> {
    let entries = bootstrap.standard_functions();
    if entries.len() != exports.len() {
        return Err(NativeExportValidationError::ExportCount {
            expected: entries.len(),
            actual: exports.len(),
        });
    }

    let mut bindings = BTreeSet::new();
    let mut native_symbols = BTreeSet::new();
    for export in exports {
        if export.bubble() != FoundationBubble::Standard {
            return Err(NativeExportValidationError::WrongBubble {
                native_symbol: export.native_symbol(),
            });
        }
        if export.namespace() != "Pop" {
            return Err(NativeExportValidationError::WrongNamespace {
                native_symbol: export.native_symbol(),
                namespace: export.namespace(),
            });
        }
        let binding = (
            export.namespace(),
            export.name(),
            export.parameters(),
            export.results(),
        );
        if !bindings.insert(binding) {
            return Err(NativeExportValidationError::DuplicateBinding {
                namespace: export.namespace(),
                name: export.name(),
            });
        }
        if !native_symbols.insert(export.native_symbol()) {
            return Err(NativeExportValidationError::DuplicateNativeSymbol {
                native_symbol: export.native_symbol(),
            });
        }
    }

    for entry in entries {
        let matching = exports.iter().any(|export| {
            export.name() == entry.source_name()
                && export
                    .parameters()
                    .iter()
                    .copied()
                    .map(pop_abi_type_name)
                    .eq(entry.parameter_types().iter().copied())
                && export
                    .results()
                    .iter()
                    .copied()
                    .map(pop_abi_type_name)
                    .eq(entry.result_types().iter().copied())
                && export
                    .effects()
                    .iter()
                    .copied()
                    .map(native_effect_name)
                    .eq(entry.effects().iter().copied())
        });
        if !matching {
            return Err(NativeExportValidationError::MissingBinding {
                name: entry.source_name(),
                parameter_types: entry.parameter_types().to_vec(),
            });
        }
    }
    Ok(())
}

const fn pop_abi_type_name(value: PopAbiType) -> &'static str {
    match value {
        PopAbiType::Int => "Int",
        PopAbiType::Int64 => "Int64",
        PopAbiType::UInt64 => "UInt64",
        PopAbiType::Float => "Float",
        PopAbiType::Boolean => "Boolean",
        PopAbiType::Byte => "Byte",
        PopAbiType::String => "String",
        PopAbiType::ManagedReference => "ManagedReference",
    }
}

const fn native_effect_name(value: NativeEffect) -> &'static str {
    match value {
        NativeEffect::Allocates => "Allocates",
        NativeEffect::WritesManagedReference => "WritesManagedReference",
        NativeEffect::MayTrap => "MayTrap",
        NativeEffect::MayUnwind => "MayUnwind",
        NativeEffect::Suspends => "Suspends",
        NativeEffect::UnsafeMemory => "UnsafeMemory",
        NativeEffect::ForeignFunction => "ForeignFunction",
        NativeEffect::AmbientIo => "AmbientIo",
        NativeEffect::CompilerQuery => "CompilerQuery",
        NativeEffect::GcSafePoint => "GcSafePoint",
        NativeEffect::Roots => "Roots",
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrontEndConstant {
    pub(crate) symbol: SymbolId,
    pub(crate) name: String,
    pub(crate) type_id: TypeId,
    pub(crate) value: CompileTimeValue,
}

impl FrontEndConstant {
    #[must_use]
    pub const fn symbol(&self) -> SymbolId {
        self.symbol
    }
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }
    #[must_use]
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }
    #[must_use]
    pub const fn value(&self) -> &CompileTimeValue {
        &self.value
    }
}

/// One source-requested compile-time outcome retained for incremental
/// dependency tracking and provenance-aware tooling.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FrontEndCompileTimeEvaluation {
    Result(EvaluationResult),
    Failure(EvaluationFailure),
}

impl FrontEndCompileTimeEvaluation {
    #[must_use]
    pub const fn result(&self) -> Option<&EvaluationResult> {
        match self {
            Self::Result(result) => Some(result),
            Self::Failure(_) => None,
        }
    }

    #[must_use]
    pub const fn failure(&self) -> Option<&EvaluationFailure> {
        match self {
            Self::Result(_) => None,
            Self::Failure(failure) => Some(failure),
        }
    }
}

impl FrontEndResult {
    #[must_use]
    pub const fn hir(&self) -> Option<&HirBubble> {
        self.hir.as_ref()
    }

    #[must_use]
    pub const fn types(&self) -> &TypeArena {
        &self.types
    }

    #[must_use]
    pub const fn attribute_queries(&self) -> &AttributeQueryIndex {
        &self.attribute_queries
    }

    #[must_use]
    pub fn compile_time_evaluations(&self) -> &[FrontEndCompileTimeEvaluation] {
        &self.compile_time_evaluations
    }

    #[must_use]
    pub fn constants(&self) -> &[FrontEndConstant] {
        &self.constants
    }

    #[must_use]
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    #[must_use]
    pub fn diagnostic_snapshot(&self) -> String {
        diagnostic_snapshot(&self.diagnostics)
    }

    /// Returns the verified public-function projection for dependent Bubbles.
    ///
    /// # Errors
    ///
    /// Fails closed when analysis did not publish HIR or a public signature
    /// contains a type outside the current metadata schema.
    pub const fn reference_metadata(&self) -> Result<&ReferenceMetadata, ReferenceMetadataError> {
        match &self.reference_metadata {
            Ok(metadata) => Ok(metadata),
            Err(error) => Err(*error),
        }
    }
}
