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
use inkwell::passes::PassBuilderOptions;
use inkwell::targets::{
    CodeModel, FileType, InitializationConfig, RelocMode, Target, TargetMachine, TargetTriple,
};
use pop_foundation::{BlockId, BubbleId, ClassId, FieldId, FunctionId, SymbolId, TypeId, ValueId};
use pop_mir::{
    MirBubble, MirDeclarationKind, MirEffect, MirEffectSummary, MirInstructionKind, MirTerminator,
    verify_mir_bubble,
};
use pop_runtime_interface::{ArrayElementMap, RuntimeOperation};
use pop_target::TargetSpec;
use pop_types::{FloatKind, IntegerKind, PrimitiveType, SemanticType, TypeArena};

const LLVM_OPTIMIZATION_PIPELINE: &str = "default<O3>";
const GC_POLL_INTERVAL: u32 = 16_384;
const GC_POLL_BUDGET: &str = "%pop_gc_poll_budget";

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
                "entry point s{} must accept () or (Array<String>) and return () or Int",
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
#[allow(clippy::too_many_lines)]
pub fn lower_mir_to_llvm_ir(
    bubble: &MirBubble,
    types: &TypeArena,
    target: &TargetSpec,
    options: LlvmLoweringOptions,
) -> Result<LlvmModule, LlvmLoweringError> {
    verify_mir_bubble(bubble, types).map_err(LlvmLoweringError::MirVerification)?;
    let field_layout = collect_field_layout(bubble);
    let record_fields = collect_record_fields(bubble);
    let record_field_types = collect_record_field_types(bubble);
    let string_literals = collect_string_literals(bubble);
    let self_capture_slots = collect_self_capture_slots(bubble);
    let memory_none_functions = analyze_memory_none_functions(bubble);
    let mut functions = bubble
        .functions()
        .iter()
        .map(|function| {
            lower_function(
                bubble.bubble(),
                function,
                types,
                options,
                memory_none_functions.contains(&function.symbol()),
                &field_layout,
                &record_fields,
                &record_field_types,
                &string_literals,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    for method in bubble.methods() {
        let mut lowered = lower_function(
            bubble.bubble(),
            method.function(),
            types,
            options,
            false,
            &field_layout,
            &record_fields,
            &record_field_types,
            &string_literals,
        )?;
        lowered.name = method_name(bubble.bubble(), method.method());
        functions.push(lowered);
    }
    for nested in bubble.nested_functions() {
        let self_slots = self_capture_slots
            .get(&(nested.owner(), nested.function()))
            .cloned()
            .unwrap_or_default();
        functions.push(lower_function_parts(
            bubble.bubble(),
            nested_name(bubble.bubble(), nested.owner(), nested.function()),
            nested.parameters(),
            nested.results(),
            nested.effects(),
            false,
            nested.blocks(),
            Some(("%environment", &self_slots)),
            types,
            options,
            &field_layout,
            &record_fields,
            &record_field_types,
            &string_literals,
        )?);
    }
    functions.push(direct_scalar_array_fill_function());
    functions.extend(lower_interface_dispatchers(bubble, types)?);
    functions.extend(lower_indirect_dispatchers(bubble, types)?);
    let entry_point = options
        .entry_point
        .map(|symbol| lower_entry_point(symbol, bubble, types))
        .transpose()?;
    let mut declarations = vec![
        format!(
            "declare i64 @{}(i64)",
            RuntimeOperation::AllocateObject.abi_symbol()
        ),
        "declare i64 @pop_rt_allocate_mapped_object(i64, ptr, i64)".to_owned(),
        format!(
            "declare i64 @{}(i64, i1)",
            RuntimeOperation::AllocateArray.abi_symbol()
        ),
        format!(
            "declare i64 @{}(i64, i1, i64)",
            RuntimeOperation::AllocateArrayFilled.abi_symbol()
        ),
        format!(
            "declare i64 @{}(i64, i1, i1)",
            RuntimeOperation::AllocateTable.abi_symbol()
        ),
        format!(
            "declare i8 @{}(i32, ptr, i64) cold nounwind",
            RuntimeOperation::GcSafePoint.abi_symbol()
        ),
        format!(
            "declare i64 @{}(i64)",
            RuntimeOperation::RetainRoot.abi_symbol()
        ),
        format!(
            "declare i8 @{}(i64)",
            RuntimeOperation::ReleaseRoot.abi_symbol()
        ),
        format!("declare i64 @{}(i64)", RuntimeOperation::Pin.abi_symbol()),
        format!("declare i8 @{}(i64)", RuntimeOperation::Unpin.abi_symbol()),
        format!(
            "declare void @{}(i64)",
            RuntimeOperation::SatbWriteBarrier.abi_symbol()
        ),
        format!(
            "declare void @{}() cold noreturn nounwind",
            RuntimeOperation::Trap.abi_symbol()
        ),
        format!(
            "declare void @{}()",
            RuntimeOperation::ContinueUnwind.abi_symbol()
        ),
        "declare i64 @pop_rt_string_literal(ptr, i64)".to_owned(),
        "declare i8 @pop_rt_string_equal(i64, i64)".to_owned(),
        "declare i64 @pop_rt_process_arguments(i32, ptr)".to_owned(),
        "declare i1 @llvm.expect.i1(i1, i1)".to_owned(),
        "declare noalias ptr @malloc(i64) nounwind".to_owned(),
        "declare void @free(ptr) nounwind".to_owned(),
    ];
    declarations.push("declare void @pop_std_print_int(i64)".to_owned());
    declarations.push("declare void @pop_std_print_string(i64)".to_owned());
    declarations.extend(runtime_declarations());
    declarations.extend(checked_integer_declarations());
    Ok(LlvmModule {
        triple: target.triple().to_owned(),
        private: PrivateModule {
            globals: render_string_literals(&string_literals),
            declarations,
            entry_point,
            functions,
            functions_internal: options.entry_point.is_some(),
        },
    })
}

fn direct_scalar_array_fill_function() -> PrivateFunction {
    PrivateFunction {
        name: "pop_llvm_fill_scalar_array".to_owned(),
        parameters: vec![
            "ptr %storage".to_owned(),
            "i64 %length".to_owned(),
            "i64 %value".to_owned(),
        ],
        result: "void".to_owned(),
        blocks: vec![
            PrivateBlock {
                label: "entry".to_owned(),
                instructions: vec!["%empty = icmp eq i64 %length, 0".to_owned()],
                terminator: "br i1 %empty, label %done, label %fill".to_owned(),
            },
            PrivateBlock {
                label: "fill".to_owned(),
                instructions: vec![
                    "%index = phi i64 [ 0, %entry ], [ %next, %fill ]".to_owned(),
                    "%slot = getelementptr i64, ptr %storage, i64 %index".to_owned(),
                    "store i64 %value, ptr %slot, align 8".to_owned(),
                    "%next = add nuw i64 %index, 1".to_owned(),
                    "%filled = icmp eq i64 %next, %length".to_owned(),
                ],
                terminator: "br i1 %filled, label %done, label %fill".to_owned(),
            },
            PrivateBlock {
                label: "done".to_owned(),
                instructions: Vec::new(),
                terminator: "ret void".to_owned(),
            },
        ],
        attributes: vec!["nounwind"],
    }
}

fn checked_integer_declarations() -> Vec<String> {
    [8_u16, 16, 32, 64]
        .into_iter()
        .flat_map(|bits| {
            ["sadd", "uadd", "ssub", "usub", "smul", "umul"].map(move |operation| {
                format!(
                    "declare {{ i{bits}, i1 }} @llvm.{operation}.with.overflow.i{bits}(i{bits}, i{bits})"
                )
            })
        })
        .collect()
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
    let nested_values = bubble
        .nested_functions()
        .iter()
        .flat_map(pop_mir::MirNestedFunction::blocks)
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::StringConstant(value) => Some(value.clone()),
            _ => None,
        });
    let values = values
        .into_iter()
        .chain(nested_values)
        .collect::<BTreeSet<_>>();
    values
        .into_iter()
        .enumerate()
        .map(|(index, value)| (value, format!("@pop_string_{index}")))
        .collect()
}

fn analyze_memory_none_functions(bubble: &MirBubble) -> BTreeSet<SymbolId> {
    let mut candidates = bubble
        .functions()
        .iter()
        .filter(|function| {
            function
                .effects()
                .is_subset_of(MirEffectSummary::empty().with(MirEffect::MayTrap))
                && function
                    .blocks()
                    .iter()
                    .flat_map(pop_mir::MirBlock::instructions)
                    .all(|instruction| {
                        matches!(instruction.kind(), MirInstructionKind::CallDirect { .. })
                            || llvm_memory_none_instruction(instruction.kind())
                    })
        })
        .map(pop_mir::MirFunction::symbol)
        .collect::<BTreeSet<_>>();
    loop {
        let rejected = bubble
            .functions()
            .iter()
            .filter(|function| candidates.contains(&function.symbol()))
            .filter(|function| {
                function
                    .blocks()
                    .iter()
                    .flat_map(pop_mir::MirBlock::instructions)
                    .any(|instruction| {
                        matches!(
                            instruction.kind(),
                            MirInstructionKind::CallDirect { function, .. }
                                if !candidates.contains(function)
                        )
                    })
            })
            .map(pop_mir::MirFunction::symbol)
            .collect::<Vec<_>>();
        if rejected.is_empty() {
            return candidates;
        }
        for function in rejected {
            candidates.remove(&function);
        }
    }
}

fn collect_self_capture_slots(
    bubble: &MirBubble,
) -> BTreeMap<(SymbolId, pop_foundation::NestedFunctionId), BTreeSet<u32>> {
    let mut slots = BTreeMap::new();
    for instruction in bubble
        .functions()
        .iter()
        .map(pop_mir::MirFunction::blocks)
        .chain(
            bubble
                .methods()
                .iter()
                .map(|method| method.function().blocks()),
        )
        .chain(
            bubble
                .nested_functions()
                .iter()
                .map(pop_mir::MirNestedFunction::blocks),
        )
        .flatten()
        .flat_map(pop_mir::MirBlock::instructions)
    {
        if let MirInstructionKind::ClosureEnvironmentAllocate {
            owner,
            function,
            captures,
            ..
        } = instruction.kind()
        {
            slots
                .entry((*owner, *function))
                .or_insert_with(BTreeSet::new)
                .extend(
                    captures
                        .iter()
                        .filter(|capture| capture.self_reference())
                        .map(|capture| capture.slot()),
                );
        }
    }
    slots
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
    vec![
        format!(
            "declare i64 @{}(i64, i64) nounwind",
            RuntimeOperation::ArrayGet.abi_symbol()
        ),
        format!(
            "declare i8 @{}(i64, ptr) nounwind",
            RuntimeOperation::ArrayLength.abi_symbol()
        ),
        format!(
            "declare i8 @{}(i64, i64, ptr) nounwind",
            RuntimeOperation::ArrayGetChecked.abi_symbol()
        ),
        format!(
            "declare i64 @{}(i64, i64) nounwind",
            RuntimeOperation::FieldGet.abi_symbol()
        ),
        format!(
            "declare i8 @{}(i64, i64, i64) nounwind",
            RuntimeOperation::ArraySet.abi_symbol()
        ),
        format!(
            "declare i8 @{}(i64, i64) nounwind",
            RuntimeOperation::ArrayFill.abi_symbol()
        ),
        format!(
            "declare i8 @{}(i64, i64, i64) nounwind",
            RuntimeOperation::FieldSet.abi_symbol()
        ),
    ]
}

fn collect_field_layout(bubble: &MirBubble) -> BTreeMap<FieldId, u32> {
    let mut layout = BTreeMap::new();
    for declaration in bubble.declarations() {
        let (fields, reserved_slots) = match declaration.kind() {
            MirDeclarationKind::Record(record) => (record.fields(), 0_u32),
            MirDeclarationKind::Class(class) => (class.fields(), 1_u32),
            MirDeclarationKind::Union(_) | MirDeclarationKind::Interface(_) => continue,
        };
        for (slot, field) in fields.iter().enumerate() {
            if let Ok(slot) = u32::try_from(slot) {
                layout.insert(field.field(), slot + reserved_slots + 1);
            }
        }
    }
    layout
}

fn collect_record_fields(bubble: &MirBubble) -> BTreeMap<SymbolId, Vec<FieldId>> {
    bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Record(record) => Some((
                declaration.symbol(),
                record
                    .fields()
                    .iter()
                    .map(pop_mir::MirField::field)
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

fn collect_record_field_types(bubble: &MirBubble) -> BTreeMap<TypeId, Vec<TypeId>> {
    bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Record(record) => Some((
                record.type_id(),
                record
                    .fields()
                    .iter()
                    .map(pop_mir::MirField::field_type)
                    .collect(),
            )),
            _ => None,
        })
        .collect()
}

fn lower_interface_dispatchers(
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    let classes = bubble
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            MirDeclarationKind::Class(class) => Some(class),
            _ => None,
        })
        .collect::<Vec<_>>();
    let mut dispatchers = Vec::new();
    for interface in
        bubble
            .declarations()
            .iter()
            .filter_map(|declaration| match declaration.kind() {
                MirDeclarationKind::Interface(interface) => Some(interface),
                _ => None,
            })
    {
        for method in interface.methods() {
            let implementations = classes
                .iter()
                .filter_map(|class| {
                    class
                        .interfaces()
                        .iter()
                        .find(|implementation| implementation.interface() == interface.interface())
                        .and_then(|implementation| {
                            implementation.methods().iter().find(|implementation| {
                                implementation.interface_method() == method.method()
                            })
                        })
                        .map(|implementation| (class.class(), implementation.class_method()))
                })
                .collect::<Vec<_>>();
            dispatchers.push(lower_interface_dispatcher(
                bubble.bubble(),
                interface.interface(),
                method,
                &implementations,
                types,
            )?);
        }
    }
    Ok(dispatchers)
}

