//! LLVM lowering implementation and private artifact machinery.
//!
//! The `Private*` types in this module are deliberately owned by this crate.
//! Canonical MIR never imports them; this is the backend's disposable lowering
//! layer. Textual LLVM IR remains deterministic and inspectable; native object
//! emission parses and verifies that private output with Inkwell before asking
//! LLVM's target machine to write the artifact.

use pop_foundation::{BlockId, BubbleId, FieldId, SymbolId, TypeId, ValueId};
use pop_mir::{
    MirBubble, MirEffect, MirEffectSummary, MirInstructionKind, MirTerminator, verify_mir_bubble,
};
use pop_runtime_interface::{ArrayElementMap, RuntimeOperation};
use pop_target::TargetSpec;
use pop_types::{IntegerKind, PrimitiveType, SemanticType, TypeArena};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::api::{LlvmLoweringError, LlvmLoweringOptions, LlvmModule};
use crate::instruction_lowering::{
    llvm_results, llvm_type, lower_instruction, lower_runtime_slot_load, lower_terminator,
};
use crate::module_lowering::{
    analyze_memory_none_functions, checked_integer_declarations, collect_field_layout,
    collect_record_field_types, collect_record_fields, collect_self_capture_slots,
    collect_string_literals, direct_scalar_array_fill_function,
    lower_builtin_interface_dispatchers, lower_indirect_dispatchers, lower_interface_dispatchers,
    render_string_literals, runtime_declarations,
};

pub(crate) const GC_POLL_BUDGET: &str = "%pop_gc_poll_budget";

