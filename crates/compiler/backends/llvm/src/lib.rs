//! LLVM-only lowering and native artifact emission boundary.
//!
//! The `Private*` types in this module are deliberately owned by this crate.
//! Canonical MIR never imports them; this is the backend's disposable lowering
//! layer. Textual LLVM IR remains deterministic and inspectable; native object
//! emission parses and verifies that private output with Inkwell before asking
//! LLVM's target machine to write the artifact.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::{self, Write as _};
use std::path::Path;

use inkwell::OptimizationLevel;
use inkwell::context::Context;
use inkwell::memory_buffer::MemoryBuffer;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetTriple,
};
use pop_foundation::{BlockId, FieldId, FunctionId, SymbolId, TypeId, ValueId};
use pop_mir::{
    MirBubble, MirDeclarationKind, MirInstructionKind, MirTerminator, verify_mir_bubble,
};
use pop_runtime_interface::{ArrayElementMap, RuntimeOperation};
use pop_target::TargetSpec;
use pop_types::{FloatKind, IntegerKind, PrimitiveType, SemanticType, TypeArena};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct LlvmLoweringOptions {
    emit_comments: bool,
    entry_point: Option<SymbolId>,
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
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LlvmModule {
    triple: String,
    private: PrivateModule,
}

impl LlvmModule {
    #[must_use]
    pub fn triple(&self) -> &str {
        &self.triple
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
        let mut text = self.to_string().into_bytes();
        text.push(0);
        let buffer = MemoryBuffer::create_from_memory_range_copy(&text, "pop-module");
        let module = context
            .create_module_from_ir(buffer)
            .map_err(|error| LlvmEmissionError::InvalidModule(error.to_string()))?;
        module
            .verify()
            .map_err(|error| LlvmEmissionError::InvalidModule(error.to_string()))?;

        let triple = TargetTriple::create(&self.triple);
        module.set_triple(&triple);
        let target = Target::from_triple(&triple)
            .map_err(|error| LlvmEmissionError::UnsupportedTarget(error.to_string()))?;
        let machine = target
            .create_target_machine(
                &triple,
                "generic",
                "",
                OptimizationLevel::Default,
                RelocMode::PIC,
                CodeModel::Default,
            )
            .ok_or_else(|| LlvmEmissionError::UnsupportedTarget(self.triple.clone()))?;
        module.set_data_layout(&machine.get_target_data().get_data_layout());
        machine
            .write_to_file(&module, FileType::Object, path)
            .map_err(|error| LlvmEmissionError::ObjectEmission(error.to_string()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LlvmEmissionError {
    TargetInitialization(String),
    UnsupportedTarget(String),
    InvalidModule(String),
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
    UnsupportedInstruction {
        function: FunctionId,
        value: ValueId,
    },
    InvalidType(TypeId),
    InvalidFieldLayout(FieldId),
    InvalidEntryPoint(SymbolId),
    UnsupportedEntryPointSignature(SymbolId),
}

impl fmt::Display for LlvmLoweringError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MirVerification(errors) => {
                write!(formatter, "MIR verification failed: {errors:?}")
            }
            Self::UnsupportedInstruction { function, value } => write!(
                formatter,
                "LLVM backend does not support MIR instruction f{} v{}",
                function.raw(),
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
                "entry point s{} must have signature () -> Int",
                symbol.raw()
            ),
        }
    }
}

impl std::error::Error for LlvmLoweringError {}

/// Lowers verified canonical MIR through the LLVM backend's private IR.
///
/// # Errors
///
/// Returns an error when MIR verification fails, a type is invalid, or the
/// requested entry point has an unsupported signature.
pub fn lower_mir_to_llvm_ir(
    bubble: &MirBubble,
    types: &TypeArena,
    target: &TargetSpec,
    options: LlvmLoweringOptions,
) -> Result<LlvmModule, LlvmLoweringError> {
    verify_mir_bubble(bubble, types).map_err(LlvmLoweringError::MirVerification)?;
    let field_layout = collect_field_layout(bubble);
    let string_literals = collect_string_literals(bubble);
    let mut functions = bubble
        .functions()
        .iter()
        .map(|function| lower_function(function, types, options, &field_layout, &string_literals))
        .collect::<Result<Vec<_>, _>>()?;
    for method in bubble.methods() {
        let mut lowered = lower_function(
            method.function(),
            types,
            options,
            &field_layout,
            &string_literals,
        )?;
        lowered.name = format!("pop_method_{}", method.method().raw());
        functions.push(lowered);
    }
    let entry_point = options
        .entry_point
        .map(|symbol| lower_entry_point(symbol, bubble, types))
        .transpose()?;
    let mut declarations = vec![
        format!(
            "declare i64 @{}(i64)",
            RuntimeOperation::AllocateObject.abi_symbol()
        ),
        format!(
            "declare i64 @{}(i64, i1)",
            RuntimeOperation::AllocateArray.abi_symbol()
        ),
        format!(
            "declare void @{}(i32)",
            RuntimeOperation::GcSafePoint.abi_symbol()
        ),
        format!(
            "declare void @{}(i64)",
            RuntimeOperation::RetainRoot.abi_symbol()
        ),
        format!(
            "declare void @{}(i64)",
            RuntimeOperation::ReleaseRoot.abi_symbol()
        ),
        format!(
            "declare void @{}(i64)",
            RuntimeOperation::SatbWriteBarrier.abi_symbol()
        ),
        format!("declare void @{}()", RuntimeOperation::Trap.abi_symbol()),
        format!(
            "declare void @{}()",
            RuntimeOperation::ContinueUnwind.abi_symbol()
        ),
        "declare i64 @pop_rt_string_literal(ptr, i64)".to_owned(),
        "declare i8 @pop_rt_string_equal(i64, i64)".to_owned(),
    ];
    declarations.push("declare void @pop_std_print_int(i64)".to_owned());
    declarations.extend(runtime_declarations());
    Ok(LlvmModule {
        triple: target.triple().to_owned(),
        private: PrivateModule {
            globals: render_string_literals(&string_literals),
            declarations,
            entry_point,
            functions,
        },
    })
}

fn collect_string_literals(bubble: &MirBubble) -> BTreeMap<String, String> {
    let values = bubble
        .functions()
        .iter()
        .chain(bubble.methods().iter().map(pop_mir::MirMethod::function))
        .flat_map(pop_mir::MirFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::StringConstant(value) => Some(value.clone()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| (value, format!("@pop_string_{index}")))
        .collect()
}

fn render_string_literals(literals: &BTreeMap<String, String>) -> Vec<String> {
    literals
        .iter()
        .map(|(value, symbol)| {
            let bytes = value
                .as_bytes()
                .iter()
                .fold(String::new(), |mut output, byte| {
                    let _ = write!(output, "\\{byte:02X}");
                    output
                });
            format!(
                "{symbol} = private unnamed_addr constant [{} x i8] c\"{bytes}\"",
                value.len()
            )
        })
        .collect()
}

fn runtime_declarations() -> Vec<String> {
    [
        RuntimeOperation::AllocateTable,
        RuntimeOperation::ArrayGet,
        RuntimeOperation::FieldGet,
        RuntimeOperation::RecordUpdate,
        RuntimeOperation::CaptureLoad,
        RuntimeOperation::DispatchCall,
    ]
    .into_iter()
    .map(|operation| format!("declare i64 @{}(...)", operation.abi_symbol()))
    .chain(std::iter::once(format!(
        "declare i64 @{}(i64, ...)",
        RuntimeOperation::TupleMake.abi_symbol()
    )))
    .chain(
        [RuntimeOperation::ArraySet, RuntimeOperation::FieldSet]
            .into_iter()
            .map(|operation| format!("declare i8 @{}(...)", operation.abi_symbol())),
    )
    .chain(std::iter::once(format!(
        "declare void @{}(...)",
        RuntimeOperation::CaptureStore.abi_symbol()
    )))
    .collect()
}

fn collect_field_layout(bubble: &MirBubble) -> BTreeMap<FieldId, u32> {
    let mut layout = BTreeMap::new();
    for declaration in bubble.declarations() {
        let fields = match declaration.kind() {
            MirDeclarationKind::Record(record) => record.fields(),
            MirDeclarationKind::Class(class) => class.fields(),
            MirDeclarationKind::Union(_) | MirDeclarationKind::Interface(_) => continue,
        };
        for (slot, field) in fields.iter().enumerate() {
            if let Ok(slot) = u32::try_from(slot) {
                layout.insert(field.field(), slot);
            }
        }
    }
    layout
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PrivateModule {
    globals: Vec<String>,
    declarations: Vec<String>,
    entry_point: Option<String>,
    functions: Vec<PrivateFunction>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PrivateFunction {
    name: String,
    parameters: Vec<String>,
    result: String,
    blocks: Vec<PrivateBlock>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PrivateBlock {
    label: String,
    instructions: Vec<String>,
    terminator: String,
}

impl PrivateModule {
    fn render(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for global in &self.globals {
            writeln!(formatter, "{global}")?;
        }
        if !self.globals.is_empty() {
            writeln!(formatter)?;
        }
        for declaration in &self.declarations {
            writeln!(formatter, "{declaration}")?;
        }
        if !self.declarations.is_empty() {
            writeln!(formatter)?;
        }
        for function in &self.functions {
            function.render(formatter)?;
            writeln!(formatter)?;
        }
        if let Some(entry_point) = &self.entry_point {
            writeln!(formatter, "{entry_point}")?;
        }
        Ok(())
    }
}

fn lower_entry_point(
    symbol: SymbolId,
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let function = bubble
        .functions()
        .iter()
        .find(|function| function.symbol() == symbol)
        .ok_or(LlvmLoweringError::InvalidEntryPoint(symbol))?;
    let int_type = types
        .source_type("Int")
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    if function.parameters().iter().next().is_some() || function.results() != [int_type] {
        return Err(LlvmLoweringError::UnsupportedEntryPointSignature(symbol));
    }
    Ok(format!(
        "define i32 @main() {{\nentry:\n  %pop_exit_value = call i64 @pop_s{}()\n  %pop_exit_code = trunc i64 %pop_exit_value to i32\n  ret i32 %pop_exit_code\n}}",
        symbol.raw()
    ))
}

impl PrivateFunction {
    fn render(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            formatter,
            "define {} @{}({}) {{",
            self.result,
            self.name,
            self.parameters.join(", ")
        )?;
        for block in &self.blocks {
            writeln!(formatter, "{}:", block.label)?;
            for instruction in &block.instructions {
                writeln!(formatter, "  {instruction}")?;
            }
            writeln!(formatter, "  {}", block.terminator)?;
        }
        writeln!(formatter, "}}")
    }
}

fn lower_function(
    function: &pop_mir::MirFunction,
    types: &TypeArena,
    options: LlvmLoweringOptions,
    field_layout: &BTreeMap<FieldId, u32>,
    string_literals: &BTreeMap<String, String>,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let mut value_types = BTreeMap::new();
    for block in function.blocks() {
        for argument in block.arguments() {
            value_types.insert(argument.value(), argument.type_id());
        }
        for instruction in block.instructions() {
            if let Some(type_id) = instruction.optional_result_type() {
                value_types.insert(instruction.result(), type_id);
            }
        }
    }
    let mut incoming_edges: BTreeMap<BlockId, Vec<(BlockId, Vec<ValueId>)>> = BTreeMap::new();
    let mut union_payload_sources = BTreeMap::new();
    let mut has_union_switch = false;
    for predecessor in function.blocks() {
        match predecessor.terminator() {
            MirTerminator::Branch { target, arguments } => {
                incoming_edges
                    .entry(*target)
                    .or_default()
                    .push((predecessor.block(), arguments.clone()));
            }
            MirTerminator::UnionSwitch {
                scrutinee, arms, ..
            } => {
                has_union_switch = true;
                for arm in arms {
                    union_payload_sources.insert(arm.target(), *scrutinee);
                }
            }
            _ => {}
        }
    }
    let mut blocks = Vec::new();
    for block in function.blocks() {
        let mut instructions = lower_block_arguments(
            block,
            incoming_edges.get(&block.block()).map(Vec::as_slice),
            union_payload_sources.get(&block.block()).copied(),
            types,
        )?;
        for instruction in block.instructions() {
            if options.emit_comments {
                instructions.push(format!("; mir v{}", instruction.result().raw()));
            }
            instructions.push(lower_instruction(
                instruction,
                &value_types,
                types,
                field_layout,
                string_literals,
            )?);
        }
        blocks.push(PrivateBlock {
            label: format!("b{}", block.block().raw()),
            instructions,
            terminator: lower_terminator(block.terminator(), &value_types, types)?,
        });
    }
    if has_union_switch {
        blocks.push(PrivateBlock {
            label: "pop_invalid_union".to_owned(),
            instructions: Vec::new(),
            terminator: format!(
                "call void @{}()\n  unreachable",
                RuntimeOperation::Trap.abi_symbol()
            ),
        });
    }
    let parameters = function
        .parameters()
        .iter()
        .enumerate()
        .map(|(index, type_id)| llvm_type(*type_id, types).map(|ty| format!("{ty} %v{index}")))
        .collect::<Result<Vec<_>, LlvmLoweringError>>()?;
    Ok(PrivateFunction {
        name: format!("pop_s{}", function.symbol().raw()),
        parameters,
        result: llvm_results(function.results(), types)?,
        blocks,
    })
}

fn lower_block_arguments(
    block: &pop_mir::MirBlock,
    incoming: Option<&[(BlockId, Vec<ValueId>)]>,
    union_payload_source: Option<ValueId>,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    if let Some(incoming) = incoming {
        return block
            .arguments()
            .iter()
            .enumerate()
            .map(|(index, argument)| {
                let incoming_values = incoming
                    .iter()
                    .map(|(predecessor, values)| {
                        let value = values
                            .get(index)
                            .ok_or(LlvmLoweringError::InvalidType(argument.type_id()))?;
                        Ok(format!("[ %v{}, %b{} ]", value.raw(), predecessor.raw()))
                    })
                    .collect::<Result<Vec<_>, LlvmLoweringError>>()?;
                Ok(format!(
                    "%v{} = phi {} {}",
                    argument.value().raw(),
                    llvm_type(argument.type_id(), types)?,
                    incoming_values.join(", ")
                ))
            })
            .collect();
    }
    let Some(scrutinee) = union_payload_source else {
        return Ok(Vec::new());
    };
    let mut instructions = Vec::new();
    for (index, argument) in block.arguments().iter().enumerate() {
        instructions.extend(lower_runtime_slot_load(
            argument.value(),
            argument.type_id(),
            scrutinee,
            index + 2,
            types,
        )?);
    }
    Ok(instructions)
}

#[allow(clippy::too_many_lines)]
fn lower_instruction(
    instruction: &pop_mir::MirInstruction,
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    string_literals: &BTreeMap<String, String>,
) -> Result<String, LlvmLoweringError> {
    let result = format!("%v{}", instruction.result().raw());
    let result_type = instruction.optional_result_type();
    let binary = |operator: &str, left: ValueId, right: ValueId, kind: IntegerKind| {
        format!(
            "{result} = {operator} i{} %v{}, %v{}",
            kind.bit_width(),
            left.raw(),
            right.raw()
        )
    };
    let line = match instruction.kind() {
        MirInstructionKind::IntegerConstant(value) => format!(
            "{result} = add i{} 0, {}",
            value.kind().bit_width(),
            integer_literal(*value)
        ),
        MirInstructionKind::FloatConstant(value) => format!(
            "{result} = fadd {} 0.0, 0x{:016X}",
            float_type(value.kind()),
            value.as_f64().to_bits()
        ),
        MirInstructionKind::BooleanConstant(value) => {
            format!("{result} = xor i1 0, {}", u8::from(*value))
        }
        MirInstructionKind::NilConstant => format!("{result} = add i64 0, 0"),
        MirInstructionKind::StringConstant(value) => {
            let symbol = string_literals
                .get(value)
                .ok_or(LlvmLoweringError::InvalidType(instruction.result_type()))?;
            format!(
                "{result} = call i64 @pop_rt_string_literal(ptr {symbol}, i64 {})",
                value.len()
            )
        }
        MirInstructionKind::CheckedIntegerAdd { kind, left, right } => {
            binary("add", *left, *right, *kind)
        }
        MirInstructionKind::CheckedIntegerSubtract { kind, left, right } => {
            binary("sub", *left, *right, *kind)
        }
        MirInstructionKind::CheckedIntegerMultiply { kind, left, right } => {
            binary("mul", *left, *right, *kind)
        }
        MirInstructionKind::CheckedIntegerDivide { kind, left, right } => binary(
            if kind.is_signed() { "sdiv" } else { "udiv" },
            *left,
            *right,
            *kind,
        ),
        MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => binary(
            if kind.is_signed() { "srem" } else { "urem" },
            *left,
            *right,
            *kind,
        ),
        MirInstructionKind::IntegerNegate { kind, operand } => format!(
            "{result} = sub i{} 0, %v{}",
            kind.bit_width(),
            operand.raw()
        ),
        MirInstructionKind::FloatAdd { kind, left, right } => format!(
            "{result} = fadd {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatSubtract { kind, left, right } => format!(
            "{result} = fsub {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatMultiply { kind, left, right } => format!(
            "{result} = fmul {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatDivide { kind, left, right } => format!(
            "{result} = fdiv {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FloatNegate { kind, operand } => {
            format!("{result} = fneg {} %v{}", float_type(*kind), operand.raw())
        }
        MirInstructionKind::BooleanNot { operand } => {
            format!("{result} = xor i1 %v{}, true", operand.raw())
        }
        MirInstructionKind::BooleanAnd { left, right } => {
            format!("{result} = and i1 %v{}, %v{}", left.raw(), right.raw())
        }
        MirInstructionKind::BooleanOr { left, right } => {
            format!("{result} = or i1 %v{}, %v{}", left.raw(), right.raw())
        }
        MirInstructionKind::CompareEqual { left, right } => {
            lower_equality(&result, *left, *right, false, value_types, types)?
        }
        MirInstructionKind::CompareNotEqual { left, right } => {
            lower_equality(&result, *left, *right, true, value_types, types)?
        }
        MirInstructionKind::CompareIntegerLess { kind, left, right } => format!(
            "{result} = icmp {} i{} %v{}, %v{}",
            if kind.is_signed() { "slt" } else { "ult" },
            kind.bit_width(),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareIntegerGreater { kind, left, right } => format!(
            "{result} = icmp {} i{} %v{}, %v{}",
            if kind.is_signed() { "sgt" } else { "ugt" },
            kind.bit_width(),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareFloatLess { kind, left, right } => format!(
            "{result} = fcmp olt {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::CompareFloatGreater { kind, left, right } => format!(
            "{result} = fcmp ogt {} %v{}, %v{}",
            float_type(*kind),
            left.raw(),
            right.raw()
        ),
        MirInstructionKind::FunctionReference(symbol) => {
            format!(
                "{result} = select i1 true, ptr @pop_s{}, ptr null",
                symbol.raw()
            )
        }
        MirInstructionKind::CallDirect {
            function: callee,
            arguments,
            ..
        } => call_line(
            &result,
            result_type,
            &format!("@pop_s{}", callee.raw()),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallStandard {
            function,
            arguments,
            ..
        } => {
            if function.raw() != 0 || arguments.len() != 1 {
                return Err(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                });
            }
            format!("call void @pop_std_print_int(i64 %v{})", arguments[0].raw())
        }
        MirInstructionKind::GcSafePoint { safe_point, .. } => format!(
            "call void @{}(i32 {})",
            RuntimeOperation::GcSafePoint.abi_symbol(),
            safe_point.raw()
        ),
        MirInstructionKind::RetainRoot { value } => format!(
            "call void @{}(i64 %v{})",
            RuntimeOperation::RetainRoot.abi_symbol(),
            value.raw()
        ),
        MirInstructionKind::ReleaseRoot { value } => format!(
            "call void @{}(i64 %v{})",
            RuntimeOperation::ReleaseRoot.abi_symbol(),
            value.raw()
        ),
        MirInstructionKind::WriteBarrier { owner, .. } => format!(
            "call void @{}(i64 %v{})",
            RuntimeOperation::SatbWriteBarrier.abi_symbol(),
            owner.raw()
        ),
        MirInstructionKind::CaptureCellAllocate { .. }
        | MirInstructionKind::ClosureEnvironmentAllocate { .. } => format!(
            "{result} = call i64 @{}(i64 0)",
            RuntimeOperation::AllocateObject.abi_symbol()
        ),
        MirInstructionKind::ArrayMake {
            elements,
            element_map,
        } => lower_array_make(&result, elements, *element_map, value_types, types)?,
        MirInstructionKind::TableMake { entries, .. } => format!(
            "{result} = call i64 @{}(i64 {})",
            RuntimeOperation::AllocateTable.abi_symbol(),
            entries.len()
        ),
        MirInstructionKind::RecordMake { fields, .. } => {
            let slot_count = u32::try_from(fields.len())
                .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            lower_object_make(
                &result,
                fields,
                slot_count,
                value_types,
                types,
                field_layout,
            )?
        }
        MirInstructionKind::ClassMake {
            fields, object_map, ..
        } => lower_object_make(
            &result,
            fields,
            object_map.slot_count(),
            value_types,
            types,
            field_layout,
        )?,
        MirInstructionKind::CallDirectMethod {
            method, arguments, ..
        } => call_line(
            &result,
            result_type,
            &format!("@pop_method_{}", method.raw()),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallInterface {
            interface,
            method,
            arguments,
            ..
        } => call_line(
            &result,
            result_type,
            &format!("@pop_interface_{}_{}", interface.raw(), method.raw()),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallIndirect {
            callee, arguments, ..
        } => indirect_call_line(&result, result_type, *callee, arguments, value_types, types)?,
        MirInstructionKind::TupleMake(elements) => runtime_call_with_count(
            &result,
            result_type,
            RuntimeOperation::TupleMake,
            elements.len(),
            elements,
            value_types,
            types,
        )?,
        MirInstructionKind::ArrayGet { array, index } => runtime_call(
            &result,
            result_type,
            RuntimeOperation::ArrayGet,
            &[*array, *index],
            value_types,
            types,
        )?,
        MirInstructionKind::RecordUpdate { base, fields, .. } => {
            let arguments = std::iter::once(*base)
                .chain(fields.iter().map(|(_, value)| *value))
                .collect::<Vec<_>>();
            runtime_call(
                &result,
                result_type,
                RuntimeOperation::RecordUpdate,
                &arguments,
                value_types,
                types,
            )?
        }
        MirInstructionKind::FieldGet { base, field } => runtime_field_call(
            &result,
            result_type,
            RuntimeOperation::FieldGet,
            *base,
            *field,
            None,
            value_types,
            types,
            field_layout,
        )?,
        MirInstructionKind::FieldSet { base, field, value } => runtime_field_call(
            &result,
            result_type,
            RuntimeOperation::FieldSet,
            *base,
            *field,
            Some(*value),
            value_types,
            types,
            field_layout,
        )?,
        MirInstructionKind::UnionMake {
            case, arguments, ..
        } => lower_union_make(&result, *case, arguments, value_types, types)?,
        MirInstructionKind::InterfaceUpcast { value, .. } => runtime_call(
            &result,
            result_type,
            RuntimeOperation::FieldGet,
            &[*value],
            value_types,
            types,
        )?,
        MirInstructionKind::CaptureCellLoad { cell } => runtime_call(
            &result,
            result_type,
            RuntimeOperation::CaptureLoad,
            &[*cell],
            value_types,
            types,
        )?,
        MirInstructionKind::CaptureCellStore { cell, value } => runtime_call(
            &result,
            result_type,
            RuntimeOperation::CaptureStore,
            &[*cell, *value],
            value_types,
            types,
        )?,
        MirInstructionKind::CaptureLoad { .. }
        | MirInstructionKind::CaptureCellReference { .. } => runtime_call(
            &result,
            result_type,
            RuntimeOperation::CaptureLoad,
            &[],
            value_types,
            types,
        )?,
        MirInstructionKind::CaptureStore { value, .. } => runtime_call(
            &result,
            result_type,
            RuntimeOperation::CaptureStore,
            &[*value],
            value_types,
            types,
        )?,
    };
    Ok(line)
}

fn lower_terminator(
    terminator: &MirTerminator,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    Ok(match terminator {
        MirTerminator::Branch { target, .. } => format!("br label %b{}", target.raw()),
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => format!(
            "br i1 %v{}, label %b{}, label %b{}",
            condition.raw(),
            when_true.raw(),
            when_false.raw()
        ),
        MirTerminator::Return { values: returned } if returned.is_empty() => "ret void".to_owned(),
        MirTerminator::Return { values: returned } => {
            let value = returned[0];
            format!(
                "ret {} %v{}",
                llvm_value_type(values, value, types)?,
                value.raw()
            )
        }
        MirTerminator::Trap(_) => format!(
            "call void @{}()\n  unreachable",
            RuntimeOperation::Trap.abi_symbol()
        ),
        MirTerminator::Panic(_) | MirTerminator::ContinueUnwind(_) => format!(
            "call void @{}()\n  unreachable",
            RuntimeOperation::ContinueUnwind.abi_symbol()
        ),
        MirTerminator::Unreachable | MirTerminator::Missing => "unreachable".to_owned(),
        MirTerminator::UnionSwitch {
            scrutinee, arms, ..
        } => {
            let tag = format!("%v{}_union_tag", scrutinee.raw());
            let cases = arms
                .iter()
                .map(|arm| {
                    format!(
                        "    i64 {}, label %b{}",
                        arm.case().raw(),
                        arm.target().raw()
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            format!(
                "{tag} = call i64 @{}(i64 %v{}, i64 1)\n  switch i64 {tag}, label %pop_invalid_union [\n{cases}\n  ]",
                RuntimeOperation::FieldGet.abi_symbol(),
                scrutinee.raw()
            )
        }
    })
}

fn lower_equality(
    result: &str,
    left: ValueId,
    right: ValueId,
    negated: bool,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let type_id = *values
        .get(&left)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    if types.get(type_id) == Some(&SemanticType::Primitive(PrimitiveType::String)) {
        let equal = format!("{result}_string_equal");
        return Ok(format!(
            "{equal} = call i8 @pop_rt_string_equal(i64 %v{}, i64 %v{})\n{result} = icmp {} i8 {equal}, 0",
            left.raw(),
            right.raw(),
            if negated { "eq" } else { "ne" }
        ));
    }
    let ty = llvm_value_type(values, left, types)?;
    let operator = match (ty.as_str(), negated) {
        ("float" | "double", false) => "fcmp oeq",
        ("float" | "double", true) => "fcmp une",
        (_, false) => "icmp eq",
        (_, true) => "icmp ne",
    };
    Ok(format!(
        "{result} = {operator} {ty} %v{}, %v{}",
        left.raw(),
        right.raw()
    ))
}

fn call_line(
    result: &str,
    result_type: Option<TypeId>,
    callee: &str,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let args = arguments
        .iter()
        .map(|value| {
            llvm_value_type(values, *value, types).map(|ty| format!("{ty} %v{}", value.raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let assignment = result_type.map_or_else(String::new, |_| format!("{result} = "));
    let return_type =
        result_type.map_or_else(|| Ok("void".to_owned()), |id| llvm_type(id, types))?;
    Ok(format!(
        "{assignment}call {return_type} {callee}({})",
        args.join(", ")
    ))
}

fn indirect_call_line(
    result: &str,
    result_type: Option<TypeId>,
    callee: ValueId,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let args = arguments
        .iter()
        .map(|value| {
            llvm_value_type(values, *value, types).map(|ty| format!("{ty} %v{}", value.raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let assignment = result_type.map_or_else(String::new, |_| format!("{result} = "));
    let return_type =
        result_type.map_or_else(|| Ok("void".to_owned()), |id| llvm_type(id, types))?;
    Ok(format!(
        "{assignment}call {return_type} %v{}({})",
        callee.raw(),
        args.join(", ")
    ))
}

fn runtime_call(
    result: &str,
    result_type: Option<TypeId>,
    operation: RuntimeOperation,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let args = arguments
        .iter()
        .map(|value| {
            llvm_value_type(values, *value, types).map(|ty| format!("{ty} %v{}", value.raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let return_type =
        result_type.map_or_else(|| Ok("void".to_owned()), |id| llvm_type(id, types))?;
    let assignment = result_type.map_or_else(String::new, |_| format!("{result} = "));
    Ok(format!(
        "{assignment}call {return_type} @{}({})",
        operation.abi_symbol(),
        args.join(", ")
    ))
}

fn lower_object_make(
    result: &str,
    fields: &[(FieldId, ValueId)],
    slot_count: u32,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 {})",
        RuntimeOperation::AllocateObject.abi_symbol(),
        slot_count
    )];
    for (field, value) in fields {
        let slot = field_layout
            .get(field)
            .ok_or(LlvmLoweringError::InvalidFieldLayout(*field))?;
        if llvm_value_type(values, *value, types)? != "i64" {
            return Err(LlvmLoweringError::InvalidType(*values.get(value).ok_or(
                LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)),
            )?));
        }
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 %v{})",
            RuntimeOperation::FieldSet.abi_symbol(),
            slot + 1,
            value.raw()
        ));
    }
    Ok(lines.join("\n"))
}

fn lower_union_make(
    result: &str,
    case: pop_foundation::UnionCaseId,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 {})",
        RuntimeOperation::AllocateObject.abi_symbol(),
        arguments.len() + 1
    )];
    lines.push(format!(
        "call i8 @{}(i64 {result}, i64 1, i64 {})",
        RuntimeOperation::FieldSet.abi_symbol(),
        case.raw()
    ));
    for (index, value) in arguments.iter().enumerate() {
        let type_id = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let ty = llvm_type(type_id, types)?;
        let (conversions, stored) = lower_runtime_slot_store(*value, type_id, &ty)?;
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            RuntimeOperation::FieldSet.abi_symbol(),
            index + 2
        ));
    }
    Ok(lines.join("\n"))
}

fn lower_runtime_slot_store(
    value: ValueId,
    type_id: TypeId,
    ty: &str,
) -> Result<(Vec<String>, String), LlvmLoweringError> {
    let source = format!("%v{}", value.raw());
    let converted = format!("%v{}_slot", value.raw());
    match ty {
        "i64" => Ok((Vec::new(), source)),
        "i1" | "i8" | "i16" | "i32" => Ok((
            vec![format!("{converted} = zext {ty} {source} to i64")],
            converted,
        )),
        "float" => Ok((
            vec![
                format!("{converted}_bits = bitcast float {source} to i32"),
                format!("{converted} = zext i32 {converted}_bits to i64"),
            ],
            converted,
        )),
        "double" => Ok((
            vec![format!("{converted} = bitcast double {source} to i64")],
            converted,
        )),
        "ptr" => Ok((
            vec![format!("{converted} = ptrtoint ptr {source} to i64")],
            converted,
        )),
        _ => Err(LlvmLoweringError::InvalidType(type_id)),
    }
}

fn lower_runtime_slot_load(
    result: ValueId,
    result_type: TypeId,
    owner: ValueId,
    slot: usize,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    let ty = llvm_type(result_type, types)?;
    let result = format!("%v{}", result.raw());
    let loaded = format!("{result}_slot");
    let call = format!(
        "call i64 @{}(i64 %v{}, i64 {slot})",
        RuntimeOperation::FieldGet.abi_symbol(),
        owner.raw()
    );
    Ok(match ty.as_str() {
        "i64" => vec![format!("{result} = {call}")],
        "i1" | "i8" | "i16" | "i32" => vec![
            format!("{loaded} = {call}"),
            format!("{result} = trunc i64 {loaded} to {ty}"),
        ],
        "float" => vec![
            format!("{loaded} = {call}"),
            format!("{loaded}_bits = trunc i64 {loaded} to i32"),
            format!("{result} = bitcast i32 {loaded}_bits to float"),
        ],
        "double" => vec![
            format!("{loaded} = {call}"),
            format!("{result} = bitcast i64 {loaded} to double"),
        ],
        "ptr" => vec![
            format!("{loaded} = {call}"),
            format!("{result} = inttoptr i64 {loaded} to ptr"),
        ],
        _ => return Err(LlvmLoweringError::InvalidType(result_type)),
    })
}

#[allow(clippy::too_many_arguments)]
fn runtime_field_call(
    result: &str,
    result_type: Option<TypeId>,
    operation: RuntimeOperation,
    base: ValueId,
    field: FieldId,
    value: Option<ValueId>,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    let slot = field_layout
        .get(&field)
        .ok_or(LlvmLoweringError::InvalidFieldLayout(field))?;
    let base_type = llvm_value_type(values, base, types)?;
    if base_type != "i64" {
        return Err(LlvmLoweringError::InvalidType(*values.get(&base).ok_or(
            LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)),
        )?));
    }
    let assignment = result_type.map_or_else(String::new, |_| format!("{result} = "));
    let return_type = result_type.map_or_else(|| Ok("i8".to_owned()), |id| llvm_type(id, types))?;
    let value_text = value
        .map(|value| format!(", i64 %v{}", value.raw()))
        .unwrap_or_default();
    Ok(format!(
        "{assignment}call {return_type} @{}(i64 %v{}, i64 {}{})",
        operation.abi_symbol(),
        base.raw(),
        slot + 1,
        value_text
    ))
}

fn lower_array_make(
    result: &str,
    elements: &[ValueId],
    element_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 {}, {})",
        RuntimeOperation::AllocateArray.abi_symbol(),
        elements.len(),
        if matches!(element_map, ArrayElementMap::ManagedReference) {
            "i1 1"
        } else {
            "i1 0"
        }
    )];
    for (index, value) in elements.iter().enumerate() {
        let value_type = llvm_value_type(values, *value, types)?;
        if value_type != "i64" {
            return Err(LlvmLoweringError::InvalidType(*values.get(value).ok_or(
                LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)),
            )?));
        }
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 %v{})",
            RuntimeOperation::ArraySet.abi_symbol(),
            index + 1,
            value.raw()
        ));
    }
    Ok(lines.join("\n"))
}

fn runtime_call_with_count(
    result: &str,
    result_type: Option<TypeId>,
    operation: RuntimeOperation,
    count: usize,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let args = arguments
        .iter()
        .map(|value| {
            llvm_value_type(values, *value, types).map(|ty| format!("{ty} %v{}", value.raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let return_type =
        result_type.map_or_else(|| Ok("void".to_owned()), |id| llvm_type(id, types))?;
    let assignment = result_type.map_or_else(String::new, |_| format!("{result} = "));
    let arguments = if args.is_empty() {
        count.to_string()
    } else {
        format!("{count}, {}", args.join(", "))
    };
    Ok(format!(
        "{assignment}call {return_type} @{}(i64 {})",
        operation.abi_symbol(),
        arguments
    ))
}

fn llvm_results(results: &[TypeId], types: &TypeArena) -> Result<String, LlvmLoweringError> {
    match results {
        [] => Ok("void".to_owned()),
        [result] => llvm_type(*result, types),
        _ => Ok(format!(
            "{{ {} }}",
            results
                .iter()
                .map(|id| llvm_type(*id, types))
                .collect::<Result<Vec<_>, _>>()?
                .join(", ")
        )),
    }
}

fn llvm_value_type(
    values: &BTreeMap<ValueId, TypeId>,
    value: ValueId,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    llvm_type(
        *values
            .get(&value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        types,
    )
}

fn llvm_type(type_id: TypeId, types: &TypeArena) -> Result<String, LlvmLoweringError> {
    match types
        .get(type_id)
        .ok_or(LlvmLoweringError::InvalidType(type_id))?
    {
        SemanticType::Primitive(PrimitiveType::Boolean) => Ok("i1".to_owned()),
        SemanticType::Primitive(PrimitiveType::Integer(kind)) => {
            Ok(format!("i{}", kind.bit_width()))
        }
        SemanticType::Primitive(PrimitiveType::Float32) => Ok("float".to_owned()),
        SemanticType::Primitive(PrimitiveType::Float64) => Ok("double".to_owned()),
        SemanticType::Primitive(PrimitiveType::Never) => Ok("void".to_owned()),
        SemanticType::Function { .. } => Ok("ptr".to_owned()),
        _ => Ok("i64".to_owned()),
    }
}

fn integer_literal(value: pop_types::IntegerValue) -> String {
    if value.kind().is_signed() {
        value.signed().unwrap_or_default().to_string()
    } else {
        value.unsigned().unwrap_or_default().to_string()
    }
}
fn float_type(kind: FloatKind) -> &'static str {
    match kind {
        FloatKind::Float32 => "float",
        FloatKind::Float64 => "double",
    }
}
