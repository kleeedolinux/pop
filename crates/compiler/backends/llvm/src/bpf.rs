//! Experimental eBPF validation and LLVM BPF object emission.
//!
//! This module keeps BPF-specific policy inside the LLVM backend. It consumes
//! verified canonical MIR, resolves runtime-contract requirements against the
//! selected profile, validates eBPF-specific restrictions, renders
//! backend-private LLVM IR, and asks LLVM's BPF target to emit an ELF object.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetTriple,
};

use pop_backend_api::{
    ProgramRequirements, RuntimeContractError, RuntimeProfile, validate_runtime_contracts,
};
use pop_foundation::{BlockId, FunctionId, SymbolId, TypeId, ValueId};
use pop_mir::{
    MirBlock, MirBubble, MirEffect, MirFunction, MirInstruction, MirInstructionKind, MirTerminator,
    verify_mir_bubble,
};
use pop_target::{TargetCapability, TargetSpec};
use pop_types::{IntegerKind, IntegerValue, PrimitiveType, SemanticType, TypeArena};

const XDP_PASS: i32 = 2;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BpfProgramKind {
    Xdp,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BpfLoweringOptions {
    pub(crate) entry_point: SymbolId,
    pub(crate) program: BpfProgramKind,
    pub(crate) runtime_profile: RuntimeProfile,
}

impl BpfLoweringOptions {
    #[must_use]
    pub const fn xdp(entry_point: SymbolId) -> Self {
        Self {
            entry_point,
            program: BpfProgramKind::Xdp,
            runtime_profile: RuntimeProfile::LinuxEbpf,
        }
    }

    #[must_use]
    pub const fn with_runtime_profile(mut self, runtime_profile: RuntimeProfile) -> Self {
        self.runtime_profile = runtime_profile;
        self
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BpfModule {
    triple: String,
    text: String,
}

impl BpfModule {
    #[must_use]
    pub fn triple(&self) -> &str {
        &self.triple
    }

    #[must_use]
    pub fn as_llvm_ir(&self) -> &str {
        &self.text
    }

    /// Emits an ELF eBPF object through LLVM's BPF target.
    ///
    /// # Errors
    ///
    /// Returns [`BpfBackendError::LlvmBpfUnavailable`] when the linked LLVM was
    /// built without the BPF target or object emission rejects the module.
    pub fn emit_object(&self, path: &Path) -> Result<(), BpfBackendError> {
        Target::initialize_bpf(&InitializationConfig::default());
        let context = Context::create();
        let mut bytes = self.text.clone().into_bytes();
        bytes.push(0);
        let buffer = MemoryBuffer::create_from_memory_range_copy(&bytes, "pop-bpf-module");
        let module = context
            .create_module_from_ir(buffer)
            .map_err(|error| BpfBackendError::InvalidLlvmModule(error.to_string()))?;
        let triple = TargetTriple::create(&self.triple);
        module.set_triple(&triple);
        let target = Target::from_triple(&triple)
            .map_err(|error| BpfBackendError::LlvmBpfUnavailable(error.to_string()))?;
        let machine = target
            .create_target_machine(
                &triple,
                "generic",
                "",
                OptimizationLevel::Default,
                RelocMode::Static,
                CodeModel::Default,
            )
            .ok_or_else(|| BpfBackendError::LlvmBpfUnavailable(self.triple.clone()))?;
        module.set_data_layout(&machine.get_target_data().get_data_layout());
        module
            .verify()
            .map_err(|error| BpfBackendError::InvalidLlvmModule(error.to_string()))?;
        machine
            .write_to_file(&module, FileType::Object, path)
            .map_err(|error| BpfBackendError::ObjectEmission(error.to_string()))
    }
}

impl fmt::Display for BpfModule {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.text)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BpfBackendError {
    MirVerification(Vec<pop_mir::MirVerificationError>),
    InvalidTarget(String),
    InvalidEntryPoint(SymbolId),
    InvalidEntryPointSignature(SymbolId),
    UnsupportedType(TypeId),
    UnsupportedEffect {
        function: FunctionId,
        effect: MirEffect,
    },
    UnsupportedInstruction {
        function: FunctionId,
        value: ValueId,
        reason: BpfUnsupportedReason,
    },
    UnsupportedTerminator {
        function: FunctionId,
        block: BlockId,
    },
    Recursion(SymbolId),
    UnboundedLoop {
        function: FunctionId,
        block: BlockId,
    },
    MissingValue(ValueId),
    InvalidLlvmModule(String),
    LlvmBpfUnavailable(String),
    ObjectEmission(String),
    RuntimeContract(RuntimeContractError),
}

impl BpfBackendError {
    #[must_use]
    pub const fn diagnostic_code(&self) -> &'static str {
        match self {
            Self::InvalidEntryPoint(_) | Self::InvalidEntryPointSignature(_) => "POP7000",
            Self::UnsupportedType(_) => "POP7002",
            Self::UnsupportedEffect {
                effect: MirEffect::Allocates,
                ..
            } => "POP7003",
            Self::UnsupportedInstruction {
                reason: BpfUnsupportedReason::FloatingPoint,
                ..
            } => "POP7004",
            Self::UnsupportedInstruction {
                reason: BpfUnsupportedReason::Call,
                ..
            }
            | Self::Recursion(_) => "POP7005",
            Self::UnsupportedEffect { .. } => "POP7006",
            Self::LlvmBpfUnavailable(_) => "POP7007",
            Self::InvalidTarget(_) => "POP7008",
            Self::UnboundedLoop { .. } => "POP7009",
            Self::RuntimeContract(_) => "POP7006",
            _ => "POP7001",
        }
    }
}

impl fmt::Display for BpfBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MirVerification(errors) => {
                write!(formatter, "MIR verification failed: {errors:?}")
            }
            Self::InvalidTarget(target) => write!(
                formatter,
                "target `{target}` is not an experimental LLVM BPF target"
            ),
            Self::InvalidEntryPoint(symbol) => {
                write!(
                    formatter,
                    "BPF entry point s{} is not defined",
                    symbol.raw()
                )
            }
            Self::InvalidEntryPointSignature(symbol) => write!(
                formatter,
                "BPF XDP entry point s{} must use the current XDP ABI and return Int",
                symbol.raw()
            ),
            Self::UnsupportedType(type_id) => write!(
                formatter,
                "BPF target does not support MIR type t{} in the initial scalar subset",
                type_id.raw()
            ),
            Self::UnsupportedEffect { function, effect } => write!(
                formatter,
                "the current eBPF backend cannot lower effect {effect:?} in MIR function f{}",
                function.raw()
            ),
            Self::UnsupportedInstruction {
                function,
                value,
                reason,
            } => write!(
                formatter,
                "BPF target rejects MIR instruction f{} v{}: {reason}",
                function.raw(),
                value.raw()
            ),
            Self::UnsupportedTerminator { function, block } => write!(
                formatter,
                "BPF target rejects MIR terminator in f{} b{}",
                function.raw(),
                block.raw()
            ),
            Self::Recursion(symbol) => {
                write!(
                    formatter,
                    "BPF target rejects recursive direct call involving s{}",
                    symbol.raw()
                )
            }
            Self::UnboundedLoop { function, block } => write!(
                formatter,
                "BPF target rejects loop backedge to f{} b{} because this MVP does not prove loop bounds",
                function.raw(),
                block.raw()
            ),
            Self::MissingValue(value) => {
                write!(formatter, "BPF lowering lost MIR value v{}", value.raw())
            }
            Self::InvalidLlvmModule(error) => {
                write!(formatter, "LLVM rejected generated BPF IR: {error}")
            }
            Self::LlvmBpfUnavailable(error) => write!(
                formatter,
                "LLVM BPF target is unavailable for this toolchain: {error}"
            ),
            Self::ObjectEmission(error) => {
                write!(formatter, "LLVM BPF object emission failed: {error}")
            }
            Self::RuntimeContract(error) => write!(formatter, "{error}"),
        }
    }
}

