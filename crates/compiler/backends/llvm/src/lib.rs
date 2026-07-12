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
use pop_foundation::{BlockId, ClassId, FieldId, FunctionId, SymbolId, TypeId, ValueId};
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
    let self_capture_slots = collect_self_capture_slots(bubble);
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
    for nested in bubble.nested_functions() {
        let self_slots = self_capture_slots
            .get(&(nested.owner(), nested.function()))
            .cloned()
            .unwrap_or_default();
        functions.push(lower_function_parts(
            format!(
                "pop_nested_{}_{}",
                nested.owner().raw(),
                nested.function().raw()
            ),
            nested.parameters(),
            nested.results(),
            nested.blocks(),
            Some(("%environment", &self_slots)),
            types,
            options,
            &field_layout,
            &string_literals,
        )?);
    }
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
    declarations.extend(checked_integer_declarations());
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
    [
        RuntimeOperation::AllocateTable,
        RuntimeOperation::ArrayGet,
        RuntimeOperation::FieldGet,
        RuntimeOperation::RecordUpdate,
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
    .collect()
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
                    "call void @pop_method_{}({arguments})",
                    class_method.raw()
                )],
                "ret void".to_owned(),
            )
        } else {
            (
                vec![format!(
                    "{dispatch_result} = call {result_type} @pop_method_{}({arguments})",
                    class_method.raw()
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
        name: format!(
            "pop_interface_{}_{}",
            interface.raw(),
            method.method().raw()
        ),
        parameters,
        result: result_type,
        blocks,
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
            &format!("@pop_s{}", function.symbol().raw()),
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
                "@pop_nested_{}_{}",
                function.owner().raw(),
                function.function().raw()
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
        name: format!("pop_indirect_t{}", function_type.raw()),
        parameters,
        result: result_type,
        blocks,
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
    lower_function_parts(
        format!("pop_s{}", function.symbol().raw()),
        function.parameters(),
        function.results(),
        function.blocks(),
        None,
        types,
        options,
        field_layout,
        string_literals,
    )
}

#[allow(clippy::too_many_arguments)]
fn lower_function_parts(
    name: String,
    parameter_types: &[TypeId],
    result_types: &[TypeId],
    function_blocks: &[pop_mir::MirBlock],
    environment: Option<(&str, &BTreeSet<u32>)>,
    types: &TypeArena,
    options: LlvmLoweringOptions,
    field_layout: &BTreeMap<FieldId, u32>,
    string_literals: &BTreeMap<String, String>,
) -> Result<PrivateFunction, LlvmLoweringError> {
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
    let mut incoming_edges: BTreeMap<BlockId, Vec<(String, Vec<ValueId>)>> = BTreeMap::new();
    let mut union_payload_sources = BTreeMap::new();
    let mut has_union_switch = false;
    for predecessor in function_blocks {
        match predecessor.terminator() {
            MirTerminator::Branch { target, arguments } => {
                incoming_edges
                    .entry(*target)
                    .or_default()
                    .push((llvm_block_exit_label(predecessor), arguments.clone()));
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
    for block in function_blocks {
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
                environment,
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
    })
}

fn llvm_block_exit_label(block: &pop_mir::MirBlock) -> String {
    block
        .instructions()
        .iter()
        .rev()
        .find(|instruction| {
            matches!(
                instruction.kind(),
                MirInstructionKind::CheckedIntegerAdd { .. }
                    | MirInstructionKind::CheckedIntegerSubtract { .. }
                    | MirInstructionKind::CheckedIntegerMultiply { .. }
                    | MirInstructionKind::CheckedIntegerDivide { .. }
                    | MirInstructionKind::CheckedIntegerRemainder { .. }
                    | MirInstructionKind::IntegerNegate { .. }
            )
        })
        .map_or_else(
            || format!("b{}", block.block().raw()),
            |instruction| format!("v{}_continue", instruction.result().raw()),
        )
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
fn lower_instruction(
    instruction: &pop_mir::MirInstruction,
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    string_literals: &BTreeMap<String, String>,
    environment: Option<(&str, &BTreeSet<u32>)>,
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
            lower_checked_integer_binary(&result, "add", *kind, *left, *right)
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
            format!("{result} = add i64 0, {}", direct_function_tag(*symbol))
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
                &format!("@pop_indirect_t{}", callee_type.raw()),
                &arguments,
                value_types,
                types,
            )?
        }
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
    format!(
        "br i1 {condition}, label %{label}_trap, label %{label}_continue\n{label}_trap:\n  call void @{}()\n  unreachable\n{label}_continue:",
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
            slot,
            value.raw()
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
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 1)",
        RuntimeOperation::AllocateObject.abi_symbol()
    )];
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
    let mut lines = vec![format!(
        "{result} = call i64 @{}(i64 {})",
        RuntimeOperation::AllocateObject.abi_symbol(),
        captures.len() + 1
    )];
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
    let ty = llvm_type(result_type, types)?;
    let result = format!("%v{}", result.raw());
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
    let assignment = result_type.map_or_else(String::new, |_| format!("{result} = "));
    let return_type = result_type.map_or_else(|| Ok("i8".to_owned()), |id| llvm_type(id, types))?;
    let value_text = value
        .map(|value| format!(", i64 %v{}", value.raw()))
        .unwrap_or_default();
    Ok(format!(
        "{assignment}call {return_type} @{}(i64 %v{}, i64 {}{})",
        operation.abi_symbol(),
        base.raw(),
        slot,
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