pub(crate) fn native_runtime_symbol(operation: RuntimeOperation) -> &'static str {
    pop_runtime_native_abi::symbol(operation)
        .expect("LLVM lowering must validate native runtime capabilities")
}

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
    functions.extend(lower_builtin_interface_dispatchers(bubble, types)?);
    functions.extend(lower_indirect_dispatchers(bubble, types)?);
    let entry_point = options
        .entry_point
        .map(|symbol| lower_entry_point(symbol, bubble, types, options.runtime_profile))
        .transpose()?;
    let mut declarations = vec![
        format!(
            "declare i64 @{}(i64)",
            native_runtime_symbol(RuntimeOperation::AllocateObject)
        ),
        format!(
            "declare i64 @{}(i64, ptr, i64, ptr, i64)",
            native_runtime_symbol(RuntimeOperation::AllocateObjectInitialized)
        ),
        "declare i64 @pop_rt_allocate_mapped_object(i64, ptr, i64)".to_owned(),
        format!(
            "declare i64 @{}(i64, i1)",
            native_runtime_symbol(RuntimeOperation::AllocateArray)
        ),
        format!(
            "declare i64 @{}(i64, i1, i64)",
            native_runtime_symbol(RuntimeOperation::AllocateArrayFilled)
        ),
        format!(
            "declare i64 @{}(i64, i1, i1)",
            native_runtime_symbol(RuntimeOperation::AllocateTable)
        ),
        format!(
            "declare i8 @{}(i32, ptr, i64) cold nounwind",
            if matches!(
                options.runtime_profile,
                pop_backend_api::RuntimeProfile::ProductionGenerational
            ) {
                pop_runtime_native_abi::GC_SAFE_POINT_V2_SYMBOL
            } else {
                native_runtime_symbol(RuntimeOperation::GcSafePoint)
            }
        ),
        format!(
            "declare i64 @{}(i64)",
            native_runtime_symbol(RuntimeOperation::RetainRoot)
        ),
        format!(
            "declare i8 @{}(i64)",
            native_runtime_symbol(RuntimeOperation::ReleaseRoot)
        ),
        format!(
            "declare i64 @{}(i64)",
            native_runtime_symbol(RuntimeOperation::Pin)
        ),
        format!(
            "declare i8 @{}(i64)",
            native_runtime_symbol(RuntimeOperation::Unpin)
        ),
        format!(
            "declare void @{}(i64)",
            native_runtime_symbol(RuntimeOperation::SatbWriteBarrier)
        ),
        format!(
            "declare void @{}() cold noreturn nounwind",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        format!(
            "declare void @{}()",
            native_runtime_symbol(RuntimeOperation::ContinueUnwind)
        ),
        format!(
            "declare i64 @{}(i64, i64)",
            native_runtime_symbol(RuntimeOperation::StringConcat)
        ),
        format!(
            "declare i64 @{}(i32, i64)",
            native_runtime_symbol(RuntimeOperation::StringFormat)
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
    if matches!(
        options.runtime_profile,
        pop_backend_api::RuntimeProfile::ProductionGenerational
    ) {
        declarations.push(format!(
            "declare i8 @{}(i16, i16)",
            pop_runtime_native_abi::ABI_SUPPORT_SYMBOL
        ));
    }
    for reference in bubble.function_references() {
        let result = llvm_results(reference.results(), types)?;
        let parameters = reference
            .parameters()
            .iter()
            .map(|type_id| llvm_type(*type_id, types))
            .collect::<Result<Vec<_>, _>>()?
            .join(", ");
        declarations.push(format!(
            "declare {result} @{}({parameters})",
            function_name(reference.identity().bubble(), reference.identity().symbol())
        ));
    }
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrivateModule {
    pub(crate) globals: Vec<String>,
    pub(crate) declarations: Vec<String>,
    pub(crate) entry_point: Option<String>,
    pub(crate) functions: Vec<PrivateFunction>,
    pub(crate) functions_internal: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrivateFunction {
    pub(crate) name: String,
    pub(crate) parameters: Vec<String>,
    pub(crate) result: String,
    pub(crate) blocks: Vec<PrivateBlock>,
    pub(crate) attributes: Vec<&'static str>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PrivateBlock {
    pub(crate) label: String,
    pub(crate) instructions: Vec<String>,
    pub(crate) terminator: String,
}

impl PrivateModule {
    pub(crate) fn render(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
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

pub(crate) fn lower_entry_point(
    symbol: SymbolId,
    bubble: &MirBubble,
    types: &TypeArena,
    runtime_profile: pop_backend_api::RuntimeProfile,
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
    let abi_guard = if matches!(
        runtime_profile,
        pop_backend_api::RuntimeProfile::ProductionGenerational
    ) {
        let version = pop_runtime_native_abi::NATIVE_ABI_2_VERSION;
        format!(
            "  %pop_abi_supported = call i8 @{}(i16 {}, i16 {})\n  %pop_abi_valid = icmp eq i8 %pop_abi_supported, 1\n  br i1 %pop_abi_valid, label %abi_valid, label %trap\nabi_valid:\n",
            pop_runtime_native_abi::ABI_SUPPORT_SYMBOL,
            version.major(),
            version.minor()
        )
    } else {
        String::new()
    };
    let abi_trap = (!abi_guard.is_empty()).then(|| {
        format!(
            "\ntrap:\n  call void @{}()\n  unreachable",
            native_runtime_symbol(RuntimeOperation::Trap)
        )
    });
    if takes_arguments {
        let invocation = if returns_status {
            format!(
                "  %pop_exit_value = call i64 @{entry}(i64 %pop_arguments)\n  %pop_exit_code = trunc i64 %pop_exit_value to i32\n  ret i32 %pop_exit_code"
            )
        } else {
            format!("  call void @{entry}(i64 %pop_arguments)\n  ret i32 0")
        };
        return Ok(format!(
            "define i32 @main(i32 %pop_argc, ptr %pop_argv) {{\nentry:\n{abi_guard}  %pop_arguments = call i64 @pop_rt_process_arguments(i32 %pop_argc, ptr %pop_argv)\n  %pop_arguments_valid = icmp ne i64 %pop_arguments, 0\n  br i1 %pop_arguments_valid, label %invoke, label %trap\ntrap:\n  call void @pop_rt_trap()\n  unreachable\ninvoke:\n{invocation}\n}}"
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
        "define i32 @main(i32 %pop_argc, ptr %pop_argv) {{\nentry:\n{abi_guard}{invocation}{}\n}}",
        abi_trap.unwrap_or_default()
    ))
}

pub(crate) fn function_name(bubble: BubbleId, symbol: SymbolId) -> String {
    format!("pop_b{}_s{}", bubble.raw(), symbol.raw())
}

pub(crate) fn method_name(bubble: BubbleId, method: pop_foundation::MethodId) -> String {
    format!("pop_b{}_method_{}", bubble.raw(), method.raw())
}

pub(crate) fn interface_name(
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

pub(crate) fn builtin_interface_name(
    bubble: BubbleId,
    receiver: TypeId,
    method: pop_foundation::IterationProtocolMethodId,
) -> String {
    format!(
        "pop_b{}_builtin_interface_t{}_{}",
        bubble.raw(),
        receiver.raw(),
        method.raw()
    )
}

pub(crate) fn nested_name(
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

pub(crate) fn indirect_name(bubble: BubbleId, function_type: TypeId) -> String {
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
pub(crate) fn lower_function(
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
pub(crate) struct DirectScalarArray {
    pub(crate) length: ValueId,
    pub(crate) initial_value: ValueId,
    pub(crate) element_type: TypeId,
    pub(crate) storage: DirectScalarArrayStorage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum DirectScalarArrayStorage {
    Native,
    ScalarReplaced,
}

#[derive(Debug, Default)]
pub(crate) struct DirectScalarArrays {
    pub(crate) allocations: BTreeMap<ValueId, DirectScalarArray>,
    pub(crate) aliases: BTreeMap<ValueId, ValueId>,
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
        let mut entry_allocations = BTreeSet::new();
        for block in blocks {
            for instruction in block.instructions() {
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
                        storage: DirectScalarArrayStorage::ScalarReplaced,
                    },
                );
                aliases.insert(instruction.result(), instruction.result());
                if block.block() == entry.block() {
                    entry_allocations.insert(instruction.result());
                }
            }
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
        let mut mutated = BTreeSet::new();
        for block in blocks {
            for instruction in block.instructions() {
                let used = instruction
                    .operands()
                    .into_iter()
                    .filter_map(|value| aliases.get(&value).copied())
                    .collect::<BTreeSet<_>>();
                for origin in used {
                    if matches!(
                        instruction.kind(),
                        MirInstructionKind::ArraySet { array, .. }
                            | MirInstructionKind::ArrayFill { array, .. }
                            if aliases.get(array).copied() == Some(origin)
                    ) {
                        mutated.insert(origin);
                    }
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
                MirTerminator::ErrorSwitch { scrutinee, .. } => {
                    if let Some(origin) = aliases.get(scrutinee) {
                        rejected.insert(*origin);
                    }
                }
                MirTerminator::Missing
                | MirTerminator::Trap(_)
                | MirTerminator::Panic(_)
                | MirTerminator::ContinueUnwind(_)
                | MirTerminator::ResumeUnwind
                | MirTerminator::Unreachable => {}
            }
        }
        for origin in mutated {
            if entry_allocations.contains(&origin) {
                if let Some(allocation) = allocations.get_mut(&origin) {
                    allocation.storage = DirectScalarArrayStorage::Native;
                }
            } else {
                rejected.insert(origin);
            }
        }
        allocations.retain(|origin, _| !rejected.contains(origin));
        aliases.retain(|_, origin| allocations.contains_key(origin));
        Self {
            allocations,
            aliases,
        }
    }

    pub(crate) fn origin(&self, value: ValueId) -> Option<ValueId> {
        self.aliases.get(&value).copied()
    }

    pub(crate) fn allocation(&self, value: ValueId) -> Option<(ValueId, DirectScalarArray)> {
        let origin = self.origin(value)?;
        self.allocations
            .get(&origin)
            .copied()
            .map(|allocation| (origin, allocation))
    }
}

pub(crate) fn is_direct_scalar_element(type_id: TypeId, types: &TypeArena) -> bool {
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
pub(crate) fn lower_function_parts(
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
    let writable_roots = matches!(
        options.runtime_profile,
        pop_backend_api::RuntimeProfile::ProductionGenerational
    );
    let writable_root_values = function_blocks
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .filter_map(|instruction| match instruction.kind() {
            MirInstructionKind::GcSafePoint { roots, .. } if writable_roots => Some(roots),
            _ => None,
        })
        .flatten()
        .copied()
        .filter(|root| direct_scalar_arrays.origin(*root).is_none())
        .collect::<BTreeSet<_>>();
    let mut incoming_edges: BTreeMap<BlockId, Vec<(BlockId, String, Vec<ValueId>)>> =
        BTreeMap::new();
    let mut union_payload_sources = BTreeMap::new();
    let mut has_union_switch = false;
    for predecessor in function_blocks {
        match predecessor.terminator() {
            MirTerminator::Branch { target, arguments } => {
                incoming_edges.entry(*target).or_default().push((
                    predecessor.block(),
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
                    union_payload_sources.insert(arm.target(), (predecessor.block(), *scrutinee));
                }
            }
            MirTerminator::ErrorSwitch {
                scrutinee, arms, ..
            } => {
                has_union_switch = true;
                for arm in arms {
                    union_payload_sources.insert(arm.target(), (predecessor.block(), *scrutinee));
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
            &writable_root_values,
            types,
        )?;
        instructions.extend(initialize_block_root_cells(block, &writable_root_values));
        if block_index == 0 {
            let mut initialization =
                initialize_gc_poll(has_gc_safe_point, options.gc_poll_interval.get());
            initialization.splice(
                0..0,
                writable_root_values
                    .iter()
                    .map(|root| format!("%v{}_gc_root = alloca i64", root.raw())),
            );
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
            let use_aliases = load_root_cell_uses(
                instruction
                    .operands()
                    .into_iter()
                    .chain(match instruction.kind() {
                        MirInstructionKind::GcSafePoint { roots, .. } => roots.clone(),
                        _ => Vec::new(),
                    }),
                &writable_root_values,
                &format!("v{}", instruction.result().raw()),
                &mut instructions,
            );
            let lowered = lower_instruction(
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
                options,
            )?;
            let lowered = rewrite_relocated_value_uses(&lowered, &use_aliases);
            verify_rewritten_root_uses(
                &lowered,
                &use_aliases,
                &format!("v{}", instruction.result().raw()),
            )?;
            instructions.push(lowered);
            if writable_roots
                && let MirInstructionKind::GcSafePoint { roots, .. } = instruction.kind()
            {
                for root in roots {
                    if writable_root_values.contains(root) {
                        instructions.push(format!(
                            "store i64 %v{}_after_v{}, ptr %v{}_gc_root",
                            root.raw(),
                            instruction.result().raw(),
                            root.raw()
                        ));
                    }
                }
            } else if writable_root_values.contains(&instruction.result()) {
                instructions.push(format!(
                    "store i64 %v{}, ptr %v{}_gc_root",
                    instruction.result().raw(),
                    instruction.result().raw()
                ));
            }
        }
        let mut terminator_prefix = Vec::new();
        let terminator = lower_terminator(
            block.terminator(),
            &value_types,
            types,
            &direct_scalar_arrays,
        )?;
        let terminator_aliases = load_root_cell_uses(
            terminator_values(block.terminator()),
            &writable_root_values,
            &format!("b{}_exit", block.block().raw()),
            &mut terminator_prefix,
        );
        instructions.extend(terminator_prefix);
        let terminator = rewrite_relocated_value_uses(&terminator, &terminator_aliases);
        verify_rewritten_root_uses(
            &terminator,
            &terminator_aliases,
            &format!("b{} terminator", block.block().raw()),
        )?;
        blocks.push(PrivateBlock {
            label: format!("b{}", block.block().raw()),
            instructions,
            terminator,
        });
    }
    if has_union_switch {
        blocks.push(PrivateBlock {
            label: "pop_invalid_union".to_owned(),
            instructions: Vec::new(),
            terminator: format!(
                "call void @{}()\n  unreachable",
                native_runtime_symbol(RuntimeOperation::Trap)
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

fn rewrite_relocated_value_uses(
    text: &str,
    relocated_values: &BTreeMap<ValueId, String>,
) -> String {
    let mut rewritten = text.to_owned();
    for (value, relocated) in relocated_values {
        rewritten = replace_llvm_value_token(&rewritten, &format!("%v{}", value.raw()), relocated);
    }
    rewritten
}

fn initialize_block_root_cells(
    block: &pop_mir::MirBlock,
    writable_root_values: &BTreeSet<ValueId>,
) -> Vec<String> {
    block
        .arguments()
        .iter()
        .filter(|argument| writable_root_values.contains(&argument.value()))
        .map(|argument| {
            format!(
                "store i64 %v{}, ptr %v{}_gc_root",
                argument.value().raw(),
                argument.value().raw()
            )
        })
        .collect()
}

fn load_root_cell_uses(
    values: impl IntoIterator<Item = ValueId>,
    writable_root_values: &BTreeSet<ValueId>,
    location: &str,
    instructions: &mut Vec<String>,
) -> BTreeMap<ValueId, String> {
    values
        .into_iter()
        .filter(|value| writable_root_values.contains(value))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|value| {
            let alias = format!("%v{}_before_{location}", value.raw());
            instructions.push(format!("{alias} = load i64, ptr %v{}_gc_root", value.raw()));
            (value, alias)
        })
        .collect()
}

fn terminator_values(terminator: &MirTerminator) -> Vec<ValueId> {
    match terminator {
        MirTerminator::Branch { arguments, .. } => arguments.clone(),
        MirTerminator::ConditionalBranch { condition, .. } => vec![*condition],
        MirTerminator::UnionSwitch { scrutinee, .. }
        | MirTerminator::ErrorSwitch { scrutinee, .. } => vec![*scrutinee],
        MirTerminator::Return { values } => values.clone(),
        MirTerminator::Missing
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
        | MirTerminator::ResumeUnwind
        | MirTerminator::Unreachable => Vec::new(),
    }
}

fn replace_llvm_value_token(text: &str, original: &str, replacement: &str) -> String {
    let mut rewritten = String::with_capacity(text.len());
    let mut remainder = text;
    while let Some(index) = remainder.find(original) {
        let after = &remainder[index + original.len()..];
        rewritten.push_str(&remainder[..index]);
        if after
            .as_bytes()
            .first()
            .is_some_and(|next| next.is_ascii_alphanumeric() || *next == b'_')
        {
            rewritten.push_str(original);
        } else {
            rewritten.push_str(replacement);
        }
        remainder = after;
    }
    rewritten.push_str(remainder);
    rewritten
}

fn verify_rewritten_root_uses(
    text: &str,
    aliases: &BTreeMap<ValueId, String>,
    location: &str,
) -> Result<(), LlvmLoweringError> {
    for value in aliases.keys() {
        if contains_llvm_value_token(text, &format!("%v{}", value.raw())) {
            return Err(LlvmLoweringError::StaleManagedReference {
                value: *value,
                location: location.to_owned(),
            });
        }
    }
    Ok(())
}

fn contains_llvm_value_token(text: &str, value: &str) -> bool {
    let mut remainder = text;
    while let Some(index) = remainder.find(value) {
        let after = &remainder[index + value.len()..];
        if !after
            .as_bytes()
            .first()
            .is_some_and(|next| next.is_ascii_alphanumeric() || *next == b'_')
        {
            return true;
        }
        remainder = after;
    }
    false
}

#[cfg(test)]
mod relocation_verification_tests {
    use super::*;

    #[test]
    fn writable_root_verifier_rejects_an_old_post_safe_point_ssa_use() {
        let value = ValueId::from_raw(7);
        let aliases = BTreeMap::from([(value, "%v7_before_v9".to_owned())]);

        assert!(verify_rewritten_root_uses("call i64 @consume(i64 %v7)", &aliases, "v9").is_err());
        assert!(
            verify_rewritten_root_uses("call i64 @consume(i64 %v7_before_v9)", &aliases, "v9")
                .is_ok()
        );
    }
}

pub(crate) fn proven_counted_reduction_adds(blocks: &[pop_mir::MirBlock]) -> BTreeSet<ValueId> {
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

pub(crate) fn prove_counted_reduction(
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

pub(crate) fn can_reach_block(
    blocks: &[pop_mir::MirBlock],
    start: BlockId,
    target: BlockId,
) -> bool {
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
            MirTerminator::ErrorSwitch { arms, .. } => {
                pending.extend(arms.iter().map(|arm| arm.target()));
            }
            MirTerminator::Missing
            | MirTerminator::Return { .. }
            | MirTerminator::Trap(_)
            | MirTerminator::Panic(_)
            | MirTerminator::ContinueUnwind(_)
            | MirTerminator::ResumeUnwind
            | MirTerminator::Unreachable => {}
        }
    }
    false
}

pub(crate) fn counted_loop_limit(
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

pub(crate) fn prove_reduction_range(
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

pub(crate) fn llvm_function_attributes(
    effects: MirEffectSummary,
    memory_none: bool,
) -> Vec<&'static str> {
    let mut attributes = Vec::new();
    if memory_none {
        attributes.push("memory(none)");
    }
    if !effects.contains(MirEffect::MayUnwind) {
        attributes.push("nounwind");
    }
    attributes
}

pub(crate) fn llvm_memory_none_instruction(instruction: &MirInstructionKind) -> bool {
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
            | MirInstructionKind::ConvertInteger { .. }
            | MirInstructionKind::ConvertIntegerToFloat { .. }
            | MirInstructionKind::ConvertFloatToInteger { .. }
            | MirInstructionKind::ConvertFloat { .. }
            | MirInstructionKind::FloatAdd { .. }
            | MirInstructionKind::FloatSubtract { .. }
            | MirInstructionKind::FloatMultiply { .. }
            | MirInstructionKind::FloatDivide { .. }
            | MirInstructionKind::FloatNegate { .. }
            | MirInstructionKind::CompareIntegerLess { .. }
            | MirInstructionKind::CompareIntegerLessOrEqual { .. }
            | MirInstructionKind::CompareIntegerGreater { .. }
            | MirInstructionKind::CompareIntegerGreaterOrEqual { .. }
            | MirInstructionKind::CompareFloatLess { .. }
            | MirInstructionKind::CompareFloatLessOrEqual { .. }
            | MirInstructionKind::CompareFloatGreater { .. }
            | MirInstructionKind::CompareFloatGreaterOrEqual { .. }
            | MirInstructionKind::BooleanNot { .. }
            | MirInstructionKind::BooleanAnd { .. }
            | MirInstructionKind::BooleanOr { .. }
    )
}

pub(crate) fn initialize_gc_poll(has_gc_safe_point: bool, interval: u32) -> Vec<String> {
    if !has_gc_safe_point {
        return Vec::new();
    }
    vec![
        format!("{GC_POLL_BUDGET} = alloca i32, align 4"),
        format!("store i32 {interval}, ptr {GC_POLL_BUDGET}, align 4"),
    ]
}

pub(crate) fn initialize_array_outputs(
    blocks: &[pop_mir::MirBlock],
    direct_scalar_arrays: &DirectScalarArrays,
) -> Vec<String> {
    blocks
        .iter()
        .flat_map(pop_mir::MirBlock::instructions)
        .filter(|instruction| {
            matches!(
                instruction.kind(),
                MirInstructionKind::ArrayGet { .. }
                    | MirInstructionKind::TableGet { .. }
                    | MirInstructionKind::ArrayLength { .. }
                    | MirInstructionKind::ArrayGetChecked { .. }
                    | MirInstructionKind::ListGet { .. }
                    | MirInstructionKind::ListLength { .. }
                    | MirInstructionKind::ListGetChecked { .. }
            ) && match instruction.kind() {
                MirInstructionKind::ArrayGet { .. } => true,
                MirInstructionKind::TableGet { .. } => true,
                MirInstructionKind::ListGet { .. } => true,
                MirInstructionKind::ListLength { .. }
                | MirInstructionKind::ListGetChecked { .. } => true,
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

pub(crate) fn llvm_block_exit_label(
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
                | MirInstructionKind::TableSet { .. }
                | MirInstructionKind::ArrayFill { .. } => "continue",
                MirInstructionKind::ListSet { .. } | MirInstructionKind::ListAdd { .. } => {
                    "continue"
                }
                MirInstructionKind::GcSafePoint { .. } => "poll_continue",
                MirInstructionKind::ArrayCreate { .. } => "create",
                MirInstructionKind::ListCreate { .. } => "create",
                MirInstructionKind::ArrayLength { array }
                | MirInstructionKind::ArrayGetChecked { array, .. } => {
                    let _ = direct_scalar_arrays.origin(*array);
                    "load"
                }
                MirInstructionKind::ListLength { .. }
                | MirInstructionKind::ListGetChecked { .. } => "load",
                _ => return None,
            };
            Some(format!("v{}_{suffix}", instruction.result().raw()))
        })
        .unwrap_or_else(|| format!("b{}", block.block().raw()))
}

pub(crate) fn lower_block_arguments(
    block: &pop_mir::MirBlock,
    incoming: Option<&[(BlockId, String, Vec<ValueId>)]>,
    union_payload_source: Option<(BlockId, ValueId)>,
    writable_root_values: &BTreeSet<ValueId>,
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
                    .map(|(predecessor, predecessor_label, values)| {
                        let value = values
                            .get(index)
                            .ok_or(LlvmLoweringError::InvalidType(argument.type_id()))?;
                        let incoming_value = if writable_root_values.contains(value) {
                            format!("%v{}_before_b{}_exit", value.raw(), predecessor.raw())
                        } else {
                            format!("%v{}", value.raw())
                        };
                        Ok(format!("[ {incoming_value}, %{predecessor_label} ]"))
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
    let Some((_predecessor, scrutinee)) = union_payload_source else {
        return Ok(Vec::new());
    };
    let mut instructions = Vec::new();
    let scrutinee_alias = writable_root_values.contains(&scrutinee).then(|| {
        let alias = format!(
            "%v{}_before_b{}_entry",
            scrutinee.raw(),
            block.block().raw()
        );
        instructions.push(format!(
            "{alias} = load i64, ptr %v{}_gc_root",
            scrutinee.raw()
        ));
        alias
    });
    for (index, argument) in block.arguments().iter().enumerate() {
        let loads = lower_runtime_slot_load(
            argument.value(),
            argument.type_id(),
            scrutinee,
            index + 2,
            types,
        )?;
        instructions.extend(loads.into_iter().map(|load| {
            scrutinee_alias.as_ref().map_or(load.clone(), |alias| {
                replace_llvm_value_token(&load, &format!("%v{}", scrutinee.raw()), alias)
            })
        }));
    }
    Ok(instructions)
}