impl std::error::Error for BpfBackendError {}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BpfUnsupportedReason {
    FloatingPoint,
    Call,
    BackendImplementation,
    Operation,
}

impl fmt::Display for BpfUnsupportedReason {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FloatingPoint => formatter.write_str(
                "the current eBPF backend cannot lower floating-point operations",
            ),
            Self::Call => {
                formatter.write_str("only non-recursive direct scalar calls are available")
            }
            Self::BackendImplementation => formatter.write_str(
                "the current eBPF backend cannot lower this representation yet; this is not a language restriction",
            ),
            Self::Operation => {
                formatter.write_str("operation is outside the initial scalar subset")
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct BpfValidationPass;

impl BpfValidationPass {
    /// Validates that canonical MIR belongs to the experimental eBPF subset.
    ///
    /// # Errors
    ///
    /// Returns a closed backend error before any BPF artifact is produced.
    pub fn validate(
        self,
        bubble: &MirBubble,
        types: &TypeArena,
        target: &TargetSpec,
        options: BpfLoweringOptions,
    ) -> Result<(), BpfBackendError> {
        validate_target(target)?;
        verify_mir_bubble(bubble, types).map_err(BpfBackendError::MirVerification)?;
        let requirements = ProgramRequirements::derive_from_mir(bubble);
        validate_runtime_contracts(&requirements, options.runtime_profile, target)
            .map_err(BpfBackendError::RuntimeContract)?;
        validate_entry(bubble, types, options.entry_point)?;
        validate_call_graph(bubble)?;
        for function in bubble.functions() {
            validate_function(function, types)?;
        }
        if !bubble.declarations().is_empty()
            || !bubble.methods().is_empty()
            || !bubble.nested_functions().is_empty()
            || !bubble.function_references().is_empty()
        {
            return Err(BpfBackendError::UnsupportedInstruction {
                function: FunctionId::from_raw(0),
                value: ValueId::from_raw(0),
                reason: BpfUnsupportedReason::BackendImplementation,
            });
        }
        Ok(())
    }
}

/// Lowers MIR to backend-private LLVM IR for eBPF.
///
/// # Errors
///
/// Rejects invalid MIR, missing runtime contracts, non-BPF targets, and
/// operations outside the current eBPF backend implementation.
pub fn lower_mir_to_bpf_module(
    bubble: &MirBubble,
    types: &TypeArena,
    target: &TargetSpec,
    options: BpfLoweringOptions,
) -> Result<BpfModule, BpfBackendError> {
    BpfValidationPass.validate(bubble, types, target, options)?;
    let entry = bubble
        .functions()
        .iter()
        .find(|function| function.symbol() == options.entry_point)
        .ok_or(BpfBackendError::InvalidEntryPoint(options.entry_point))?;
    let mut text = String::new();
    text.push_str("; Pop Lang experimental eBPF module\n");
    text.push_str(&format!("target triple = \"{}\"\n\n", target.triple()));
    for function in bubble.functions() {
        lower_function(&mut text, bubble, function, types)?;
        text.push('\n');
    }
    let entry_name = function_name(bubble, entry.symbol());
    match options.program {
        BpfProgramKind::Xdp => {
            text.push_str(&format!(
                "define i32 @pop_bpf_xdp(ptr %ctx) section \"xdp\" {{\nentry:\n  %pop_result = call {} @{entry_name}()\n  %pop_result_i32 = trunc {} %pop_result to i32\n  ret i32 %pop_result_i32\n}}\n",
                llvm_results(entry.results(), types)?,
                llvm_results(entry.results(), types)?
            ));
        }
    }
    Ok(BpfModule {
        triple: target.triple().to_owned(),
        text,
    })
}

fn validate_target(target: &TargetSpec) -> Result<(), BpfBackendError> {
    if matches!(target.triple(), "bpfel-unknown-none" | "bpfeb-unknown-none")
        && target.supports(TargetCapability::LlvmBpf)
    {
        Ok(())
    } else {
        Err(BpfBackendError::InvalidTarget(target.triple().to_owned()))
    }
}

fn validate_entry(
    bubble: &MirBubble,
    types: &TypeArena,
    entry: SymbolId,
) -> Result<(), BpfBackendError> {
    let function = bubble
        .functions()
        .iter()
        .find(|function| function.symbol() == entry)
        .ok_or(BpfBackendError::InvalidEntryPoint(entry))?;
    let int_type = types
        .source_type("Int")
        .ok_or(BpfBackendError::UnsupportedType(TypeId::from_raw(u32::MAX)))?;
    if function.parameters().is_empty() && function.results() == [int_type] {
        Ok(())
    } else {
        Err(BpfBackendError::InvalidEntryPointSignature(entry))
    }
}

fn validate_function(function: &MirFunction, types: &TypeArena) -> Result<(), BpfBackendError> {
    for effect in function.effects().iter() {
        if !matches!(effect, MirEffect::MayTrap) {
            return Err(BpfBackendError::UnsupportedEffect {
                function: function.function(),
                effect,
            });
        }
    }
    for type_id in function.parameters().iter().chain(function.results()) {
        bpf_type(*type_id, types)?;
    }
    let mut seen_blocks = BTreeSet::new();
    for block in function.blocks() {
        if !block.arguments().is_empty() {
            return Err(BpfBackendError::UnsupportedTerminator {
                function: function.function(),
                block: block.block(),
            });
        }
        for argument in block.arguments() {
            bpf_type(argument.type_id(), types)?;
        }
        for instruction in block.instructions() {
            validate_instruction(function, instruction)?;
            if let Some(type_id) = instruction.optional_result_type() {
                bpf_type(type_id, types)?;
            }
        }
        validate_terminator(function, block)?;
        seen_blocks.insert(block.block());
        for target in terminator_targets(block.terminator()) {
            if seen_blocks.contains(&target) {
                return Err(BpfBackendError::UnboundedLoop {
                    function: function.function(),
                    block: target,
                });
            }
        }
    }
    Ok(())
}

fn validate_instruction(
    function: &MirFunction,
    instruction: &MirInstruction,
) -> Result<(), BpfBackendError> {
    let reason = match instruction.kind() {
        MirInstructionKind::IntegerConstant(_)
        | MirInstructionKind::BooleanConstant(_)
        | MirInstructionKind::EnumConstant { .. }
        | MirInstructionKind::BooleanNot { .. }
        | MirInstructionKind::BooleanAnd { .. }
        | MirInstructionKind::BooleanOr { .. }
        | MirInstructionKind::CompareEqual { .. }
        | MirInstructionKind::CompareNotEqual { .. }
        | MirInstructionKind::CompareIntegerLess { .. }
        | MirInstructionKind::CompareIntegerLessOrEqual { .. }
        | MirInstructionKind::CompareIntegerGreater { .. }
        | MirInstructionKind::CompareIntegerGreaterOrEqual { .. }
        | MirInstructionKind::CallDirect { .. } => return Ok(()),
        MirInstructionKind::FloatConstant(_)
        | MirInstructionKind::FloatAdd { .. }
        | MirInstructionKind::FloatSubtract { .. }
        | MirInstructionKind::FloatMultiply { .. }
        | MirInstructionKind::FloatDivide { .. }
        | MirInstructionKind::FloatNegate { .. }
        | MirInstructionKind::CompareFloatLess { .. }
        | MirInstructionKind::CompareFloatLessOrEqual { .. }
        | MirInstructionKind::CompareFloatGreater { .. }
        | MirInstructionKind::CompareFloatGreaterOrEqual { .. }
        | MirInstructionKind::ConvertIntegerToFloat { .. }
        | MirInstructionKind::ConvertFloatToInteger { .. }
        | MirInstructionKind::ConvertFloat { .. } => BpfUnsupportedReason::FloatingPoint,
        MirInstructionKind::StringConstant(_)
        | MirInstructionKind::StringConcat { .. }
        | MirInstructionKind::StringFormat { .. }
        | MirInstructionKind::ArrayMake { .. }
        | MirInstructionKind::ArrayCreate { .. }
        | MirInstructionKind::TableMake { .. }
        | MirInstructionKind::ClassMake { .. }
        | MirInstructionKind::RecordMake { .. }
        | MirInstructionKind::UnionMake { .. }
        | MirInstructionKind::CaptureCellAllocate { .. }
        | MirInstructionKind::ClosureEnvironmentAllocate { .. }
        | MirInstructionKind::GcSafePoint { .. }
        | MirInstructionKind::RetainRoot { .. }
        | MirInstructionKind::ReleaseRoot { .. }
        | MirInstructionKind::FfiHandleOpen { .. }
        | MirInstructionKind::FfiHandleGet { .. }
        | MirInstructionKind::FfiHandleClose { .. }
        | MirInstructionKind::FfiBufferOpen { .. }
        | MirInstructionKind::FfiBufferLength { .. }
        | MirInstructionKind::FfiBufferRead { .. }
        | MirInstructionKind::FfiBufferWrite { .. }
        | MirInstructionKind::FfiBufferBorrow { .. }
        | MirInstructionKind::FfiBufferEndBorrow { .. }
        | MirInstructionKind::FfiBufferClose { .. }
        | MirInstructionKind::FfiPointerNone
        | MirInstructionKind::FfiPointerToOptional { .. }
        | MirInstructionKind::FfiPointerReadOnly { .. }
        | MirInstructionKind::FfiPointerIsPresent { .. }
        | MirInstructionKind::Pin { .. }
        | MirInstructionKind::Unpin { .. }
        | MirInstructionKind::WriteBarrier { .. }
        | MirInstructionKind::CallStandard { .. }
        | MirInstructionKind::TaskCreate { .. }
        | MirInstructionKind::CancelSourceCreate
        | MirInstructionKind::CancelSourceToken { .. }
        | MirInstructionKind::CancelRequest { .. }
        | MirInstructionKind::TaskGroupCreate { .. }
        | MirInstructionKind::TaskStart { .. }
        | MirInstructionKind::CallBuiltinInterface { .. } => {
            BpfUnsupportedReason::BackendImplementation
        }
        MirInstructionKind::CallIndirect { .. }
        | MirInstructionKind::CallInterface { .. }
        | MirInstructionKind::CallReferenced { .. }
        | MirInstructionKind::CallDirectMethod { .. } => BpfUnsupportedReason::Call,
        MirInstructionKind::CheckedIntegerAdd { .. }
        | MirInstructionKind::CheckedIntegerSubtract { .. }
        | MirInstructionKind::CheckedIntegerMultiply { .. }
        | MirInstructionKind::CheckedIntegerDivide { .. }
        | MirInstructionKind::CheckedIntegerRemainder { .. }
        | MirInstructionKind::IntegerNegate { .. }
        | MirInstructionKind::ConvertInteger { .. } => BpfUnsupportedReason::Operation,
        _ => BpfUnsupportedReason::Operation,
    };
    Err(BpfBackendError::UnsupportedInstruction {
        function: function.function(),
        value: instruction.result(),
        reason,
    })
}

fn validate_terminator(function: &MirFunction, block: &MirBlock) -> Result<(), BpfBackendError> {
    if matches!(
        block.terminator(),
        MirTerminator::Branch { .. }
            | MirTerminator::ConditionalBranch { .. }
            | MirTerminator::Return { .. }
            | MirTerminator::Trap(_)
            | MirTerminator::Unreachable
    ) {
        Ok(())
    } else {
        Err(BpfBackendError::UnsupportedTerminator {
            function: function.function(),
            block: block.block(),
        })
    }
}

fn validate_call_graph(bubble: &MirBubble) -> Result<(), BpfBackendError> {
    let graph = bubble
        .functions()
        .iter()
        .map(|function| {
            let calls = function
                .blocks()
                .iter()
                .flat_map(MirBlock::instructions)
                .filter_map(|instruction| match instruction.kind() {
                    MirInstructionKind::CallDirect { function, .. } => Some(*function),
                    _ => None,
                })
                .collect::<Vec<_>>();
            (function.symbol(), calls)
        })
        .collect::<BTreeMap<_, _>>();
    for root in graph.keys().copied() {
        let mut visiting = BTreeSet::new();
        if reaches(root, root, &graph, &mut visiting) {
            return Err(BpfBackendError::Recursion(root));
        }
    }
    Ok(())
}

fn reaches(
    root: SymbolId,
    current: SymbolId,
    graph: &BTreeMap<SymbolId, Vec<SymbolId>>,
    visiting: &mut BTreeSet<SymbolId>,
) -> bool {
    let Some(calls) = graph.get(&current) else {
        return false;
    };
    for call in calls {
        if *call == root {
            return true;
        }
        if visiting.insert(*call) && reaches(root, *call, graph, visiting) {
            return true;
        }
    }
    false
}

fn terminator_targets(terminator: &MirTerminator) -> Vec<BlockId> {
    match terminator {
        MirTerminator::Branch { target, .. } => vec![*target],
        MirTerminator::ConditionalBranch {
            when_true,
            when_false,
            ..
        } => vec![*when_true, *when_false],
        _ => Vec::new(),
    }
}

fn lower_function(
    text: &mut String,
    bubble: &MirBubble,
    function: &MirFunction,
    types: &TypeArena,
) -> Result<(), BpfBackendError> {
    let name = function_name(bubble, function.symbol());
    let parameters = function
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, type_id)| Ok(format!("{} %p{index}", bpf_type(*type_id, types)?)))
        .collect::<Result<Vec<_>, BpfBackendError>>()?
        .join(", ");
    let result = llvm_results(function.results(), types)?;
    text.push_str(&format!(
        "define internal {result} @{name}({parameters}) nounwind {{\n"
    ));
    let parameter_values = function
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, type_id)| {
            Ok((
                ValueId::from_raw(index as u32),
                format!("%p{index}"),
                bpf_type(*type_id, types)?,
            ))
        })
        .collect::<Result<Vec<_>, BpfBackendError>>()?;
    let mut values = parameter_values
        .into_iter()
        .map(|(value, name, type_text)| (value, LoweredValue { name, type_text }))
        .collect::<BTreeMap<_, _>>();
    for block in function.blocks() {
        text.push_str(&format!("b{}:\n", block.block().raw()));
        for argument in block.arguments() {
            values.insert(
                argument.value(),
                LoweredValue {
                    name: format!("%v{}", argument.value().raw()),
                    type_text: bpf_type(argument.type_id(), types)?,
                },
            );
        }
        for instruction in block.instructions() {
            lower_instruction(text, bubble, instruction, types, &mut values)?;
        }
        lower_terminator(text, block.terminator(), types, &values)?;
    }
    text.push_str("}\n");
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct LoweredValue {
    name: String,
    type_text: &'static str,
}