fn lower_interface_dispatcher(
    bubble: BubbleId,
    interface: pop_foundation::InterfaceId,
    method: &pop_mir::MirInterfaceMethod,
    implementations: &[(ClassId, pop_foundation::MethodId)],
    types: &TypeArena,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let result_type = llvm_results(method.results(), types)?;
    let mut parameters = vec!["i64 %v0".to_owned()];
    parameters.extend(
        method
            .parameters()
            .iter()
            .enumerate()
            .map(|(index, type_id)| {
                llvm_type(*type_id, types).map(|ty| format!("{ty} %v{}", index + 1))
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    let cases = implementations
        .iter()
        .map(|(class, _)| format!("    i64 {}, label %class_{}", class.raw(), class.raw()))
        .collect::<Vec<_>>()
        .join("\n");
    let mut blocks = vec![PrivateBlock {
        label: "dispatch".to_owned(),
        instructions: vec![format!(
            "%dispatch_tag = call i64 @{}(i64 %v0, i64 1)",
            RuntimeOperation::FieldGet.abi_symbol()
        )],
        terminator: format!("switch i64 %dispatch_tag, label %invalid_dispatch [\n{cases}\n  ]"),
    }];
    let arguments = std::iter::once("i64 %v0".to_owned())
        .chain(
            method
                .parameters()
                .iter()
                .enumerate()
                .map(|(index, type_id)| {
                    llvm_type(*type_id, types).map(|ty| format!("{ty} %v{}", index + 1))
                })
                .collect::<Result<Vec<_>, _>>()?,
        )
        .collect::<Vec<_>>()
        .join(", ");
    for (class, class_method) in implementations {
        let dispatch_result = format!("%dispatch_result_{}", class.raw());
        let (instructions, terminator) = if method.results().is_empty() {
            (
                vec![format!(
                    "call void @{}({arguments})",
                    method_name(bubble, *class_method)
                )],
                "ret void".to_owned(),
            )
        } else {
            (
                vec![format!(
                    "{dispatch_result} = call {result_type} @{}({arguments})",
                    method_name(bubble, *class_method)
                )],
                format!("ret {result_type} {dispatch_result}"),
            )
        };
        blocks.push(PrivateBlock {
            label: format!("class_{}", class.raw()),
            instructions,
            terminator,
        });
    }
    blocks.push(PrivateBlock {
        label: "invalid_dispatch".to_owned(),
        instructions: Vec::new(),
        terminator: format!(
            "call void @{}()\n  unreachable",
            RuntimeOperation::Trap.abi_symbol()
        ),
    });
    Ok(PrivateFunction {
        name: interface_name(bubble, interface, method.method()),
        parameters,
        result: result_type,
        blocks,
        attributes: Vec::new(),
    })
}

fn lower_indirect_dispatchers(
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    let mut function_types = BTreeSet::new();
    for blocks in bubble
        .functions()
        .iter()
        .map(pop_mir::MirFunction::blocks)
        .chain(
            bubble
                .methods()
                .iter()
                .map(|method| method.function().blocks()),
        )
        .chain(
            bubble
                .nested_functions()
                .iter()
                .map(pop_mir::MirNestedFunction::blocks),
        )
    {
        let value_types = collect_block_value_types(blocks);
        for instruction in blocks.iter().flat_map(pop_mir::MirBlock::instructions) {
            if let MirInstructionKind::CallIndirect { callee, .. } = instruction.kind()
                && let Some(type_id) = value_types.get(callee)
            {
                function_types.insert(*type_id);
            }
        }
    }
    function_types
        .into_iter()
        .map(|type_id| lower_indirect_dispatcher(type_id, bubble, types))
        .collect()
}

fn collect_block_value_types(blocks: &[pop_mir::MirBlock]) -> BTreeMap<ValueId, TypeId> {
    blocks
        .iter()
        .flat_map(|block| {
            block
                .arguments()
                .iter()
                .map(|argument| (argument.value(), argument.type_id()))
                .chain(block.instructions().iter().filter_map(|instruction| {
                    instruction
                        .optional_result_type()
                        .map(|type_id| (instruction.result(), type_id))
                }))
        })
        .collect()
}

#[allow(clippy::too_many_lines)]
fn lower_indirect_dispatcher(
    function_type: TypeId,
    bubble: &MirBubble,
    types: &TypeArena,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let Some(SemanticType::Function {
        parameters: parameter_types,
        results: result_types,
        ..
    }) = types.get(function_type)
    else {
        return Err(LlvmLoweringError::InvalidType(function_type));
    };
    let result_type = llvm_results(result_types, types)?;
    let mut parameters = vec!["i64 %v0".to_owned()];
    let typed_arguments = parameter_types
        .iter()
        .enumerate()
        .map(|(index, type_id)| {
            llvm_type(*type_id, types).map(|ty| format!("{ty} %v{}", index + 1))
        })
        .collect::<Result<Vec<_>, _>>()?;
    parameters.extend(typed_arguments.clone());
    let argument_text = typed_arguments.join(", ");
    let direct = bubble
        .functions()
        .iter()
        .filter(|function| {
            function.parameters() == parameter_types && function.results() == result_types
        })
        .collect::<Vec<_>>();
    let nested = bubble
        .nested_functions()
        .iter()
        .filter(|function| {
            function.parameters() == parameter_types && function.results() == result_types
        })
        .collect::<Vec<_>>();
    let direct_cases = direct
        .iter()
        .map(|function| {
            format!(
                "    i64 {}, label %direct_s{}",
                function.symbol().raw(),
                function.symbol().raw()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let nested_cases = nested
        .iter()
        .map(|function| {
            format!(
                "    i64 {}, label %nested_{}_{}",
                nested_function_tag(function.owner(), function.function()),
                function.owner().raw(),
                function.function().raw()
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let mut blocks = vec![
        PrivateBlock {
            label: "dispatch".to_owned(),
            instructions: vec![
                "%direct_bits = and i64 %v0, 9223372036854775808".to_owned(),
                "%is_direct = icmp ne i64 %direct_bits, 0".to_owned(),
            ],
            terminator: "br i1 %is_direct, label %direct, label %closure".to_owned(),
        },
        PrivateBlock {
            label: "direct".to_owned(),
            instructions: vec!["%direct_symbol = and i64 %v0, 9223372036854775807".to_owned()],
            terminator: format!(
                "switch i64 %direct_symbol, label %invalid_indirect [\n{direct_cases}\n  ]"
            ),
        },
        PrivateBlock {
            label: "closure".to_owned(),
            instructions: vec![format!(
                "%closure_tag = call i64 @{}(i64 %v0, i64 1)",
                RuntimeOperation::FieldGet.abi_symbol()
            )],
            terminator: format!(
                "switch i64 %closure_tag, label %invalid_indirect [\n{nested_cases}\n  ]"
            ),
        },
    ];
    for function in direct {
        blocks.push(indirect_call_target(
            format!("direct_s{}", function.symbol().raw()),
            &format!("@{}", function_name(bubble.bubble(), function.symbol())),
            &argument_text,
            &result_type,
            result_types.is_empty(),
        ));
    }
    for function in nested {
        let arguments = if argument_text.is_empty() {
            "i64 %v0".to_owned()
        } else {
            format!("i64 %v0, {argument_text}")
        };
        blocks.push(indirect_call_target(
            format!(
                "nested_{}_{}",
                function.owner().raw(),
                function.function().raw()
            ),
            &format!(
                "@{}",
                nested_name(bubble.bubble(), function.owner(), function.function())
            ),
            &arguments,
            &result_type,
            result_types.is_empty(),
        ));
    }
    blocks.push(PrivateBlock {
        label: "invalid_indirect".to_owned(),
        instructions: Vec::new(),
        terminator: format!(
            "call void @{}()\n  unreachable",
            RuntimeOperation::Trap.abi_symbol()
        ),
    });
    Ok(PrivateFunction {
        name: indirect_name(bubble.bubble(), function_type),
        parameters,
        result: result_type,
        blocks,
        attributes: Vec::new(),
    })
}

fn indirect_call_target(
    label: String,
    callee: &str,
    arguments: &str,
    result_type: &str,
    returns_void: bool,
) -> PrivateBlock {
    if returns_void {
        return PrivateBlock {
            label,
            instructions: vec![format!("call void {callee}({arguments})")],
            terminator: "ret void".to_owned(),
        };
    }
    let result = format!("%indirect_result_{label}");
    PrivateBlock {
        label,
        instructions: vec![format!(
            "{result} = call {result_type} {callee}({arguments})"
        )],
        terminator: format!("ret {result_type} {result}"),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PrivateModule {
    globals: Vec<String>,
    declarations: Vec<String>,
    entry_point: Option<String>,
    functions: Vec<PrivateFunction>,
    functions_internal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PrivateFunction {
    name: String,
    parameters: Vec<String>,
    result: String,
    blocks: Vec<PrivateBlock>,
    attributes: Vec<&'static str>,
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
            function.render(formatter, self.functions_internal)?;
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
    let string_type = types
        .source_type("String")
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let takes_arguments = function.parameters().len() == 1
        && function.parameters().first().is_some_and(|parameter| {
            matches!(types.get(*parameter), Some(SemanticType::Array(element)) if *element == string_type)
        });
    let returns_status = function.results() == [int_type];
    if !(function.parameters().is_empty() || takes_arguments)
        || !(function.results().is_empty() || returns_status)
    {
        return Err(LlvmLoweringError::UnsupportedEntryPointSignature(symbol));
    }
    let entry = function_name(bubble.bubble(), symbol);
    if takes_arguments {
        let invocation = if returns_status {
            format!(
                "  %pop_exit_value = call i64 @{entry}(i64 %pop_arguments)\n  %pop_exit_code = trunc i64 %pop_exit_value to i32\n  ret i32 %pop_exit_code"
            )
        } else {
            format!("  call void @{entry}(i64 %pop_arguments)\n  ret i32 0")
        };
        return Ok(format!(
            "define i32 @main(i32 %pop_argc, ptr %pop_argv) {{\nentry:\n  %pop_arguments = call i64 @pop_rt_process_arguments(i32 %pop_argc, ptr %pop_argv)\n  %pop_arguments_valid = icmp ne i64 %pop_arguments, 0\n  br i1 %pop_arguments_valid, label %invoke, label %trap\ntrap:\n  call void @pop_rt_trap()\n  unreachable\ninvoke:\n{invocation}\n}}"
        ));
    }
    let invocation = if returns_status {
        format!(
            "  %pop_exit_value = call i64 @{entry}()\n  %pop_exit_code = trunc i64 %pop_exit_value to i32\n  ret i32 %pop_exit_code"
        )
    } else {
        format!("  call void @{entry}()\n  ret i32 0")
    };
    Ok(format!(
        "define i32 @main(i32 %pop_argc, ptr %pop_argv) {{\nentry:\n{invocation}\n}}"
    ))
}

fn function_name(bubble: BubbleId, symbol: SymbolId) -> String {
    format!("pop_b{}_s{}", bubble.raw(), symbol.raw())
}

fn method_name(bubble: BubbleId, method: pop_foundation::MethodId) -> String {
    format!("pop_b{}_method_{}", bubble.raw(), method.raw())
}

fn interface_name(
    bubble: BubbleId,
    interface: pop_foundation::InterfaceId,
    method: pop_foundation::InterfaceMethodId,
) -> String {
    format!(
        "pop_b{}_interface_{}_{}",
        bubble.raw(),
        interface.raw(),
        method.raw()
    )
}

fn nested_name(
    bubble: BubbleId,
    owner: SymbolId,
    function: pop_foundation::NestedFunctionId,
) -> String {
    format!(
        "pop_b{}_nested_{}_{}",
        bubble.raw(),
        owner.raw(),
        function.raw()
    )
}

fn indirect_name(bubble: BubbleId, function_type: TypeId) -> String {
    format!("pop_b{}_indirect_t{}", bubble.raw(), function_type.raw())
}

impl PrivateFunction {
    fn render(&self, formatter: &mut fmt::Formatter<'_>, internal: bool) -> fmt::Result {
        let linkage = if internal { "internal " } else { "" };
        let attributes = if self.attributes.is_empty() {
            String::new()
        } else {
            format!(" {}", self.attributes.join(" "))
        };
        writeln!(
            formatter,
            "define {linkage}{} @{}({}){attributes} {{",
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

#[allow(clippy::too_many_arguments)]
fn lower_function(
    bubble: BubbleId,
    function: &pop_mir::MirFunction,
    types: &TypeArena,
    options: LlvmLoweringOptions,
    memory_none: bool,
    field_layout: &BTreeMap<FieldId, u32>,
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    string_literals: &BTreeMap<String, String>,
) -> Result<PrivateFunction, LlvmLoweringError> {
    lower_function_parts(
        bubble,
        function_name(bubble, function.symbol()),
        function.parameters(),
        function.results(),
        function.effects(),
        memory_none,
        function.blocks(),
        None,
        types,
        options,
        field_layout,
        record_fields,
        record_field_types,
        string_literals,
    )
}

#[derive(Clone, Copy, Debug)]
struct DirectScalarArray {
    length: ValueId,
    initial_value: ValueId,
    element_type: TypeId,
}

#[derive(Debug, Default)]
struct DirectScalarArrays {
    allocations: BTreeMap<ValueId, DirectScalarArray>,
    aliases: BTreeMap<ValueId, ValueId>,
}

impl DirectScalarArrays {
    #[allow(clippy::too_many_lines)]
    fn analyze(
        blocks: &[pop_mir::MirBlock],
        value_types: &BTreeMap<ValueId, TypeId>,
        types: &TypeArena,
    ) -> Self {
        let Some(entry) = blocks.first() else {
            return Self::default();
        };
        let mut allocations = BTreeMap::new();
        let mut aliases = BTreeMap::new();
        for instruction in entry.instructions() {
            let MirInstructionKind::ArrayCreate {
                length,
                initial_value,
                element_map: ArrayElementMap::Scalar,
            } = instruction.kind()
            else {
                continue;
            };
            let Some(SemanticType::Array(element_type)) = value_types
                .get(&instruction.result())
                .and_then(|type_id| types.get(*type_id))
            else {
                continue;
            };
            if !is_direct_scalar_element(*element_type, types) {
                continue;
            }
            allocations.insert(
                instruction.result(),
                DirectScalarArray {
                    length: *length,
                    initial_value: *initial_value,
                    element_type: *element_type,
                },
            );
            aliases.insert(instruction.result(), instruction.result());
        }
        if allocations.is_empty() {
            return Self::default();
        }

        let mut incoming: BTreeMap<BlockId, Vec<Vec<ValueId>>> = BTreeMap::new();
        for block in blocks {
            if let MirTerminator::Branch { target, arguments } = block.terminator() {
                incoming.entry(*target).or_default().push(arguments.clone());
            }
        }
        loop {
            let mut changed = false;
            for block in blocks {
                let Some(edges) = incoming.get(&block.block()) else {
                    continue;
                };
                for (index, argument) in block.arguments().iter().enumerate() {
                    let origins = edges
                        .iter()
                        .filter_map(|values| values.get(index))
                        .filter_map(|value| aliases.get(value).copied())
                        .collect::<BTreeSet<_>>();
                    let Some(origin) = origins.first().copied() else {
                        continue;
                    };
                    if origins.len() == 1 && !aliases.contains_key(&argument.value()) {
                        aliases.insert(argument.value(), origin);
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }

        let mut rejected = BTreeSet::new();
        for block in blocks {
            for instruction in block.instructions() {
                let used = instruction
                    .operands()
                    .into_iter()
                    .filter_map(|value| aliases.get(&value).copied())
                    .collect::<BTreeSet<_>>();
                for origin in used {
                    let allowed_array = match instruction.kind() {
                        MirInstructionKind::ArrayLength { array }
                        | MirInstructionKind::ArrayGetChecked { array, .. }
                        | MirInstructionKind::ArraySet { array, .. }
                        | MirInstructionKind::ArrayFill { array, .. } => {
                            aliases.get(array).copied() == Some(origin)
                        }
                        _ => false,
                    };
                    let has_non_array_use = instruction.operands().into_iter().any(|value| {
                        aliases.get(&value).copied() == Some(origin)
                            && !matches!(
                                instruction.kind(),
                                MirInstructionKind::ArrayLength { array }
                                    | MirInstructionKind::ArrayGetChecked { array, .. }
                                    | MirInstructionKind::ArraySet { array, .. }
                                    | MirInstructionKind::ArrayFill { array, .. }
                                    if *array == value
                            )
                    });
                    if !allowed_array || has_non_array_use {
                        rejected.insert(origin);
                    }
                }
            }
            match block.terminator() {
                MirTerminator::Branch { target, arguments } => {
                    let target_arguments = blocks
                        .iter()
                        .find(|candidate| candidate.block() == *target)
                        .map(pop_mir::MirBlock::arguments)
                        .unwrap_or_default();
                    for (index, value) in arguments.iter().enumerate() {
                        let Some(origin) = aliases.get(value).copied() else {
                            continue;
                        };
                        let target_origin = target_arguments
                            .get(index)
                            .and_then(|argument| aliases.get(&argument.value()))
                            .copied();
                        if target_origin != Some(origin) {
                            rejected.insert(origin);
                            rejected.extend(target_origin);
                        }
                    }
                }
                MirTerminator::Return { values } => {
                    rejected.extend(
                        values
                            .iter()
                            .filter_map(|value| aliases.get(value).copied()),
                    );
                }
                MirTerminator::ConditionalBranch { condition, .. } => {
                    if let Some(origin) = aliases.get(condition) {
                        rejected.insert(*origin);
                    }
                }
                MirTerminator::UnionSwitch { scrutinee, .. } => {
                    if let Some(origin) = aliases.get(scrutinee) {
                        rejected.insert(*origin);
                    }
                }
                MirTerminator::Missing
                | MirTerminator::Trap(_)
                | MirTerminator::Panic(_)
                | MirTerminator::ContinueUnwind(_)
                | MirTerminator::Unreachable => {}
            }
        }
        allocations.retain(|origin, _| !rejected.contains(origin));
        aliases.retain(|_, origin| allocations.contains_key(origin));
        Self {
            allocations,
            aliases,
        }
    }

    fn origin(&self, value: ValueId) -> Option<ValueId> {
        self.aliases.get(&value).copied()
    }

    fn allocation(&self, value: ValueId) -> Option<(ValueId, DirectScalarArray)> {
        let origin = self.origin(value)?;
        self.allocations
            .get(&origin)
            .copied()
            .map(|allocation| (origin, allocation))
    }
}

fn is_direct_scalar_element(type_id: TypeId, types: &TypeArena) -> bool {
    matches!(
        types.get(type_id),
        Some(SemanticType::Primitive(
            PrimitiveType::Boolean
                | PrimitiveType::Integer(_)
                | PrimitiveType::Float32
                | PrimitiveType::Float64
        ))
    )
}

#[allow(clippy::too_many_arguments)]
#[allow(clippy::too_many_lines)]
fn lower_function_parts(
    bubble: BubbleId,
    name: String,
    parameter_types: &[TypeId],
    result_types: &[TypeId],
    effects: MirEffectSummary,
    memory_none: bool,
    function_blocks: &[pop_mir::MirBlock],
    environment: Option<(&str, &BTreeSet<u32>)>,
    types: &TypeArena,
    options: LlvmLoweringOptions,
    field_layout: &BTreeMap<FieldId, u32>,
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    string_literals: &BTreeMap<String, String>,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let proven_non_overflow_adds = proven_counted_reduction_adds(function_blocks);
    let has_gc_safe_point = function_blocks
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .any(|instruction| matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. }));
    let mut value_types = BTreeMap::new();
    for block in function_blocks {
        for argument in block.arguments() {
            value_types.insert(argument.value(), argument.type_id());
        }
        for instruction in block.instructions() {
            if let Some(type_id) = instruction.optional_result_type() {
                value_types.insert(instruction.result(), type_id);
            }
        }
    }
    let direct_scalar_arrays = DirectScalarArrays::analyze(function_blocks, &value_types, types);
    let mut incoming_edges: BTreeMap<BlockId, Vec<(String, Vec<ValueId>)>> = BTreeMap::new();
    let mut union_payload_sources = BTreeMap::new();
    let mut has_union_switch = false;
    for predecessor in function_blocks {
        match predecessor.terminator() {
            MirTerminator::Branch { target, arguments } => {
                incoming_edges.entry(*target).or_default().push((
                    llvm_block_exit_label(
                        predecessor,
                        &proven_non_overflow_adds,
                        &direct_scalar_arrays,
                    ),
                    arguments.clone(),
                ));
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
    for (block_index, block) in function_blocks.iter().enumerate() {
        let mut instructions = lower_block_arguments(
            block,
            incoming_edges.get(&block.block()).map(Vec::as_slice),
            union_payload_sources.get(&block.block()).copied(),
            types,
        )?;
        if block_index == 0 {
            let mut initialization = initialize_gc_poll(has_gc_safe_point);
            initialization.extend(initialize_array_outputs(
                function_blocks,
                &direct_scalar_arrays,
            ));
            instructions.splice(0..0, initialization);
        }
        for instruction in block.instructions() {
            if options.emit_comments {
                instructions.push(format!("; mir v{}", instruction.result().raw()));
            }
            instructions.push(lower_instruction(
                bubble,
                instruction,
                &value_types,
                types,
                field_layout,
                record_fields,
                record_field_types,
                string_literals,
                environment,
                &proven_non_overflow_adds,
                &direct_scalar_arrays,
            )?);
        }
        blocks.push(PrivateBlock {
            label: format!("b{}", block.block().raw()),
            instructions,
            terminator: lower_terminator(
                block.terminator(),
                &value_types,
                types,
                &direct_scalar_arrays,
            )?,
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
    let mut parameters = environment
        .map(|(name, _)| vec![format!("i64 {name}")])
        .unwrap_or_default();
    parameters.extend(
        parameter_types
            .iter()
            .enumerate()
            .map(|(index, type_id)| llvm_type(*type_id, types).map(|ty| format!("{ty} %v{index}")))
            .collect::<Result<Vec<_>, LlvmLoweringError>>()?,
    );
    Ok(PrivateFunction {
        name,
        parameters,
        result: llvm_results(result_types, types)?,
        blocks,
        attributes: llvm_function_attributes(effects, memory_none),
    })
}

fn proven_counted_reduction_adds(blocks: &[pop_mir::MirBlock]) -> BTreeSet<ValueId> {
    let constants = blocks
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::IntegerConstant(value) if value.kind() == IntegerKind::Int64 => {
                value
                    .signed()
                    .map(|value| (instruction.result(), i128::from(value)))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    blocks
        .iter()
        .find_map(|body| prove_counted_reduction(body, blocks, &constants))
        .map_or_else(BTreeSet::new, BTreeSet::from)
}

fn prove_counted_reduction(
    body: &pop_mir::MirBlock,
    blocks: &[pop_mir::MirBlock],
    constants: &BTreeMap<ValueId, i128>,
) -> Option<[ValueId; 2]> {
    let [induction, accumulator, ..] = body.arguments() else {
        return None;
    };
    let (next_induction, step_value) = body.instructions().iter().find_map(|instruction| {
        let MirInstructionKind::CheckedIntegerAdd {
            kind: IntegerKind::Int64,
            left,
            right,
        } = instruction.kind()
        else {
            return None;
        };
        if *left == induction.value() && constants.contains_key(right) {
            Some((instruction.result(), *right))
        } else if *right == induction.value() && constants.contains_key(left) {
            Some((instruction.result(), *left))
        } else {
            None
        }
    })?;
    let next_accumulator = body.instructions().iter().find_map(|instruction| {
        let MirInstructionKind::CheckedIntegerAdd {
            kind: IntegerKind::Int64,
            left,
            right,
        } = instruction.kind()
        else {
            return None;
        };
        ((*left == accumulator.value() && *right == induction.value())
            || (*right == accumulator.value() && *left == induction.value()))
        .then_some(instruction.result())
    })?;
    let step = *constants.get(&step_value)?;
    let entry = blocks.first()?;
    let MirTerminator::Branch { target, arguments } = entry.terminator() else {
        return None;
    };
    if *target != body.block() || arguments.len() != 2 {
        return None;
    }
    let initial_induction = *constants.get(&arguments[0])?;
    let initial_accumulator = *constants.get(&arguments[1])?;
    let MirTerminator::Branch {
        target: condition_block,
        arguments,
    } = body.terminator()
    else {
        return None;
    };
    if !arguments.is_empty() {
        return None;
    }
    let condition_block = blocks
        .iter()
        .find(|block| block.block() == *condition_block)?;
    let (comparison, limit) = counted_loop_limit(condition_block, next_induction, constants)?;
    let MirTerminator::ConditionalBranch {
        condition,
        when_true,
        when_false,
    } = condition_block.terminator()
    else {
        return None;
    };
    if *condition != comparison || can_reach_block(blocks, *when_true, body.block()) {
        return None;
    }
    let backedge = blocks.iter().find(|block| block.block() == *when_false)?;
    if !backedge
        .instructions()
        .iter()
        .any(|instruction| matches!(instruction.kind(), MirInstructionKind::GcSafePoint { .. }))
        || !matches!(
            backedge.terminator(),
            MirTerminator::Branch { target, arguments }
                if *target == body.block()
                    && arguments == &[next_induction, next_accumulator]
        )
    {
        return None;
    }
    prove_reduction_range(initial_induction, initial_accumulator, step, limit)
        .then_some([next_induction, next_accumulator])
}

fn can_reach_block(blocks: &[pop_mir::MirBlock], start: BlockId, target: BlockId) -> bool {
    let mut pending = vec![start];
    let mut visited = BTreeSet::new();
    while let Some(block_id) = pending.pop() {
        if block_id == target {
            return true;
        }
        if !visited.insert(block_id) {
            continue;
        }
        let Some(block) = blocks.iter().find(|block| block.block() == block_id) else {
            continue;
        };
        match block.terminator() {
            MirTerminator::Branch { target, .. } => pending.push(*target),
            MirTerminator::ConditionalBranch {
                when_true,
                when_false,
                ..
            } => pending.extend([*when_true, *when_false]),
            MirTerminator::UnionSwitch { arms, .. } => {
                pending.extend(arms.iter().map(|arm| arm.target()));
            }
            MirTerminator::Missing
            | MirTerminator::Return { .. }
            | MirTerminator::Trap(_)
            | MirTerminator::Panic(_)
            | MirTerminator::ContinueUnwind(_)
            | MirTerminator::Unreachable => {}
        }
    }
    false
}

fn counted_loop_limit(
    condition_block: &pop_mir::MirBlock,
    next_induction: ValueId,
    constants: &BTreeMap<ValueId, i128>,
) -> Option<(ValueId, i128)> {
    condition_block
        .instructions()
        .iter()
        .find_map(|instruction| match instruction.kind() {
            MirInstructionKind::CompareEqual { left, right } if *left == next_induction => {
                constants
                    .get(right)
                    .copied()
                    .map(|limit| (instruction.result(), limit))
            }
            MirInstructionKind::CompareEqual { left, right } if *right == next_induction => {
                constants
                    .get(left)
                    .copied()
                    .map(|limit| (instruction.result(), limit))
            }
            _ => None,
        })
}

fn prove_reduction_range(
    initial_induction: i128,
    initial_accumulator: i128,
    step: i128,
    limit: i128,
) -> bool {
    let Some(distance) = limit.checked_sub(initial_induction) else {
        return false;
    };
    if initial_induction < 0
        || initial_accumulator < 0
        || step <= 0
        || distance <= 0
        || distance % step != 0
    {
        return false;
    }
    let iterations = distance / step;
    let final_accumulator = (|| {
        let last_offset = (iterations - 1).checked_mul(step)?;
        let series_factor = initial_induction.checked_mul(2)?.checked_add(last_offset)?;
        let series = iterations.checked_mul(series_factor)? / 2;
        initial_accumulator.checked_add(series)
    })();
    limit <= i128::from(i64::MAX)
        && final_accumulator.is_some_and(|value| value <= i128::from(i64::MAX))
}

fn llvm_function_attributes(effects: MirEffectSummary, memory_none: bool) -> Vec<&'static str> {
    let mut attributes = Vec::new();
    if memory_none {
        attributes.push("memory(none)");
    }
    if !effects.contains(MirEffect::MayUnwind) {
        attributes.push("nounwind");
    }
    attributes
}

fn llvm_memory_none_instruction(instruction: &MirInstructionKind) -> bool {
    matches!(
        instruction,
        MirInstructionKind::IntegerConstant(_)
            | MirInstructionKind::FloatConstant(_)
            | MirInstructionKind::BooleanConstant(_)
            | MirInstructionKind::NilConstant
            | MirInstructionKind::FunctionReference(_)
            | MirInstructionKind::CheckedIntegerAdd { .. }
            | MirInstructionKind::CheckedIntegerSubtract { .. }
            | MirInstructionKind::CheckedIntegerMultiply { .. }
            | MirInstructionKind::CheckedIntegerDivide { .. }
            | MirInstructionKind::CheckedIntegerRemainder { .. }
            | MirInstructionKind::IntegerNegate { .. }
            | MirInstructionKind::FloatAdd { .. }
            | MirInstructionKind::FloatSubtract { .. }
            | MirInstructionKind::FloatMultiply { .. }
            | MirInstructionKind::FloatDivide { .. }
            | MirInstructionKind::FloatNegate { .. }
            | MirInstructionKind::CompareIntegerLess { .. }
            | MirInstructionKind::CompareIntegerGreater { .. }
            | MirInstructionKind::CompareFloatLess { .. }
            | MirInstructionKind::CompareFloatGreater { .. }
            | MirInstructionKind::BooleanNot { .. }
            | MirInstructionKind::BooleanAnd { .. }
            | MirInstructionKind::BooleanOr { .. }
    )
}

fn initialize_gc_poll(has_gc_safe_point: bool) -> Vec<String> {
    if !has_gc_safe_point {
        return Vec::new();
    }
    vec![
        format!("{GC_POLL_BUDGET} = alloca i32, align 4"),
        format!("store i32 {GC_POLL_INTERVAL}, ptr {GC_POLL_BUDGET}, align 4"),
    ]
}

fn initialize_array_outputs(
    blocks: &[pop_mir::MirBlock],
    direct_scalar_arrays: &DirectScalarArrays,
) -> Vec<String> {
    blocks
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .filter(|instruction| {
            matches!(
                instruction.kind(),
                MirInstructionKind::ArrayLength { .. } | MirInstructionKind::ArrayGetChecked { .. }
            ) && match instruction.kind() {
                MirInstructionKind::ArrayLength { array }
                | MirInstructionKind::ArrayGetChecked { array, .. } => {
                    direct_scalar_arrays.origin(*array).is_none()
                }
                _ => true,
            }
        })
        .map(|instruction| format!("%v{}_output = alloca i64", instruction.result().raw()))
        .collect()
}

fn llvm_block_exit_label(
    block: &pop_mir::MirBlock,
    proven_non_overflow_adds: &BTreeSet<ValueId>,
    direct_scalar_arrays: &DirectScalarArrays,
) -> String {
    block
        .instructions()
        .iter()
        .rev()
        .find_map(|instruction| {
            let suffix = match instruction.kind() {
                MirInstructionKind::CheckedIntegerAdd { .. }
                    if proven_non_overflow_adds.contains(&instruction.result()) =>
                {
                    return None;
                }
                MirInstructionKind::CheckedIntegerAdd { .. }
                | MirInstructionKind::CheckedIntegerSubtract { .. }
                | MirInstructionKind::CheckedIntegerMultiply { .. }
                | MirInstructionKind::CheckedIntegerDivide { .. }
                | MirInstructionKind::CheckedIntegerRemainder { .. }
                | MirInstructionKind::IntegerNegate { .. }
                | MirInstructionKind::ArraySet { .. }
                | MirInstructionKind::ArrayFill { .. } => "continue",
                MirInstructionKind::GcSafePoint { .. } => "poll_continue",
                MirInstructionKind::ArrayCreate { .. } => "create",
                MirInstructionKind::ArrayLength { array }
                | MirInstructionKind::ArrayGetChecked { array, .. } => {
                    let _ = direct_scalar_arrays.origin(*array);
                    "load"
                }
                _ => return None,
            };
            Some(format!("v{}_{suffix}", instruction.result().raw()))
        })
        .unwrap_or_else(|| format!("b{}", block.block().raw()))
}

fn lower_block_arguments(
    block: &pop_mir::MirBlock,
    incoming: Option<&[(String, Vec<ValueId>)]>,
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
                        Ok(format!("[ %v{}, %{predecessor} ]", value.raw()))
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
#[allow(clippy::too_many_arguments)]
fn lower_instruction(
    bubble: BubbleId,
    instruction: &pop_mir::MirInstruction,
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    string_literals: &BTreeMap<String, String>,
    environment: Option<(&str, &BTreeSet<u32>)>,
    proven_non_overflow_adds: &BTreeSet<ValueId>,
    direct_scalar_arrays: &DirectScalarArrays,
) -> Result<String, LlvmLoweringError> {
    let result = format!("%v{}", instruction.result().raw());
    let result_type = instruction.optional_result_type();
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
            if proven_non_overflow_adds.contains(&instruction.result()) {
                format!(
                    "{result} = add nsw i{} %v{}, %v{}",
                    kind.bit_width(),
                    left.raw(),
                    right.raw()
                )
            } else {
                lower_checked_integer_binary(&result, "add", *kind, *left, *right)
            }
        }
        MirInstructionKind::CheckedIntegerSubtract { kind, left, right } => {
            lower_checked_integer_binary(&result, "sub", *kind, *left, *right)
        }
        MirInstructionKind::CheckedIntegerMultiply { kind, left, right } => {
            lower_checked_integer_binary(&result, "mul", *kind, *left, *right)
        }
        MirInstructionKind::CheckedIntegerDivide { kind, left, right } => {
            lower_checked_integer_division(&result, "div", *kind, *left, *right)
        }
        MirInstructionKind::CheckedIntegerRemainder { kind, left, right } => {
            lower_checked_integer_division(&result, "rem", *kind, *left, *right)
        }
        MirInstructionKind::IntegerNegate { kind, operand } => {
            lower_checked_integer_negate(&result, *kind, *operand)
        }
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
        MirInstructionKind::CompareEqual { left, right } => lower_equality(
            &result,
            *left,
            *right,
            false,
            value_types,
            types,
            record_field_types,
        )?,
        MirInstructionKind::CompareNotEqual { left, right } => lower_equality(
            &result,
            *left,
            *right,
            true,
            value_types,
            types,
            record_field_types,
        )?,
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
            format!("{result} = add i64 0, {}", direct_function_tag(*symbol))
        }
        MirInstructionKind::CallDirect {
            function: callee,
            arguments,
            ..
        } => call_line(
            &result,
            result_type,
            &format!("@{}", function_name(bubble, *callee)),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallStandard {
            function,
            arguments,
            ..
        } => {
            if arguments.len() != 1 {
                return Err(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                });
            }
            match function.raw() {
                0 => format!("call void @pop_std_print_int(i64 %v{})", arguments[0].raw()),
                1 => format!(
                    "call void @pop_std_print_string(i64 %v{})",
                    arguments[0].raw()
                ),
                _ => {
                    return Err(LlvmLoweringError::UnsupportedInstruction {
                        function: FunctionId::from_raw(u32::MAX),
                        value: instruction.result(),
                    });
                }
            }
        }
        MirInstructionKind::GcSafePoint {
            safe_point, roots, ..
        } => lower_gc_safe_point(&result, safe_point.raw(), roots, direct_scalar_arrays),
        MirInstructionKind::RetainRoot { value } => format!(
            "{result} = call i64 @{}(i64 %v{})",
            RuntimeOperation::RetainRoot.abi_symbol(),
            value.raw()
        ),
        MirInstructionKind::ReleaseRoot { handle } => format!(
            "call i8 @{}(i64 %v{})",
            RuntimeOperation::ReleaseRoot.abi_symbol(),
            handle.raw()
        ),
        MirInstructionKind::Pin { value } => format!(
            "{result} = call i64 @{}(i64 %v{})",
            RuntimeOperation::Pin.abi_symbol(),
            value.raw()
        ),
        MirInstructionKind::Unpin { handle } => format!(
            "call i8 @{}(i64 %v{})",
            RuntimeOperation::Unpin.abi_symbol(),
            handle.raw()
        ),
        MirInstructionKind::WriteBarrier { owner, .. } => format!(
            "call void @{}(i64 %v{})",
            RuntimeOperation::SatbWriteBarrier.abi_symbol(),
            owner.raw()
        ),
        MirInstructionKind::CaptureCellAllocate { initial, .. } => {
            lower_capture_cell_allocate(&result, *initial, value_types, types)?
        }
        MirInstructionKind::ClosureEnvironmentAllocate {
            owner,
            function,
            captures,
            ..
        } => lower_closure_environment_allocate(
            &result,
            *owner,
            *function,
            captures,
            value_types,
            types,
        )?,
        MirInstructionKind::ArrayMake {
            elements,
            element_map,
        } => lower_array_make(&result, elements, *element_map, value_types, types)?,
        MirInstructionKind::ArrayCreate {
            length,
            initial_value,
            element_map,
        } => {
            if let Some((origin, allocation)) =
                direct_scalar_arrays.allocation(instruction.result())
            {
                debug_assert_eq!(origin, instruction.result());
                lower_direct_array_create(&result, allocation, value_types, types)?
            } else {
                lower_array_create(
                    &result,
                    *length,
                    *initial_value,
                    *element_map,
                    value_types,
                    types,
                )?
            }
        }
        MirInstructionKind::TableMake {
            entries,
            key_map,
            value_map,
        } => lower_table_make(&result, entries, *key_map, *value_map, value_types, types)?,
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
            class,
            fields,
            object_map,
        } => lower_class_make(
            &result,
            *class,
            fields,
            object_map.slot_count() + 1,
            value_types,
            types,
            field_layout,
        )?,
        MirInstructionKind::CallDirectMethod {
            method, arguments, ..
        } => call_line(
            &result,
            result_type,
            &format!("@{}", method_name(bubble, *method)),
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
            &format!("@{}", interface_name(bubble, *interface, *method)),
            arguments,
            value_types,
            types,
        )?,
        MirInstructionKind::CallIndirect {
            callee, arguments, ..
        } => {
            let callee_type = value_types
                .get(callee)
                .copied()
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let arguments = std::iter::once(*callee)
                .chain(arguments.iter().copied())
                .collect::<Vec<_>>();
            call_line(
                &result,
                result_type,
                &format!("@{}", indirect_name(bubble, callee_type)),
                &arguments,
                value_types,
                types,
            )?
        }
        MirInstructionKind::TupleMake(elements) => {
            lower_tuple_make(&result, elements, value_types, types)?
        }
        MirInstructionKind::ArrayGet { array, index } => runtime_call(
            &result,
            result_type,
            RuntimeOperation::ArrayGet,
            &[*array, *index],
            value_types,
            types,
        )?,
        MirInstructionKind::ArrayLength { array } => {
            if let Some((_, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_length(&result, allocation)
            } else {
                lower_array_output_call(
                    &result,
                    instruction.result_type(),
                    RuntimeOperation::ArrayLength,
                    &[*array],
                    value_types,
                    types,
                )?
            }
        }
        MirInstructionKind::ArrayGetChecked { array, index } => {
            if let Some((origin, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_get(
                    &result,
                    origin,
                    allocation,
                    *index,
                    instruction.result_type(),
                    types,
                )?
            } else {
                lower_array_output_call(
                    &result,
                    instruction.result_type(),
                    RuntimeOperation::ArrayGetChecked,
                    &[*array, *index],
                    value_types,
                    types,
                )?
            }
        }
        MirInstructionKind::ArraySet {
            array,
            index,
            value,
            ..
        } => {
            if let Some((origin, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_set(
                    &result,
                    origin,
                    allocation,
                    *index,
                    *value,
                    value_types,
                    types,
                )?
            } else {
                lower_array_set(&result, *array, *index, *value, value_types, types)?
            }
        }
        MirInstructionKind::ArrayFill { array, value, .. } => {
            if let Some((origin, allocation)) = direct_scalar_arrays.allocation(*array) {
                lower_direct_array_fill(&result, origin, allocation, *value, value_types, types)?
            } else {
                lower_array_fill(&result, *array, *value, value_types, types)?
            }
        }
        MirInstructionKind::RecordUpdate {
            record,
            base,
            fields,
        } => lower_record_update(
            &result,
            *record,
            *base,
            fields,
            record_fields,
            record_field_types,
            field_layout,
            value_types,
            types,
        )?,
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
        MirInstructionKind::InterfaceUpcast { value, .. } => {
            format!("{result} = add i64 %v{}, 0", value.raw())
        }
        MirInstructionKind::CaptureCellLoad { cell } => lower_runtime_slot_load_from(
            instruction.result(),
            instruction.result_type(),
            &format!("%v{}", cell.raw()),
            1,
            types,
        )?
        .join("\n"),
        MirInstructionKind::CaptureCellStore { cell, value } => {
            lower_capture_store(&format!("%v{}", cell.raw()), *value, value_types, types)?
        }
        MirInstructionKind::CaptureLoad { slot, mode, .. } => lower_capture_load(
            instruction.result(),
            instruction.result_type(),
            environment
                .ok_or(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                })?
                .0,
            *slot,
            *mode,
            environment.is_some_and(|(_, self_slots)| self_slots.contains(slot)),
            types,
        )?,
        MirInstructionKind::CaptureCellReference { slot, .. } => lower_runtime_slot_load_from(
            instruction.result(),
            instruction.result_type(),
            environment
                .ok_or(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                })?
                .0,
            *slot as usize + 2,
            types,
        )?
        .join("\n"),
        MirInstructionKind::CaptureStore { slot, value, .. } => lower_nested_capture_store(
            environment
                .ok_or(LlvmLoweringError::UnsupportedInstruction {
                    function: FunctionId::from_raw(u32::MAX),
                    value: instruction.result(),
                })?
                .0,
            *slot,
            *value,
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
    direct_scalar_arrays: &DirectScalarArrays,
) -> Result<String, LlvmLoweringError> {
    let lowered = match terminator {
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
    };
    if matches!(terminator, MirTerminator::Return { .. })
        && !direct_scalar_arrays.allocations.is_empty()
    {
        let releases = direct_scalar_arrays
            .allocations
            .keys()
            .map(|origin| {
                format!(
                    "%pop_direct_array_{}_storage = inttoptr i64 %v{} to ptr\n  call void @free(ptr %pop_direct_array_{}_storage)",
                    origin.raw(),
                    origin.raw(),
                    origin.raw()
                )
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        Ok(format!("{releases}\n  {lowered}"))
    } else {
        Ok(lowered)
    }
}

fn lower_checked_integer_binary(
    result: &str,
    operation: &str,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
) -> String {
    let bits = kind.bit_width();
    let signed = if kind.is_signed() { 's' } else { 'u' };
    let pair = format!("{result}_checked");
    let overflow = format!("{result}_overflow");
    format!(
        "{pair} = call {{ i{bits}, i1 }} @llvm.{signed}{operation}.with.overflow.i{bits}(i{bits} %v{}, i{bits} %v{})\n{result} = extractvalue {{ i{bits}, i1 }} {pair}, 0\n{overflow} = extractvalue {{ i{bits}, i1 }} {pair}, 1\n{}",
        left.raw(),
        right.raw(),
        lower_trap_edge(result, &overflow)
    )
}

fn lower_checked_integer_division(
    result: &str,
    operation: &str,
    kind: IntegerKind,
    left: ValueId,
    right: ValueId,
) -> String {
    let bits = kind.bit_width();
    let zero = format!("{result}_zero");
    let mut lines = vec![format!("{zero} = icmp eq i{bits} %v{}, 0", right.raw())];
    let invalid = if kind.is_signed() {
        let minimum = -(1_i128 << (bits - 1));
        let minimum_value = format!("{result}_minimum");
        let negative_one = format!("{result}_negative_one");
        let overflow = format!("{result}_overflow");
        let invalid = format!("{result}_invalid");
        lines.extend([
            format!(
                "{minimum_value} = icmp eq i{bits} %v{}, {minimum}",
                left.raw()
            ),
            format!("{negative_one} = icmp eq i{bits} %v{}, -1", right.raw()),
            format!("{overflow} = and i1 {minimum_value}, {negative_one}"),
            format!("{invalid} = or i1 {zero}, {overflow}"),
        ]);
        invalid
    } else {
        zero
    };
    lines.push(lower_trap_edge(result, &invalid));
    lines.push(format!(
        "{result} = {} i{bits} %v{}, %v{}",
        if kind.is_signed() {
            format!("s{operation}")
        } else {
            format!("u{operation}")
        },
        left.raw(),
        right.raw()
    ));
    lines.join("\n")
}

fn lower_checked_integer_negate(result: &str, kind: IntegerKind, operand: ValueId) -> String {
    let bits = kind.bit_width();
    let signed = if kind.is_signed() { 's' } else { 'u' };
    let pair = format!("{result}_checked");
    let overflow = format!("{result}_overflow");
    format!(
        "{pair} = call {{ i{bits}, i1 }} @llvm.{signed}sub.with.overflow.i{bits}(i{bits} 0, i{bits} %v{})\n{result} = extractvalue {{ i{bits}, i1 }} {pair}, 0\n{overflow} = extractvalue {{ i{bits}, i1 }} {pair}, 1\n{}",
        operand.raw(),
        lower_trap_edge(result, &overflow)
    )
}

fn lower_trap_edge(result: &str, condition: &str) -> String {
    let label = result.trim_start_matches('%');
    let expected = format!("{condition}_expected");
    format!(
        "{expected} = call i1 @llvm.expect.i1(i1 {condition}, i1 false)\nbr i1 {expected}, label %{label}_trap, label %{label}_continue\n{label}_trap:\n  call void @{}()\n  unreachable\n{label}_continue:",
        RuntimeOperation::Trap.abi_symbol()
    )
}

fn lower_equality(
    result: &str,
    left: ValueId,
    right: ValueId,
    negated: bool,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
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
    if matches!(
        types.get(type_id),
        Some(SemanticType::Tuple(_) | SemanticType::Record(_))
    ) {
        return lower_aggregate_equality(
            result,
            left,
            right,
            type_id,
            negated,
            types,
            record_field_types,
        );
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

fn lower_aggregate_equality(
    result: &str,
    left: ValueId,
    right: ValueId,
    type_id: TypeId,
    negated: bool,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
) -> Result<String, LlvmLoweringError> {
    let mut lines = Vec::new();
    let condition = emit_aggregate_equality(
        &mut lines,
        result.trim_start_matches('%'),
        &format!("%v{}", left.raw()),
        &format!("%v{}", right.raw()),
        type_id,
        types,
        record_field_types,
    )?;
    lines.push(if negated {
        format!("{result} = xor i1 {condition}, true")
    } else {
        format!("{result} = xor i1 {condition}, false")
    });
    Ok(lines.join("\n"))
}

fn emit_aggregate_equality(
    lines: &mut Vec<String>,
    prefix: &str,
    left: &str,
    right: &str,
    type_id: TypeId,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
) -> Result<String, LlvmLoweringError> {
    let field_types = match types
        .get(type_id)
        .ok_or(LlvmLoweringError::InvalidType(type_id))?
    {
        SemanticType::Tuple(elements) => elements.clone(),
        SemanticType::Record(_) => record_field_types
            .get(&type_id)
            .cloned()
            .ok_or(LlvmLoweringError::InvalidType(type_id))?,
        _ => return Err(LlvmLoweringError::InvalidType(type_id)),
    };
    let mut conditions = Vec::new();
    for (index, field_type) in field_types.into_iter().enumerate() {
        let left_field = format!("%{prefix}_{index}_left");
        let right_field = format!("%{prefix}_{index}_right");
        lines.extend([
            format!(
                "{left_field} = call i64 @{}(i64 {left}, i64 {})",
                RuntimeOperation::FieldGet.abi_symbol(),
                index + 1
            ),
            format!(
                "{right_field} = call i64 @{}(i64 {right}, i64 {})",
                RuntimeOperation::FieldGet.abi_symbol(),
                index + 1
            ),
        ]);
        conditions.push(emit_stored_value_equality(
            lines,
            &format!("{prefix}_{index}"),
            &left_field,
            &right_field,
            field_type,
            types,
            record_field_types,
        )?);
    }
    if conditions.is_empty() {
        let condition = format!("%{prefix}_empty");
        lines.push(format!("{condition} = xor i1 0, true"));
        return Ok(condition);
    }
    let mut combined = conditions[0].clone();
    for (index, condition) in conditions.into_iter().enumerate().skip(1) {
        let next = format!("%{prefix}_and_{index}");
        lines.push(format!("{next} = and i1 {combined}, {condition}"));
        combined = next;
    }
    Ok(combined)
}

fn emit_stored_value_equality(
    lines: &mut Vec<String>,
    prefix: &str,
    left: &str,
    right: &str,
    type_id: TypeId,
    types: &TypeArena,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
) -> Result<String, LlvmLoweringError> {
    let semantic = types
        .get(type_id)
        .ok_or(LlvmLoweringError::InvalidType(type_id))?;
    if matches!(semantic, SemanticType::Tuple(_) | SemanticType::Record(_)) {
        return emit_aggregate_equality(
            lines,
            prefix,
            left,
            right,
            type_id,
            types,
            record_field_types,
        );
    }
    let condition = format!("%{prefix}_equal");
    match semantic {
        SemanticType::Primitive(PrimitiveType::String) => {
            let raw = format!("%{prefix}_string_equal");
            lines.extend([
                format!("{raw} = call i8 @pop_rt_string_equal(i64 {left}, i64 {right})"),
                format!("{condition} = icmp ne i8 {raw}, 0"),
            ]);
        }
        SemanticType::Primitive(PrimitiveType::Float32) => {
            let left_bits = format!("%{prefix}_left_bits");
            let right_bits = format!("%{prefix}_right_bits");
            let left_float = format!("%{prefix}_left_float");
            let right_float = format!("%{prefix}_right_float");
            lines.extend([
                format!("{left_bits} = trunc i64 {left} to i32"),
                format!("{right_bits} = trunc i64 {right} to i32"),
                format!("{left_float} = bitcast i32 {left_bits} to float"),
                format!("{right_float} = bitcast i32 {right_bits} to float"),
                format!("{condition} = fcmp oeq float {left_float}, {right_float}"),
            ]);
        }
        SemanticType::Primitive(PrimitiveType::Float64) => {
            let left_float = format!("%{prefix}_left_float");
            let right_float = format!("%{prefix}_right_float");
            lines.extend([
                format!("{left_float} = bitcast i64 {left} to double"),
                format!("{right_float} = bitcast i64 {right} to double"),
                format!("{condition} = fcmp oeq double {left_float}, {right_float}"),
            ]);
        }
        _ => lines.push(format!("{condition} = icmp eq i64 {left}, {right}")),
    }
    Ok(condition)
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

fn lower_array_create(
    result: &str,
    length: ValueId,
    initial_value: ValueId,
    element_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let initial_type = *values
        .get(&initial_value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) = lower_runtime_slot_store(
        initial_value,
        initial_type,
        &llvm_type(initial_type, types)?,
    )?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!("{result}_length_valid = icmp sge i64 %v{}, 0", length.raw()),
        format!(
            "{result}_length_expected = call i1 @llvm.expect.i1(i1 {result}_length_valid, i1 true)"
        ),
        format!(
            "br i1 {result}_length_expected, label %{label}_create, label %{label}_length_trap"
        ),
        format!("{label}_length_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_create:"),
        format!(
            "  {result} = call i64 @{}(i64 %v{}, i1 {}, i64 {stored})",
            RuntimeOperation::AllocateArrayFilled.abi_symbol(),
            length.raw(),
            u8::from(element_map == ArrayElementMap::ManagedReference)
        ),
    ]);
    Ok(lines.join("\n"))
}

fn lower_direct_array_create(
    result: &str,
    allocation: DirectScalarArray,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let initial_type = *values
        .get(&allocation.initial_value)
        .ok_or(LlvmLoweringError::InvalidType(allocation.element_type))?;
    if initial_type != allocation.element_type {
        return Err(LlvmLoweringError::InvalidType(allocation.element_type));
    }
    let (mut lines, stored) = lower_runtime_slot_store(
        allocation.initial_value,
        initial_type,
        &llvm_type(initial_type, types)?,
    )?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result}_size_pair = call {{ i64, i1 }} @llvm.umul.with.overflow.i64(i64 %v{}, i64 8)",
            allocation.length.raw()
        ),
        format!("{result}_size = extractvalue {{ i64, i1 }} {result}_size_pair, 0"),
        format!("{result}_size_overflow = extractvalue {{ i64, i1 }} {result}_size_pair, 1"),
        format!(
            "{result}_length_nonnegative = icmp sge i64 %v{}, 0",
            allocation.length.raw()
        ),
        format!("{result}_size_valid = xor i1 {result}_size_overflow, true"),
        format!(
            "{result}_shape_valid = and i1 {result}_length_nonnegative, {result}_size_valid"
        ),
        format!(
            "{result}_shape_expected = call i1 @llvm.expect.i1(i1 {result}_shape_valid, i1 true)"
        ),
        format!(
            "br i1 {result}_shape_expected, label %{label}_allocate, label %{label}_length_trap"
        ),
        format!("{label}_length_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_allocate:"),
        format!("  {result}_storage = call noalias ptr @malloc(i64 {result}_size)"),
        format!(
            "  {result}_empty = icmp eq i64 %v{}, 0",
            allocation.length.raw()
        ),
        format!("  {result}_allocated = icmp ne ptr {result}_storage, null"),
        format!(
            "  {result}_allocation_valid = or i1 {result}_empty, {result}_allocated"
        ),
        format!(
            "  {result}_allocation_expected = call i1 @llvm.expect.i1(i1 {result}_allocation_valid, i1 true)"
        ),
        format!(
            "  br i1 {result}_allocation_expected, label %{label}_initialize, label %{label}_allocation_trap"
        ),
        format!("{label}_allocation_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_initialize:"),
        format!(
            "  call void @pop_llvm_fill_scalar_array(ptr {result}_storage, i64 %v{}, i64 {stored})",
            allocation.length.raw()
        ),
        format!("  br label %{label}_create"),
        format!("{label}_create:"),
        format!("  {result} = ptrtoint ptr {result}_storage to i64"),
    ]);
    Ok(lines.join("\n"))
}

fn lower_direct_array_length(result: &str, allocation: DirectScalarArray) -> String {
    let label = result.trim_start_matches('%');
    format!(
        "br label %{label}_load\n{label}_load:\n  {result} = add i64 %v{}, 0",
        allocation.length.raw()
    )
}

fn lower_direct_array_get(
    result: &str,
    origin: ValueId,
    allocation: DirectScalarArray,
    index: ValueId,
    result_type: TypeId,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let element_type = llvm_type(result_type, types)?;
    let expected_type = llvm_type(allocation.element_type, types)?;
    if element_type != expected_type {
        return Err(LlvmLoweringError::InvalidType(result_type));
    }
    let label = result.trim_start_matches('%');
    let mut lines = vec![
        format!("{result}_zero_index = sub i64 %v{}, 1", index.raw()),
        format!(
            "{result}_in_bounds = icmp ult i64 {result}_zero_index, %v{}",
            allocation.length.raw()
        ),
        format!(
            "{result}_in_bounds_expected = call i1 @llvm.expect.i1(i1 {result}_in_bounds, i1 true)"
        ),
        format!("br i1 {result}_in_bounds_expected, label %{label}_load, label %{label}_trap"),
        format!("{label}_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_load:"),
        format!(
            "  {result}_storage = inttoptr i64 %v{} to ptr",
            origin.raw()
        ),
        format!(
            "  {result}_slot = getelementptr i64, ptr {result}_storage, i64 {result}_zero_index"
        ),
    ];
    lines.extend(lower_array_output_load(
        result,
        result_type,
        &format!("{result}_slot"),
        types,
    )?);
    Ok(lines.join("\n"))
}

#[allow(clippy::too_many_arguments)]
fn lower_direct_array_set(
    result: &str,
    origin: ValueId,
    allocation: DirectScalarArray,
    index: ValueId,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(allocation.element_type))?;
    if value_type != allocation.element_type {
        return Err(LlvmLoweringError::InvalidType(allocation.element_type));
    }
    let (mut conversion, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    conversion.extend([
        format!("{result}_zero_index = sub i64 %v{}, 1", index.raw()),
        format!(
            "{result}_in_bounds = icmp ult i64 {result}_zero_index, %v{}",
            allocation.length.raw()
        ),
        format!(
            "{result}_in_bounds_expected = call i1 @llvm.expect.i1(i1 {result}_in_bounds, i1 true)"
        ),
        format!("br i1 {result}_in_bounds_expected, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!(
            "  {result}_storage = inttoptr i64 %v{} to ptr",
            origin.raw()
        ),
        format!(
            "  {result}_slot = getelementptr i64, ptr {result}_storage, i64 {result}_zero_index"
        ),
        format!("  store i64 {stored}, ptr {result}_slot, align 8"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(conversion.join("\n"))
}

fn lower_direct_array_fill(
    result: &str,
    origin: ValueId,
    allocation: DirectScalarArray,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(allocation.element_type))?;
    if value_type != allocation.element_type {
        return Err(LlvmLoweringError::InvalidType(allocation.element_type));
    }
    let (mut lines, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!("{result}_storage = inttoptr i64 %v{} to ptr", origin.raw()),
        format!(
            "call void @pop_llvm_fill_scalar_array(ptr {result}_storage, i64 %v{}, i64 {stored})",
            allocation.length.raw()
        ),
        format!("br label %{label}_continue"),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

fn lower_array_output_call(
    result: &str,
    result_type: TypeId,
    operation: RuntimeOperation,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let output = format!("{result}_output");
    let success = format!("{result}_success");
    let expected = format!("{result}_success_expected");
    let label = result.trim_start_matches('%');
    let arguments = arguments
        .iter()
        .map(|value| {
            llvm_value_type(values, *value, types).map(|ty| format!("{ty} %v{}", value.raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mut lines = Vec::new();
    lines.extend([
        format!(
            "{success} = call i8 @{}({}, ptr {output})",
            operation.abi_symbol(),
            arguments.join(", ")
        ),
        format!("{success}_condition = icmp ne i8 {success}, 0"),
        format!("{expected} = call i1 @llvm.expect.i1(i1 {success}_condition, i1 true)"),
        format!("br i1 {expected}, label %{label}_load, label %{label}_trap"),
        format!("{label}_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_load:"),
    ]);
    lines.extend(lower_array_output_load(
        result,
        result_type,
        &output,
        types,
    )?);
    Ok(lines.join("\n"))
}

fn lower_array_output_load(
    result: &str,
    result_type: TypeId,
    output: &str,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    let ty = llvm_type(result_type, types)?;
    let loaded = format!("{result}_slot");
    Ok(match ty.as_str() {
        "i64" => vec![format!("  {result} = load i64, ptr {output}")],
        "i1" | "i8" | "i16" | "i32" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {result} = trunc i64 {loaded} to {ty}"),
        ],
        "float" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {loaded}_bits = trunc i64 {loaded} to i32"),
            format!("  {result} = bitcast i32 {loaded}_bits to float"),
        ],
        "double" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {result} = bitcast i64 {loaded} to double"),
        ],
        "ptr" => vec![
            format!("  {loaded} = load i64, ptr {output}"),
            format!("  {result} = inttoptr i64 {loaded} to ptr"),
        ],
        _ => return Err(LlvmLoweringError::InvalidType(result_type)),
    })
}

fn lower_array_fill(
    result: &str,
    array: ValueId,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result}_filled = call i8 @{}(i64 %v{}, i64 {stored})",
            RuntimeOperation::ArrayFill.abi_symbol(),
            array.raw()
        ),
        format!("{result}_success = icmp ne i8 {result}_filled, 0"),
        format!("{result}_expected = call i1 @llvm.expect.i1(i1 {result}_success, i1 true)"),
        format!("br i1 {result}_expected, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

fn lower_array_set(
    result: &str,
    array: ValueId,
    index: ValueId,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let value_type = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) =
        lower_runtime_slot_store(value, value_type, &llvm_type(value_type, types)?)?;
    let label = result.trim_start_matches('%');
    lines.extend([
        format!(
            "{result}_stored = call i8 @{}(i64 %v{}, i64 %v{}, i64 {stored})",
            RuntimeOperation::ArraySet.abi_symbol(),
            array.raw(),
            index.raw()
        ),
        format!("{result}_in_bounds = icmp ne i8 {result}_stored, 0"),
        format!(
            "{result}_in_bounds_expected = call i1 @llvm.expect.i1(i1 {result}_in_bounds, i1 true)"
        ),
        format!("br i1 {result}_in_bounds_expected, label %{label}_continue, label %{label}_trap"),
        format!("{label}_trap:"),
        format!("  call void @{}()", RuntimeOperation::Trap.abi_symbol()),
        "  unreachable".to_owned(),
        format!("{label}_continue:"),
        format!("  {result} = add i64 0, 0"),
    ]);
    Ok(lines.join("\n"))
}

fn lower_object_make(
    result: &str,
    fields: &[(FieldId, ValueId)],
    slot_count: u32,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    let reference_slots = fields
        .iter()
        .filter_map(|(field, value)| {
            values
                .get(value)
                .copied()
                .filter(|type_id| is_managed_type(*type_id, types))
                .and_then(|_| field_layout.get(field).copied())
                .map(|slot| slot - 1)
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(result, slot_count, &reference_slots);
    for (field, value) in fields {
        let slot = field_layout
            .get(field)
            .ok_or(LlvmLoweringError::InvalidFieldLayout(*field))?;
        let type_id = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (conversions, stored) =
            lower_runtime_slot_store(*value, type_id, &llvm_type(type_id, types)?)?;
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            RuntimeOperation::FieldSet.abi_symbol(),
            slot
        ));
    }
    Ok(lines.join("\n"))
}

fn lower_mapped_allocation(result: &str, slot_count: u32, reference_slots: &[u32]) -> Vec<String> {
    if reference_slots.is_empty() {
        return vec![format!(
            "{result} = call i64 @pop_rt_allocate_mapped_object(i64 {slot_count}, ptr null, i64 0)"
        )];
    }
    let map = format!("{result}_object_map");
    let mut lines = vec![format!("{map} = alloca [{} x i32]", reference_slots.len())];
    for (index, slot) in reference_slots.iter().enumerate() {
        let entry = format!("{map}_{index}");
        lines.extend([
            format!(
                "{entry} = getelementptr [{} x i32], ptr {map}, i64 0, i64 {index}",
                reference_slots.len()
            ),
            format!("store i32 {slot}, ptr {entry}"),
        ]);
    }
    lines.push(format!(
        "{result} = call i64 @pop_rt_allocate_mapped_object(i64 {slot_count}, ptr {map}, i64 {})",
        reference_slots.len()
    ));
    lines
}

fn lower_gc_safe_point(
    result: &str,
    safe_point: u32,
    roots: &[ValueId],
    direct_scalar_arrays: &DirectScalarArrays,
) -> String {
    let roots = roots
        .iter()
        .copied()
        .filter(|root| direct_scalar_arrays.origin(*root).is_none())
        .collect::<Vec<_>>();
    let label = result.trim_start_matches('%');
    let budget = format!("{result}_poll_budget");
    let remaining = format!("{result}_poll_remaining");
    let expired = format!("{result}_poll_expired");
    let expected = format!("{result}_poll_expired_expected");
    let slow = format!("{label}_poll_slow");
    let continuation = format!("{label}_poll_continue");
    let mut lines = vec![
        format!("{budget} = load i32, ptr {GC_POLL_BUDGET}, align 4"),
        format!("{remaining} = sub i32 {budget}, 1"),
        format!("store i32 {remaining}, ptr {GC_POLL_BUDGET}, align 4"),
        format!("{expired} = icmp eq i32 {remaining}, 0"),
        format!("{expected} = call i1 @llvm.expect.i1(i1 {expired}, i1 false)"),
        format!("br i1 {expected}, label %{slow}, label %{continuation}"),
        format!("{slow}:"),
        format!("store i32 {GC_POLL_INTERVAL}, ptr {GC_POLL_BUDGET}, align 4"),
    ];
    if roots.is_empty() {
        lines.extend([
            format!(
                "call i8 @{}(i32 {safe_point}, ptr null, i64 0)",
                RuntimeOperation::GcSafePoint.abi_symbol()
            ),
            format!("br label %{continuation}"),
            format!("{continuation}:"),
        ]);
        return lines.join("\n");
    }
    let root_array = format!("{result}_roots");
    lines.push(format!("{root_array} = alloca [{} x i64]", roots.len()));
    for (index, root) in roots.iter().enumerate() {
        let entry = format!("{root_array}_{index}");
        lines.extend([
            format!(
                "{entry} = getelementptr [{} x i64], ptr {root_array}, i64 0, i64 {index}",
                roots.len()
            ),
            format!("store i64 %v{}, ptr {entry}", root.raw()),
        ]);
    }
    lines.push(format!(
        "call i8 @{}(i32 {safe_point}, ptr {root_array}, i64 {})",
        RuntimeOperation::GcSafePoint.abi_symbol(),
        roots.len()
    ));
    lines.extend([
        format!("br label %{continuation}"),
        format!("{continuation}:"),
    ]);
    lines.join("\n")
}

fn is_managed_type(type_id: TypeId, types: &TypeArena) -> bool {
    !matches!(
        types.get(type_id),
        Some(
            SemanticType::Primitive(
                PrimitiveType::Nil
                    | PrimitiveType::Boolean
                    | PrimitiveType::Integer(_)
                    | PrimitiveType::Float32
                    | PrimitiveType::Float64
                    | PrimitiveType::Never
            ) | SemanticType::Function { .. }
        )
    )
}

fn lower_tuple_make(
    result: &str,
    elements: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let reference_slots = elements
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            values
                .get(value)
                .copied()
                .filter(|type_id| is_managed_type(*type_id, types))
                .and_then(|_| u32::try_from(index).ok())
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(elements.len())
            .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        &reference_slots,
    );
    for (index, value) in elements.iter().enumerate() {
        let type_id = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (conversions, stored) =
            lower_runtime_slot_store(*value, type_id, &llvm_type(type_id, types)?)?;
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            RuntimeOperation::FieldSet.abi_symbol(),
            index + 1
        ));
    }
    Ok(lines.join("\n"))
}

#[allow(clippy::too_many_arguments)]
fn lower_record_update(
    result: &str,
    record: SymbolId,
    base: ValueId,
    updates: &[(FieldId, ValueId)],
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    field_layout: &BTreeMap<FieldId, u32>,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let fields = record_fields
        .get(&record)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let base_type = *values
        .get(&base)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let reference_slots = record_field_types
        .get(&base_type)
        .ok_or(LlvmLoweringError::InvalidType(base_type))?
        .iter()
        .enumerate()
        .filter_map(|(index, type_id)| {
            is_managed_type(*type_id, types)
                .then(|| u32::try_from(index).ok())
                .flatten()
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(fields.len()).map_err(|_| LlvmLoweringError::InvalidType(base_type))?,
        &reference_slots,
    );
    for field in fields {
        let slot = *field_layout
            .get(field)
            .ok_or(LlvmLoweringError::InvalidFieldLayout(*field))?;
        let stored = if let Some((_, value)) = updates.iter().find(|(updated, _)| updated == field)
        {
            let type_id = *values
                .get(value)
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let (conversions, stored) =
                lower_runtime_slot_store(*value, type_id, &llvm_type(type_id, types)?)?;
            lines.extend(conversions);
            stored
        } else {
            let loaded = format!("{result}_field_{slot}");
            lines.push(format!(
                "{loaded} = call i64 @{}(i64 %v{}, i64 {slot})",
                RuntimeOperation::FieldGet.abi_symbol(),
                base.raw()
            ));
            loaded
        };
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {slot}, i64 {stored})",
            RuntimeOperation::FieldSet.abi_symbol()
        ));
    }
    Ok(lines.join("\n"))
}

fn lower_class_make(
    result: &str,
    class: ClassId,
    fields: &[(FieldId, ValueId)],
    slot_count: u32,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<String, LlvmLoweringError> {
    let lowered = lower_object_make(result, fields, slot_count, values, types, field_layout)?;
    let mut lines = lowered.lines().map(str::to_owned).collect::<Vec<_>>();
    lines.insert(
        1,
        format!(
            "call i8 @{}(i64 {result}, i64 1, i64 {})",
            RuntimeOperation::FieldSet.abi_symbol(),
            class.raw()
        ),
    );
    Ok(lines.join("\n"))
}

fn lower_union_make(
    result: &str,
    case: pop_foundation::UnionCaseId,
    arguments: &[ValueId],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let reference_slots = arguments
        .iter()
        .enumerate()
        .filter_map(|(index, value)| {
            values
                .get(value)
                .copied()
                .filter(|type_id| is_managed_type(*type_id, types))
                .and_then(|_| u32::try_from(index + 1).ok())
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(arguments.len() + 1)
            .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        &reference_slots,
    );
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

fn direct_function_tag(symbol: SymbolId) -> u64 {
    (1_u64 << 63) | u64::from(symbol.raw())
}

fn nested_function_tag(owner: SymbolId, function: pop_foundation::NestedFunctionId) -> u64 {
    ((u64::from(owner.raw()) << 32) | u64::from(function.raw())).saturating_add(1)
}

fn lower_capture_cell_allocate(
    result: &str,
    initial: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let type_id = *values
        .get(&initial)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (conversions, stored) =
        lower_runtime_slot_store(initial, type_id, &llvm_type(type_id, types)?)?;
    let reference_slots = is_managed_type(type_id, types)
        .then_some(0)
        .into_iter()
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(result, 1, &reference_slots);
    lines.extend(conversions);
    lines.push(format!(
        "call i8 @{}(i64 {result}, i64 1, i64 {stored})",
        RuntimeOperation::FieldSet.abi_symbol()
    ));
    Ok(lines.join("\n"))
}

fn lower_closure_environment_allocate(
    result: &str,
    owner: SymbolId,
    function: pop_foundation::NestedFunctionId,
    captures: &[pop_mir::MirClosureCapture],
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let reference_slots = captures
        .iter()
        .filter_map(|capture| {
            (capture.self_reference()
                || capture.mode() == pop_mir::MirCaptureMode::Cell
                || is_managed_type(capture.type_id(), types))
            .then_some(capture.slot() + 1)
        })
        .collect::<Vec<_>>();
    let mut lines = lower_mapped_allocation(
        result,
        u32::try_from(captures.len() + 1)
            .map_err(|_| LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?,
        &reference_slots,
    );
    lines.push(format!(
        "call i8 @{}(i64 {result}, i64 1, i64 {})",
        RuntimeOperation::FieldSet.abi_symbol(),
        nested_function_tag(owner, function)
    ));
    for capture in captures {
        let (conversions, stored) = if capture.self_reference() {
            (Vec::new(), result.to_owned())
        } else {
            let value = capture.value();
            let type_id = *values
                .get(&value)
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            lower_runtime_slot_store(value, type_id, &llvm_type(type_id, types)?)?
        };
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            RuntimeOperation::FieldSet.abi_symbol(),
            capture.slot() + 2
        ));
    }
    Ok(lines.join("\n"))
}

fn lower_capture_store(
    owner: &str,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let type_id = *values
        .get(&value)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let (mut lines, stored) =
        lower_runtime_slot_store(value, type_id, &llvm_type(type_id, types)?)?;
    lines.push(format!(
        "call i8 @{}(i64 {owner}, i64 1, i64 {stored})",
        RuntimeOperation::FieldSet.abi_symbol()
    ));
    Ok(lines.join("\n"))
}

fn lower_capture_load(
    result: ValueId,
    result_type: TypeId,
    environment: &str,
    slot: u32,
    mode: pop_mir::MirCaptureMode,
    self_reference: bool,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    if mode == pop_mir::MirCaptureMode::Value || self_reference {
        return lower_runtime_slot_load_from(
            result,
            result_type,
            environment,
            slot as usize + 2,
            types,
        )
        .map(|lines| lines.join("\n"));
    }
    let cell = format!("%v{}_cell", result.raw());
    let mut lines = vec![format!(
        "{cell} = call i64 @{}(i64 {environment}, i64 {})",
        RuntimeOperation::FieldGet.abi_symbol(),
        slot + 2
    )];
    lines.extend(lower_runtime_slot_load_from(
        result,
        result_type,
        &cell,
        1,
        types,
    )?);
    Ok(lines.join("\n"))
}

fn lower_nested_capture_store(
    environment: &str,
    slot: u32,
    value: ValueId,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let cell = format!("%capture_cell_{}", value.raw());
    let mut lines = vec![format!(
        "{cell} = call i64 @{}(i64 {environment}, i64 {})",
        RuntimeOperation::FieldGet.abi_symbol(),
        slot + 2
    )];
    lines.push(lower_capture_store(&cell, value, values, types)?);
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
    lower_runtime_slot_load_from(
        result,
        result_type,
        &format!("%v{}", owner.raw()),
        slot,
        types,
    )
}

fn lower_runtime_slot_load_from(
    result: ValueId,
    result_type: TypeId,
    owner: &str,
    slot: usize,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    let result = format!("%v{}", result.raw());
    lower_runtime_slot_load_named(&result, result_type, owner, slot, types)
}

fn lower_runtime_slot_load_named(
    result: &str,
    result_type: TypeId,
    owner: &str,
    slot: usize,
    types: &TypeArena,
) -> Result<Vec<String>, LlvmLoweringError> {
    let ty = llvm_type(result_type, types)?;
    let loaded = format!("{result}_slot");
    let call = format!(
        "call i64 @{}(i64 {owner}, i64 {slot})",
        RuntimeOperation::FieldGet.abi_symbol(),
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
    if let Some(value) = value {
        let type_id = *values
            .get(&value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (mut lines, stored) =
            lower_runtime_slot_store(value, type_id, &llvm_type(type_id, types)?)?;
        lines.push(format!(
            "call i8 @{}(i64 %v{}, i64 {}, i64 {stored})",
            operation.abi_symbol(),
            base.raw(),
            slot
        ));
        return Ok(lines.join("\n"));
    }
    let result_type =
        result_type.ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    lower_runtime_slot_load_named(
        result,
        result_type,
        &format!("%v{}", base.raw()),
        *slot as usize,
        types,
    )
    .map(|lines| lines.join("\n"))
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
        let type_id = *values
            .get(value)
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
        let (conversions, stored) =
            lower_runtime_slot_store(*value, type_id, &llvm_type(type_id, types)?)?;
        lines.extend(conversions);
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
            RuntimeOperation::ArraySet.abi_symbol(),
            index + 1
        ));
    }
    Ok(lines.join("\n"))
}

fn lower_table_make(
    result: &str,
    entries: &[(ValueId, ValueId)],
    key_map: ArrayElementMap,
    value_map: ArrayElementMap,
    values: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 {}, i1 {}, i1 {})",
        RuntimeOperation::AllocateTable.abi_symbol(),
        entries.len(),
        u8::from(key_map == ArrayElementMap::ManagedReference),
        u8::from(value_map == ArrayElementMap::ManagedReference),
    )];
    for (entry, (key, value)) in entries.iter().enumerate() {
        for (offset, item) in [*key, *value].into_iter().enumerate() {
            let type_id = *values
                .get(&item)
                .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
            let (conversions, stored) =
                lower_runtime_slot_store(item, type_id, &llvm_type(type_id, types)?)?;
            lines.extend(conversions);
            lines.push(format!(
                "call i8 @{}(i64 {result}, i64 {}, i64 {stored})",
                RuntimeOperation::FieldSet.abi_symbol(),
                entry * 2 + offset + 1
            ));
        }
    }
    Ok(lines.join("\n"))
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
