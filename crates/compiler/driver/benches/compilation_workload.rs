#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate,
    clippy::single_match_else
)]

use std::fmt::Write as _;

use pop_backend_c::{CLoweringOptions, lower_mir_to_c};
use pop_backend_llvm::{LlvmLoweringOptions, lower_mir_to_llvm_ir};
use pop_driver::{FrontEndBubbleInput, FrontEndModule, FrontEndResult, analyze_bubble};
use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
use pop_hir::HirBubble;
use pop_mir::{MirBubble, lower_hir_bubble, optimize_mir};
use pop_source::SourceFile;
use pop_target::TargetSpec;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompilationWorkloadConfiguration {
    pub modules: u32,
    pub functions_per_module: u32,
    pub statements_per_function: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompilationWorkloadKind {
    ManyFunctions,
    LargeBodies,
    ManyModules,
    CompileTime,
}

impl CompilationWorkloadKind {
    pub const ALL: [Self; 4] = [
        Self::ManyFunctions,
        Self::LargeBodies,
        Self::ManyModules,
        Self::CompileTime,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::ManyFunctions => "many_functions",
            Self::LargeBodies => "large_bodies",
            Self::ManyModules => "many_modules",
            Self::CompileTime => "compile_time",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|workload| workload.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompilationStage {
    FrontEnd,
    HirToMir,
    MirOptimize,
    CBackend,
    LlvmBackend,
}

impl CompilationStage {
    pub const ALL: [Self; 5] = [
        Self::FrontEnd,
        Self::HirToMir,
        Self::MirOptimize,
        Self::CBackend,
        Self::LlvmBackend,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::FrontEnd => "front_end",
            Self::HirToMir => "hir_to_mir",
            Self::MirOptimize => "mir_optimize",
            Self::CBackend => "c_backend",
            Self::LlvmBackend => "llvm_backend",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|stage| stage.name() == name)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CompilationObservation {
    pub semantic_items: usize,
    pub output_bytes: usize,
}

pub struct PreparedCompilationWorkload {
    input: FrontEndBubbleInput,
    front_end: FrontEndResult,
    mir: MirBubble,
    optimized_mir: MirBubble,
    logical_modules: u32,
    logical_functions: u64,
    logical_statements: u64,
    source_bytes: usize,
}

impl PreparedCompilationWorkload {
    pub const fn logical_modules(&self) -> u32 {
        self.logical_modules
    }

    pub const fn logical_functions(&self) -> u64 {
        self.logical_functions
    }

    pub const fn logical_statements(&self) -> u64 {
        self.logical_statements
    }

    pub const fn source_bytes(&self) -> usize {
        self.source_bytes
    }

    pub fn hir(&self) -> &HirBubble {
        self.front_end.hir().expect("prepared workload has HIR")
    }

    pub const fn mir(&self) -> &MirBubble {
        &self.mir
    }

    pub const fn optimized_mir(&self) -> &MirBubble {
        &self.optimized_mir
    }

    pub fn run_stage(&self, stage: CompilationStage) -> Result<CompilationObservation, String> {
        match stage {
            CompilationStage::FrontEnd => {
                let result = analyze_bubble(self.input.clone());
                if !result.diagnostics().is_empty() {
                    return Err(result.diagnostic_snapshot());
                }
                let hir = result.hir().ok_or("front end did not publish HIR")?;
                Ok(CompilationObservation {
                    semantic_items: hir.functions().len() + hir.declarations().len(),
                    output_bytes: 0,
                })
            }
            CompilationStage::HirToMir => {
                let mir = lower_hir_bubble(self.hir(), self.front_end.types())
                    .map_err(|errors| format!("MIR lowering failed: {errors:?}"))?;
                Ok(CompilationObservation {
                    semantic_items: mir_item_count(&mir),
                    output_bytes: 0,
                })
            }
            CompilationStage::MirOptimize => {
                let mir = optimize_mir(self.mir().clone(), self.front_end.types())
                    .map_err(|errors| format!("MIR optimization failed: {errors:?}"))?;
                Ok(CompilationObservation {
                    semantic_items: mir_item_count(&mir),
                    output_bytes: 0,
                })
            }
            CompilationStage::CBackend => {
                let translation = lower_mir_to_c(
                    self.optimized_mir(),
                    self.front_end.types(),
                    CLoweringOptions::default(),
                )
                .map_err(|error| format!("C lowering failed: {error}"))?;
                Ok(CompilationObservation {
                    semantic_items: self.optimized_mir().functions().len(),
                    output_bytes: translation.as_str().len(),
                })
            }
            CompilationStage::LlvmBackend => {
                let module = lower_mir_to_llvm_ir(
                    self.optimized_mir(),
                    self.front_end.types(),
                    &benchmark_target(),
                    LlvmLoweringOptions::default(),
                )
                .map_err(|error| format!("LLVM lowering failed: {error}"))?;
                Ok(CompilationObservation {
                    semantic_items: self.optimized_mir().functions().len(),
                    output_bytes: module.to_string().len(),
                })
            }
        }
    }
}

pub fn prepare_workload(
    kind: CompilationWorkloadKind,
    configuration: CompilationWorkloadConfiguration,
) -> Result<PreparedCompilationWorkload, String> {
    if configuration.modules == 0
        || configuration.functions_per_module == 0
        || configuration.statements_per_function == 0
    {
        return Err("compiler benchmark dimensions must be non-zero".to_owned());
    }

    let (input, source_bytes) = build_input(kind, configuration)?;
    let front_end = analyze_bubble(input.clone());
    if !front_end.diagnostics().is_empty() {
        return Err(front_end.diagnostic_snapshot());
    }
    let hir = front_end.hir().ok_or("front end did not publish HIR")?;
    let mir = lower_hir_bubble(hir, front_end.types())
        .map_err(|errors| format!("MIR lowering failed: {errors:?}"))?;
    let optimized_mir = optimize_mir(mir.clone(), front_end.types())
        .map_err(|errors| format!("MIR optimization failed: {errors:?}"))?;
    let logical_functions =
        u64::from(configuration.modules) * u64::from(configuration.functions_per_module);
    let logical_statements = logical_functions * u64::from(configuration.statements_per_function);

    Ok(PreparedCompilationWorkload {
        input,
        front_end,
        mir,
        optimized_mir,
        logical_modules: configuration.modules,
        logical_functions,
        logical_statements,
        source_bytes,
    })
}

fn build_input(
    kind: CompilationWorkloadKind,
    configuration: CompilationWorkloadConfiguration,
) -> Result<(FrontEndBubbleInput, usize), String> {
    let mut modules = Vec::new();
    let mut source_bytes = 0;
    for module_index in 0..configuration.modules {
        let text = module_source(kind, configuration, module_index);
        source_bytes += text.len();
        let source = SourceFile::new(
            FileId::from_raw(module_index),
            format!("src/benchmarkModule{module_index}.pop"),
            text,
        )
        .map_err(|error| format!("benchmark source failed: {error}"))?;
        modules.push(FrontEndModule::new(
            ModuleId::from_raw(module_index),
            source,
        ));
    }
    Ok((
        FrontEndBubbleInput::new(
            BubbleId::from_raw(0),
            NamespaceId::from_raw(0),
            Vec::new(),
            modules,
        ),
        source_bytes,
    ))
}

fn module_source(
    kind: CompilationWorkloadKind,
    configuration: CompilationWorkloadConfiguration,
    module_index: u32,
) -> String {
    let mut source = format!("namespace Benchmark.Module{module_index}\n");
    if kind == CompilationWorkloadKind::ManyModules && module_index > 0 {
        let _ = writeln!(source, "using Benchmark.Module{}", module_index - 1);
    }
    if kind == CompilationWorkloadKind::CompileTime {
        let _ = write!(
            source,
            "@CompileTime\n\
             private function compileValue{module_index}(value: Int): Int\n\
                 return value + 1\n\
             end\n\
             @AttributeUsage(targets = {{ AttributeTarget.Function }}, repeatable = false)\n\
             private attribute BenchmarkValue{module_index}(value: Int)\n"
        );
    }

    for function_index in 0..configuration.functions_per_module {
        let global_index = module_index * configuration.functions_per_module + function_index;
        if kind == CompilationWorkloadKind::CompileTime {
            let _ = writeln!(
                source,
                "@BenchmarkValue{module_index}(compileValue{module_index}({function_index}))"
            );
        }
        let _ = write!(
            source,
            "internal function benchmarkFunction{global_index}(): Int\n\
                 local value = {function_index}\n"
        );
        for statement_index in 0..configuration.statements_per_function {
            match kind {
                CompilationWorkloadKind::LargeBodies => {
                    let operand = statement_index % 7 + 1;
                    let _ = writeln!(source, "    value = (value + {operand}) * 2 - {operand}");
                }
                _ => {
                    let operand = statement_index % 7 + 1;
                    let _ = writeln!(source, "    value = value + {operand}");
                }
            }
        }
        if kind == CompilationWorkloadKind::ManyModules && module_index > 0 {
            let previous = (module_index - 1) * configuration.functions_per_module + function_index;
            let _ = writeln!(source, "    value = value + benchmarkFunction{previous}()");
        }
        source.push_str("    return value\nend\n");
    }
    source
}

fn mir_item_count(mir: &MirBubble) -> usize {
    mir.functions().len()
        + mir.methods().len()
        + mir.nested_functions().len()
        + mir.declarations().len()
}

fn benchmark_target() -> TargetSpec {
    TargetSpec::for_triple("x86_64-unknown-linux-gnu").expect("benchmark target is supported")
}