fn lower_instruction(
    text: &mut String,
    bubble: &MirBubble,
    instruction: &MirInstruction,
    types: &TypeArena,
    values: &mut BTreeMap<ValueId, LoweredValue>,
) -> Result<(), BpfBackendError> {
    let result = format!("%v{}", instruction.result().raw());
    let kind = instruction.kind();
    let type_text = instruction
        .optional_result_type()
        .map(|type_id| bpf_type(type_id, types))
        .transpose()?
        .unwrap_or("void");
    let line = match kind {
        MirInstructionKind::IntegerConstant(value) => {
            format!("{result} = add {type_text} 0, {}", integer_literal(*value))
        }
        MirInstructionKind::BooleanConstant(value) => {
            format!("{result} = add i1 0, {}", u8::from(*value))
        }
        MirInstructionKind::EnumConstant { discriminant, .. } => {
            format!("{result} = add i32 0, {discriminant}")
        }
        MirInstructionKind::CheckedIntegerAdd { left, right, .. } => {
            binary(&result, "add", type_text, *left, *right, values)?
        }
        MirInstructionKind::CheckedIntegerSubtract { left, right, .. } => {
            binary(&result, "sub", type_text, *left, *right, values)?
        }
        MirInstructionKind::CheckedIntegerMultiply { left, right, .. } => {
            binary(&result, "mul", type_text, *left, *right, values)?
        }
        MirInstructionKind::IntegerNegate { operand, .. } => {
            let operand = value(*operand, values)?;
            format!("{result} = sub {type_text} 0, {}", operand.name)
        }
        MirInstructionKind::BooleanNot { operand } => {
            let operand = value(*operand, values)?;
            format!("{result} = xor i1 {}, true", operand.name)
        }
        MirInstructionKind::BooleanAnd { left, right } => {
            binary(&result, "and", "i1", *left, *right, values)?
        }
        MirInstructionKind::BooleanOr { left, right } => {
            binary(&result, "or", "i1", *left, *right, values)?
        }
        MirInstructionKind::CompareEqual { left, right } => {
            compare(&result, "eq", *left, *right, values)?
        }
        MirInstructionKind::CompareNotEqual { left, right } => {
            compare(&result, "ne", *left, *right, values)?
        }
        MirInstructionKind::CompareIntegerLess { kind, left, right } => compare(
            &result,
            if kind.is_signed() { "slt" } else { "ult" },
            *left,
            *right,
            values,
        )?,
        MirInstructionKind::CompareIntegerLessOrEqual { kind, left, right } => compare(
            &result,
            if kind.is_signed() { "sle" } else { "ule" },
            *left,
            *right,
            values,
        )?,
        MirInstructionKind::CompareIntegerGreater { kind, left, right } => compare(
            &result,
            if kind.is_signed() { "sgt" } else { "ugt" },
            *left,
            *right,
            values,
        )?,
        MirInstructionKind::CompareIntegerGreaterOrEqual { kind, left, right } => compare(
            &result,
            if kind.is_signed() { "sge" } else { "uge" },
            *left,
            *right,
            values,
        )?,
        MirInstructionKind::ConvertInteger {
            target, operand, ..
        } => {
            let operand = value(*operand, values)?;
            let target_type = integer_type(*target);
            match integer_bits(type_text).cmp(&integer_bits(operand.type_text)) {
                std::cmp::Ordering::Less => format!(
                    "{result} = trunc {} {} to {target_type}",
                    operand.type_text, operand.name
                ),
                std::cmp::Ordering::Equal => {
                    format!("{result} = add {target_type} 0, {}", operand.name)
                }
                std::cmp::Ordering::Greater if target.is_signed() => format!(
                    "{result} = sext {} {} to {target_type}",
                    operand.type_text, operand.name
                ),
                std::cmp::Ordering::Greater => format!(
                    "{result} = zext {} {} to {target_type}",
                    operand.type_text, operand.name
                ),
            }
        }
        MirInstructionKind::CallDirect {
            function,
            arguments,
            ..
        } => {
            let callee = function_name(bubble, *function);
            let arguments = arguments
                .iter()
                .map(|argument| {
                    let value = value(*argument, values)?;
                    Ok(format!("{} {}", value.type_text, value.name))
                })
                .collect::<Result<Vec<_>, BpfBackendError>>()?
                .join(", ");
            format!("{result} = call {type_text} @{callee}({arguments})")
        }
        _ => {
            return Err(BpfBackendError::UnsupportedInstruction {
                function: FunctionId::from_raw(0),
                value: instruction.result(),
                reason: BpfUnsupportedReason::Operation,
            });
        }
    };
    text.push_str("  ");
    text.push_str(&line);
    text.push('\n');
    values.insert(
        instruction.result(),
        LoweredValue {
            name: result,
            type_text,
        },
    );
    Ok(())
}

fn lower_terminator(
    text: &mut String,
    terminator: &MirTerminator,
    _types: &TypeArena,
    values: &BTreeMap<ValueId, LoweredValue>,
) -> Result<(), BpfBackendError> {
    match terminator {
        MirTerminator::Branch { target, arguments } => {
            let _ = arguments;
            text.push_str(&format!("  br label %b{}\n", target.raw()));
        }
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => {
            let condition = value(*condition, values)?;
            text.push_str(&format!(
                "  br i1 {}, label %b{}, label %b{}\n",
                condition.name,
                when_true.raw(),
                when_false.raw()
            ));
        }
        MirTerminator::Return { values: returned } => {
            if returned.is_empty() {
                text.push_str("  ret void\n");
            } else {
                let returned = value(returned[0], values)?;
                text.push_str(&format!("  ret {} {}\n", returned.type_text, returned.name));
            }
        }
        MirTerminator::Trap(_) | MirTerminator::Unreachable => {
            text.push_str(&format!("  ret i32 {XDP_PASS}\n"));
        }
        _ => {
            return Err(BpfBackendError::UnsupportedTerminator {
                function: FunctionId::from_raw(0),
                block: BlockId::from_raw(0),
            });
        }
    }
    Ok(())
}

fn binary(
    result: &str,
    opcode: &'static str,
    type_text: &'static str,
    left: ValueId,
    right: ValueId,
    values: &BTreeMap<ValueId, LoweredValue>,
) -> Result<String, BpfBackendError> {
    let left = value(left, values)?;
    let right = value(right, values)?;
    Ok(format!(
        "{result} = {opcode} {type_text} {}, {}",
        left.name, right.name
    ))
}

fn compare(
    result: &str,
    predicate: &'static str,
    left: ValueId,
    right: ValueId,
    values: &BTreeMap<ValueId, LoweredValue>,
) -> Result<String, BpfBackendError> {
    let left = value(left, values)?;
    let right = value(right, values)?;
    Ok(format!(
        "{result} = icmp {predicate} {} {}, {}",
        left.type_text, left.name, right.name
    ))
}

fn value(
    value: ValueId,
    values: &BTreeMap<ValueId, LoweredValue>,
) -> Result<&LoweredValue, BpfBackendError> {
    values
        .get(&value)
        .ok_or(BpfBackendError::MissingValue(value))
}

fn bpf_type(type_id: TypeId, types: &TypeArena) -> Result<&'static str, BpfBackendError> {
    match types.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Boolean)) => Ok("i1"),
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Ok(integer_type(*kind)),
        Some(SemanticType::Enum { .. }) => Ok("i32"),
        _ => Err(BpfBackendError::UnsupportedType(type_id)),
    }
}

fn llvm_results(results: &[TypeId], types: &TypeArena) -> Result<&'static str, BpfBackendError> {
    match results {
        [] => Ok("void"),
        [type_id] => bpf_type(*type_id, types),
        [type_id, ..] => Err(BpfBackendError::UnsupportedType(*type_id)),
    }
}

const fn integer_type(kind: IntegerKind) -> &'static str {
    match kind {
        IntegerKind::Int8 | IntegerKind::UInt8 => "i8",
        IntegerKind::Int16 | IntegerKind::UInt16 => "i16",
        IntegerKind::Int32 | IntegerKind::UInt32 => "i32",
        IntegerKind::Int64 | IntegerKind::UInt64 => "i64",
    }
}

fn integer_bits(type_text: &str) -> u8 {
    match type_text.as_bytes() {
        b"i1" => 1,
        b"i8" => 8,
        b"i16" => 16,
        b"i32" => 32,
        b"i64" => 64,
        _ => 64,
    }
}

fn integer_literal(value: IntegerValue) -> String {
    if value.kind().is_signed() {
        value.signed().unwrap_or_default().to_string()
    } else {
        value.unsigned().unwrap_or_default().to_string()
    }
}

fn function_name(bubble: &MirBubble, symbol: SymbolId) -> String {
    format!("pop_b{}_s{}", bubble.bubble().raw(), symbol.raw())
}

#[must_use]
pub const fn xdp_pass() -> i32 {
    XDP_PASS
}
