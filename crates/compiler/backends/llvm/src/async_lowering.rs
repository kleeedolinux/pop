//! LLVM-private stackless coroutine frame and poll lowering.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{BubbleId, FieldId, SymbolId, TypeId, ValueId};
use pop_mir::{
    MirCancellationMode, MirCleanupExitReason, MirEffectSummary, MirFfiLayoutCatalog,
    MirInstructionKind, MirSuspendOperation, MirTerminator, MirUnwindAction,
};
use pop_runtime_interface::RuntimeOperation;
use pop_types::TypeArena;

use crate::api::{LlvmLoweringError, LlvmLoweringOptions};
use crate::instruction_lowering::{
    is_managed_type, llvm_type, lower_instruction, lower_runtime_slot_load_named,
    optional_inner_type,
};
use crate::lowering::{
    CaptureEnvironment, DirectScalarArrays, PrivateBlock, PrivateFunction,
    async_function_create_name, async_function_poll_name, async_nested_create_name,
    async_nested_poll_name, initialize_array_outputs, native_runtime_symbol,
    replace_llvm_value_token,
};

#[derive(Clone, Copy)]
struct FrameValue {
    offset: u32,
    type_id: TypeId,
}

struct FrameLayout {
    values: BTreeMap<ValueId, FrameValue>,
    slot_count: u32,
}

impl FrameLayout {
    fn new(blocks: &[pop_mir::MirBlock], types: &TypeArena) -> Result<Self, LlvmLoweringError> {
        let mut values = BTreeMap::new();
        let mut next = 2_u32;
        for (value, type_id) in blocks.iter().flat_map(|block| {
            block
                .arguments()
                .iter()
                .map(|argument| (argument.value(), argument.type_id()))
                .chain(block.instructions().iter().filter_map(|instruction| {
                    instruction
                        .optional_result_type()
                        .map(|type_id| (instruction.result(), type_id))
                }))
        }) {
            if values.contains_key(&value) {
                continue;
            }
            let width = frame_width(type_id, types)?;
            values.insert(
                value,
                FrameValue {
                    offset: next,
                    type_id,
                },
            );
            next = next
                .checked_add(width)
                .ok_or(LlvmLoweringError::InvalidType(type_id))?;
        }
        Ok(Self {
            values,
            slot_count: next,
        })
    }

    fn value(&self, value: ValueId) -> Result<FrameValue, LlvmLoweringError> {
        self.values
            .get(&value)
            .copied()
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))
    }

    fn roots_for_values(
        &self,
        values: impl IntoIterator<Item = ValueId>,
        types: &TypeArena,
    ) -> Result<Vec<u32>, LlvmLoweringError> {
        let mut roots = BTreeSet::new();
        for value in values {
            let layout = self.value(value)?;
            append_type_roots(layout.type_id, layout.offset, types, &mut roots)?;
        }
        Ok(roots.into_iter().collect())
    }
}

fn frame_width(type_id: TypeId, types: &TypeArena) -> Result<u32, LlvmLoweringError> {
    if let Some(inner) = optional_inner_type(types, type_id) {
        return 1_u32
            .checked_add(frame_width(inner, types)?)
            .ok_or(LlvmLoweringError::InvalidType(type_id));
    }
    let _ = llvm_type(type_id, types)?;
    Ok(1)
}

fn append_type_roots(
    type_id: TypeId,
    offset: u32,
    types: &TypeArena,
    roots: &mut BTreeSet<u32>,
) -> Result<(), LlvmLoweringError> {
    if let Some(inner) = optional_inner_type(types, type_id) {
        return append_type_roots(inner, offset + 1, types, roots);
    }
    if crate::instruction_lowering::is_managed_type(type_id, types) {
        roots.insert(offset);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_async_function(
    bubble: BubbleId,
    function: &pop_mir::MirFunction,
    types: &TypeArena,
    ffi_layouts: &MirFfiLayoutCatalog,
    options: LlvmLoweringOptions,
    field_layout: &BTreeMap<FieldId, u32>,
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    string_literals: &BTreeMap<String, String>,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    lower_async_parts(
        bubble,
        &async_function_poll_name(bubble, function.symbol()),
        async_function_create_name(bubble, function.symbol()),
        function.results(),
        function.effects(),
        function.blocks(),
        None,
        types,
        ffi_layouts,
        options,
        field_layout,
        record_fields,
        record_field_types,
        string_literals,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn lower_async_nested(
    bubble: BubbleId,
    function: &pop_mir::MirNestedFunction,
    types: &TypeArena,
    ffi_layouts: &MirFfiLayoutCatalog,
    options: LlvmLoweringOptions,
    field_layout: &BTreeMap<FieldId, u32>,
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    string_literals: &BTreeMap<String, String>,
    self_capture_slots: &BTreeSet<u32>,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    lower_async_parts(
        bubble,
        &async_nested_poll_name(bubble, function.owner(), function.function()),
        async_nested_create_name(bubble, function.owner(), function.function()),
        function.results(),
        function.effects(),
        function.blocks(),
        Some(("%environment", self_capture_slots)),
        types,
        ffi_layouts,
        options,
        field_layout,
        record_fields,
        record_field_types,
        string_literals,
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn lower_async_parts(
    bubble: BubbleId,
    poll_name: &str,
    create_name: String,
    result_types: &[TypeId],
    effects: MirEffectSummary,
    blocks: &[pop_mir::MirBlock],
    environment: Option<(&str, &BTreeSet<u32>)>,
    types: &TypeArena,
    ffi_layouts: &MirFfiLayoutCatalog,
    options: LlvmLoweringOptions,
    field_layout: &BTreeMap<FieldId, u32>,
    record_fields: &BTreeMap<SymbolId, Vec<FieldId>>,
    record_field_types: &BTreeMap<TypeId, Vec<TypeId>>,
    string_literals: &BTreeMap<String, String>,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    let layout = FrameLayout::new(blocks, types)?;
    let value_types = layout
        .values
        .iter()
        .map(|(value, layout)| (*value, layout.type_id))
        .collect::<BTreeMap<_, _>>();
    let direct_scalar_arrays = DirectScalarArrays::default();
    let mut poll_blocks = vec![poll_entry(
        blocks,
        &direct_scalar_arrays,
        environment.is_some(),
        options.gc_poll_interval.get(),
    )];
    let suspend_states = blocks
        .iter()
        .filter(|block| matches!(block.terminator(), MirTerminator::Suspend { .. }))
        .enumerate()
        .map(|(index, block)| (block.block(), u32::try_from(index + 1).unwrap_or(u32::MAX)))
        .collect::<BTreeMap<_, _>>();
    let switch_payload_sources = blocks
        .iter()
        .flat_map(|block| match block.terminator() {
            MirTerminator::UnionSwitch {
                scrutinee, arms, ..
            } => arms
                .iter()
                .map(|arm| (arm.target(), *scrutinee))
                .collect::<Vec<_>>(),
            MirTerminator::ErrorSwitch {
                scrutinee, arms, ..
            } => arms
                .iter()
                .map(|arm| (arm.target(), *scrutinee))
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .collect::<BTreeMap<_, _>>();
    poll_blocks[0].terminator = poll_dispatch(&suspend_states);
    for block in blocks {
        let mut instructions = Vec::new();
        if let Some(scrutinee) = switch_payload_sources.get(&block.block()) {
            load_switch_payloads(&mut instructions, block, *scrutinee, &layout, types)?;
        }
        for instruction in block.instructions() {
            let mut aliases = BTreeMap::new();
            let explicit_roots = match instruction.kind() {
                MirInstructionKind::GcSafePoint { roots, .. } => roots.clone(),
                _ => Vec::new(),
            };
            for operand in instruction
                .operands()
                .into_iter()
                .chain(explicit_roots)
                .collect::<BTreeSet<_>>()
            {
                let alias = format!("%v{}_at_v{}", operand.raw(), instruction.result().raw());
                load_frame_value(
                    &mut instructions,
                    "%pop_frame",
                    &alias,
                    layout.value(operand)?,
                    types,
                )?;
                aliases.insert(operand, alias);
            }
            let mut lowered = lower_instruction(
                bubble,
                instruction,
                &value_types,
                types,
                ffi_layouts,
                field_layout,
                record_fields,
                record_field_types,
                string_literals,
                environment.map_or(CaptureEnvironment::None, |(name, slots)| {
                    CaptureEnvironment::Managed(name, slots)
                }),
                &BTreeSet::new(),
                &direct_scalar_arrays,
                options,
            )?;
            for (value, alias) in aliases {
                lowered = replace_llvm_value_token(&lowered, &format!("%v{}", value.raw()), &alias);
            }
            lowered = contain_async_traps(&lowered);
            instructions.push(lowered);
            if instruction.has_result() {
                store_frame_value(
                    &mut instructions,
                    "%pop_frame",
                    &format!("%v{}", instruction.result().raw()),
                    layout.value(instruction.result())?,
                    types,
                    &format!("v{}_frame", instruction.result().raw()),
                )?;
            }
        }
        let terminator = lower_async_terminator(
            block,
            blocks,
            result_types,
            &layout,
            &suspend_states,
            types,
            &mut instructions,
        )?;
        poll_blocks.push(PrivateBlock {
            label: format!("b{}", block.block().raw()),
            instructions,
            terminator,
        });
    }
    let poll = PrivateFunction {
        name: poll_name.to_owned(),
        parameters: vec![
            "i64 %pop_task".to_owned(),
            "i64 %pop_frame".to_owned(),
            "i8 %pop_cancelled".to_owned(),
        ],
        result: "i8".to_owned(),
        blocks: poll_blocks,
        attributes: Vec::new(),
    };
    let create = lower_create_helper(
        create_name,
        poll_name,
        blocks,
        &layout,
        environment.is_some(),
        result_types,
        types,
        effects,
    )?;
    Ok(vec![poll, create])
}

fn poll_entry(
    blocks: &[pop_mir::MirBlock],
    direct_scalar_arrays: &DirectScalarArrays,
    has_environment: bool,
    gc_poll_interval: u32,
) -> PrivateBlock {
    let mut instructions = vec![
        "%pop_gc_poll_budget = alloca i32".to_owned(),
        format!("store i32 {gc_poll_interval}, ptr %pop_gc_poll_budget, align 4"),
        "%pop_state_out = alloca i64".to_owned(),
        format!(
            "%pop_state_ok = call i8 @{}(i64 %pop_frame, i32 0, ptr %pop_state_out)",
            native_runtime_symbol(RuntimeOperation::TaskFrameLoad)
        ),
        "%pop_state = load i64, ptr %pop_state_out".to_owned(),
    ];
    instructions.extend(initialize_array_outputs(blocks, direct_scalar_arrays));
    if has_environment {
        instructions.extend([
            "%pop_environment_out = alloca i64".to_owned(),
            format!(
                "%pop_environment_ok = call i8 @{}(i64 %pop_frame, i32 1, ptr %pop_environment_out)",
                native_runtime_symbol(RuntimeOperation::TaskFrameLoad)
            ),
            "%environment = load i64, ptr %pop_environment_out".to_owned(),
        ]);
    }
    PrivateBlock {
        label: "entry".to_owned(),
        instructions,
        terminator: String::new(),
    }
}

fn poll_dispatch(states: &BTreeMap<pop_foundation::BlockId, u32>) -> String {
    let mut cases = vec!["    i64 0, label %b0".to_owned()];
    cases.extend(
        states
            .iter()
            .map(|(block, state)| format!("    i64 {state}, label %s{}", block.raw())),
    );
    format!(
        "switch i64 %pop_state, label %pop_invalid_state [\n{}\n  ]\n  pop_invalid_state:\n  call void @{}()\n  unreachable",
        cases.join("\n"),
        native_runtime_symbol(RuntimeOperation::Trap)
    )
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn lower_async_terminator(
    block: &pop_mir::MirBlock,
    blocks: &[pop_mir::MirBlock],
    result_types: &[TypeId],
    layout: &FrameLayout,
    suspend_states: &BTreeMap<pop_foundation::BlockId, u32>,
    types: &TypeArena,
    instructions: &mut Vec<String>,
) -> Result<String, LlvmLoweringError> {
    match block.terminator() {
        MirTerminator::Branch { target, arguments } => {
            copy_edge_arguments(
                instructions,
                blocks,
                block.block(),
                *target,
                arguments,
                layout,
                types,
            )?;
            Ok(format!("br label %b{}", target.raw()))
        }
        MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } => {
            let alias = format!("%v{}_at_b{}_exit", condition.raw(), block.block().raw());
            load_frame_value(
                instructions,
                "%pop_frame",
                &alias,
                layout.value(*condition)?,
                types,
            )?;
            Ok(format!(
                "br i1 {alias}, label %b{}, label %b{}",
                when_true.raw(),
                when_false.raw()
            ))
        }
        MirTerminator::Return { values } => {
            let completion = lower_completion_value(
                instructions,
                values,
                result_types,
                layout,
                types,
                &format!("b{}_return", block.block().raw()),
            )?;
            instructions.push(format!(
                "%pop_completion_stored_b{} = call i8 @{}(i64 %pop_task, i64 {completion})",
                block.block().raw(),
                native_runtime_symbol(RuntimeOperation::TaskCompletionStore)
            ));
            Ok(format!(
                "%pop_completion_valid_b{} = icmp eq i8 %pop_completion_stored_b{}, 1\n%pop_completion_status_b{} = select i1 %pop_completion_valid_b{}, i8 3, i8 5\nret i8 %pop_completion_status_b{}",
                block.block().raw(),
                block.block().raw(),
                block.block().raw(),
                block.block().raw(),
                block.block().raw(),
            ))
        }
        MirTerminator::Suspend {
            operation: MirSuspendOperation::Task { task, result_type },
            resume,
            cancellation,
            cancellation_mode,
            unwind,
            live_frame,
            ..
        } => {
            let state = suspend_states[&block.block()];
            let [resume_argument] = blocks
                .get(resume.raw() as usize)
                .ok_or(LlvmLoweringError::InvalidType(*result_type))?
                .arguments()
            else {
                return Err(LlvmLoweringError::InvalidType(*result_type));
            };
            let roots = layout
                .roots_for_values(live_frame.slots().iter().map(|slot| slot.value()), types)?;
            let root_setup = root_array(
                "pop_suspend_roots",
                &block.block().raw().to_string(),
                &roots,
            );
            let task_layout = layout.value(*task)?;
            let mut retry = Vec::new();
            let task_alias = format!("%v{}_at_suspend_{}", task.raw(), block.block().raw());
            load_frame_value(&mut retry, "%pop_frame", &task_alias, task_layout, types)?;
            retry.extend([
                format!("%pop_await_out_{} = alloca i64", block.block().raw()),
                format!(
                    "%pop_await_status_{} = call i8 @{}(i64 {task_alias}, ptr %pop_await_out_{})",
                    block.block().raw(),
                    native_runtime_symbol(RuntimeOperation::TaskAwait),
                    block.block().raw()
                ),
            ]);
            let cancellation_check = if *cancellation_mode == MirCancellationMode::Observe {
                format!(
                    "%pop_cancelled_b{} = icmp ne i8 %pop_cancelled, 0\n  br i1 %pop_cancelled_b{}, label %b{}, label %s{}",
                    block.block().raw(),
                    block.block().raw(),
                    cancellation.raw(),
                    block.block().raw()
                )
            } else {
                format!("br label %s{}", block.block().raw())
            };
            let mut pending = vec![format!(
                "call i8 @{}(i64 %pop_frame, i32 0, i64 {state})",
                native_runtime_symbol(RuntimeOperation::TaskFrameStore)
            )];
            pending.extend(root_setup.0);
            pending.push(format!(
                "%pop_live_map_b{} = call i8 @{}(i64 %pop_frame, i32 {}, ptr {}, i64 {})",
                block.block().raw(),
                native_runtime_symbol(RuntimeOperation::TaskFrameSetLiveMap),
                live_frame.stack_map().safe_point().raw(),
                root_setup.1,
                roots.len()
            ));
            pending.extend([
                format!(
                    "%pop_live_map_valid_b{} = icmp eq i8 %pop_live_map_b{}, 1",
                    block.block().raw(),
                    block.block().raw()
                ),
                format!(
                    "br i1 %pop_live_map_valid_b{}, label %pop_live_map_ready_b{}, label %pop_live_map_trap_b{}",
                    block.block().raw(),
                    block.block().raw(),
                    block.block().raw()
                ),
                format!("pop_live_map_trap_b{}:", block.block().raw()),
                format!(
                    "  call void @{}()",
                    native_runtime_symbol(RuntimeOperation::Trap)
                ),
                "  unreachable".to_owned(),
                format!("pop_live_map_ready_b{}:", block.block().raw()),
            ]);
            let mut completed = Vec::new();
            let raw = format!("%pop_await_value_{}", block.block().raw());
            completed.push(format!(
                "{raw} = load i64, ptr %pop_await_out_{}",
                block.block().raw()
            ));
            let value = decode_completion(
                &mut completed,
                &raw,
                *result_type,
                types,
                &format!("await_b{}", block.block().raw()),
            )?;
            store_frame_value(
                &mut completed,
                "%pop_frame",
                &value,
                layout.value(resume_argument.value())?,
                types,
                &format!("await_b{}_frame", block.block().raw()),
            )?;
            let panic_target = match unwind {
                MirUnwindAction::Cleanup(target) => format!("b{}", target.raw()),
                MirUnwindAction::Propagate => format!("pop_unwind_{}", block.block().raw()),
            };
            Ok(format!(
                "{cancellation_check}\n  s{}:\n  {}\n  switch i8 %pop_await_status_{}, label %pop_invalid_suspend_{} [\n    i8 1, label %pop_pending_{}\n    i8 2, label %pop_pending_{}\n    i8 3, label %pop_completed_{}\n    i8 4, label %b{}\n    i8 5, label %{}\n  ]\n  pop_pending_{}:\n  {}\n  ret i8 2\n  pop_completed_{}:\n  {}\n  br label %b{}\n  pop_unwind_{}:\n  ret i8 5\n  pop_invalid_suspend_{}:\n  call void @{}()\n  unreachable",
                block.block().raw(),
                retry.join("\n  "),
                block.block().raw(),
                block.block().raw(),
                block.block().raw(),
                block.block().raw(),
                block.block().raw(),
                cancellation.raw(),
                panic_target,
                block.block().raw(),
                pending.join("\n  "),
                block.block().raw(),
                completed.join("\n  "),
                resume.raw(),
                block.block().raw(),
                block.block().raw(),
                native_runtime_symbol(RuntimeOperation::Trap),
            ))
        }
        MirTerminator::Trap(_) => Ok("ret i8 5".to_owned()),
        MirTerminator::ContinueUnwind(pop_runtime_interface::UnwindReason::Cancellation) => {
            Ok("ret i8 4".to_owned())
        }
        MirTerminator::ResumeUnwind
            if block
                .cleanup()
                .is_some_and(|cleanup| cleanup.reason() == MirCleanupExitReason::Cancellation) =>
        {
            Ok("ret i8 4".to_owned())
        }
        MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(pop_runtime_interface::UnwindReason::Panic(_))
        | MirTerminator::ResumeUnwind => Ok("ret i8 5".to_owned()),
        MirTerminator::Unreachable | MirTerminator::Missing => Ok("unreachable".to_owned()),
        MirTerminator::UnionSwitch {
            scrutinee, arms, ..
        } => lower_async_switch(
            block,
            *scrutinee,
            arms.iter()
                .map(|arm| (arm.case().raw(), arm.target().raw())),
            layout,
            types,
            instructions,
        ),
        MirTerminator::ErrorSwitch {
            scrutinee, arms, ..
        } => lower_async_switch(
            block,
            *scrutinee,
            arms.iter()
                .map(|arm| (arm.case().raw(), arm.target().raw())),
            layout,
            types,
            instructions,
        ),
    }
}

fn contain_async_traps(lowered: &str) -> String {
    let trap = format!(
        "call void @{}()\n  unreachable",
        native_runtime_symbol(RuntimeOperation::Trap)
    );
    lowered.replace(&trap, "ret i8 5")
}

fn load_switch_payloads(
    instructions: &mut Vec<String>,
    block: &pop_mir::MirBlock,
    scrutinee: ValueId,
    layout: &FrameLayout,
    types: &TypeArena,
) -> Result<(), LlvmLoweringError> {
    let owner = format!("%switch_owner_b{}", block.block().raw());
    load_frame_value(
        instructions,
        "%pop_frame",
        &owner,
        layout.value(scrutinee)?,
        types,
    )?;
    for (index, argument) in block.arguments().iter().enumerate() {
        let payload = format!("%switch_payload_b{}_{}", block.block().raw(), index);
        instructions.extend(lower_runtime_slot_load_named(
            &payload,
            argument.type_id(),
            &owner,
            index + 2,
            types,
        )?);
        store_frame_value(
            instructions,
            "%pop_frame",
            &payload,
            layout.value(argument.value())?,
            types,
            &format!("switch_payload_b{}_{}", block.block().raw(), index),
        )?;
    }
    Ok(())
}

fn lower_async_switch(
    block: &pop_mir::MirBlock,
    scrutinee: ValueId,
    arms: impl Iterator<Item = (u32, u32)>,
    layout: &FrameLayout,
    types: &TypeArena,
    instructions: &mut Vec<String>,
) -> Result<String, LlvmLoweringError> {
    let owner = format!("%switch_value_b{}", block.block().raw());
    load_frame_value(
        instructions,
        "%pop_frame",
        &owner,
        layout.value(scrutinee)?,
        types,
    )?;
    let tag = format!("%switch_tag_b{}", block.block().raw());
    instructions.push(format!(
        "{tag} = call i64 @{}(i64 {owner}, i64 1)",
        native_runtime_symbol(RuntimeOperation::FieldGet)
    ));
    let cases = arms
        .map(|(case, target)| format!("    i64 {case}, label %b{target}"))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(format!(
        "switch i64 {tag}, label %pop_invalid_switch_b{} [\n{cases}\n  ]\n  pop_invalid_switch_b{}:\n  call void @{}()\n  unreachable",
        block.block().raw(),
        block.block().raw(),
        native_runtime_symbol(RuntimeOperation::Trap)
    ))
}

fn copy_edge_arguments(
    instructions: &mut Vec<String>,
    blocks: &[pop_mir::MirBlock],
    source: pop_foundation::BlockId,
    target: pop_foundation::BlockId,
    arguments: &[ValueId],
    layout: &FrameLayout,
    types: &TypeArena,
) -> Result<(), LlvmLoweringError> {
    let target = blocks
        .get(target.raw() as usize)
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    for (index, (argument, parameter)) in arguments.iter().zip(target.arguments()).enumerate() {
        let alias = format!(
            "%edge_{}_{}_{}_{}",
            source.raw(),
            target.block().raw(),
            index,
            argument.raw()
        );
        load_frame_value(
            instructions,
            "%pop_frame",
            &alias,
            layout.value(*argument)?,
            types,
        )?;
        store_frame_value(
            instructions,
            "%pop_frame",
            &alias,
            layout.value(parameter.value())?,
            types,
            &format!("edge_{}_{}_{}", source.raw(), target.block().raw(), index),
        )?;
    }
    Ok(())
}

fn lower_completion_value(
    instructions: &mut Vec<String>,
    values: &[ValueId],
    result_types: &[TypeId],
    layout: &FrameLayout,
    types: &TypeArena,
    label: &str,
) -> Result<String, LlvmLoweringError> {
    if values.is_empty() {
        return Ok("0".to_owned());
    }
    if values.len() == 1 {
        let value = values[0];
        let alias = format!("%{label}_value");
        load_frame_value(
            instructions,
            "%pop_frame",
            &alias,
            layout.value(value)?,
            types,
        )?;
        return encode_completion(instructions, &alias, result_types[0], types, label);
    }
    let roots = result_types
        .iter()
        .enumerate()
        .filter_map(|(index, type_id)| {
            crate::instruction_lowering::is_managed_type(*type_id, types).then_some(index as u32)
        })
        .collect::<Vec<_>>();
    let root_setup = root_array("pop_completion_roots", label, &roots);
    instructions.extend(root_setup.0);
    let object = format!("%{label}_tuple");
    instructions.push(format!(
        "{object} = call i64 @pop_rt_allocate_mapped_object(i64 {}, ptr {}, i64 {})",
        values.len(),
        root_setup.1,
        roots.len()
    ));
    for (index, (value, type_id)) in values.iter().zip(result_types).enumerate() {
        let alias = format!("%{label}_{index}");
        load_frame_value(
            instructions,
            "%pop_frame",
            &alias,
            layout.value(*value)?,
            types,
        )?;
        let encoded = encode_scalar(
            instructions,
            &alias,
            *type_id,
            types,
            &format!("{label}_{index}"),
        )?;
        instructions.push(format!(
            "call i8 @{}(i64 {object}, i64 {}, i64 {encoded})",
            native_runtime_symbol(RuntimeOperation::FieldSet),
            index + 1
        ));
    }
    Ok(object)
}

fn encode_completion(
    instructions: &mut Vec<String>,
    value: &str,
    type_id: TypeId,
    types: &TypeArena,
    label: &str,
) -> Result<String, LlvmLoweringError> {
    if let Some(inner) = optional_inner_type(types, type_id) {
        let present = format!("%{label}_present");
        let inner_value = format!("%{label}_inner");
        instructions.push(format!(
            "{present} = extractvalue {{ i1, {} }} {value}, 0",
            llvm_type(inner, types)?
        ));
        instructions.push(format!(
            "{inner_value} = extractvalue {{ i1, {} }} {value}, 1",
            llvm_type(inner, types)?
        ));
        let present_raw = format!("%{label}_present_raw");
        instructions.push(format!("{present_raw} = zext i1 {present} to i64"));
        let inner_raw = encode_scalar(instructions, &inner_value, inner, types, label)?;
        let roots = crate::instruction_lowering::is_managed_type(inner, types)
            .then_some(vec![1_u32])
            .unwrap_or_default();
        let root_setup = root_array("pop_optional_roots", label, &roots);
        instructions.extend(root_setup.0);
        let object = format!("%{label}_optional");
        instructions.push(format!(
            "{object} = call i64 @pop_rt_allocate_mapped_object(i64 2, ptr {}, i64 {})",
            root_setup.1,
            roots.len()
        ));
        instructions.push(format!(
            "call i8 @{}(i64 {object}, i64 1, i64 {present_raw})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ));
        instructions.push(format!(
            "call i8 @{}(i64 {object}, i64 2, i64 {inner_raw})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ));
        return Ok(object);
    }
    encode_scalar(instructions, value, type_id, types, label)
}

fn decode_completion(
    instructions: &mut Vec<String>,
    raw: &str,
    type_id: TypeId,
    types: &TypeArena,
    label: &str,
) -> Result<String, LlvmLoweringError> {
    if let Some(inner) = optional_inner_type(types, type_id) {
        let present_raw = format!("%{label}_present_raw");
        let inner_raw = format!("%{label}_inner_raw");
        instructions.push(format!(
            "{present_raw} = call i64 @{}(i64 {raw}, i64 1)",
            native_runtime_symbol(RuntimeOperation::FieldGet)
        ));
        instructions.push(format!(
            "{inner_raw} = call i64 @{}(i64 {raw}, i64 2)",
            native_runtime_symbol(RuntimeOperation::FieldGet)
        ));
        let present = format!("%{label}_present");
        instructions.push(format!("{present} = trunc i64 {present_raw} to i1"));
        let inner_value = decode_scalar(instructions, &inner_raw, inner, types, label)?;
        let base = format!("%{label}_optional_base");
        let result = format!("%{label}_optional");
        let ty = llvm_type(inner, types)?;
        instructions.push(format!(
            "{base} = insertvalue {{ i1, {ty} }} zeroinitializer, i1 {present}, 0"
        ));
        instructions.push(format!(
            "{result} = insertvalue {{ i1, {ty} }} {base}, {ty} {inner_value}, 1"
        ));
        return Ok(result);
    }
    decode_scalar(instructions, raw, type_id, types, label)
}

fn encode_scalar(
    instructions: &mut Vec<String>,
    value: &str,
    type_id: TypeId,
    types: &TypeArena,
    label: &str,
) -> Result<String, LlvmLoweringError> {
    let ty = llvm_type(type_id, types)?;
    let raw = format!("%{label}_raw");
    match ty.as_str() {
        "i64" => Ok(value.to_owned()),
        "i1" | "i8" | "i16" | "i32" => {
            instructions.push(format!("{raw} = zext {ty} {value} to i64"));
            Ok(raw)
        }
        "float" => {
            instructions.push(format!("{raw}_bits = bitcast float {value} to i32"));
            instructions.push(format!("{raw} = zext i32 {raw}_bits to i64"));
            Ok(raw)
        }
        "double" => {
            instructions.push(format!("{raw} = bitcast double {value} to i64"));
            Ok(raw)
        }
        _ => Err(LlvmLoweringError::InvalidType(type_id)),
    }
}

fn decode_scalar(
    instructions: &mut Vec<String>,
    raw: &str,
    type_id: TypeId,
    types: &TypeArena,
    label: &str,
) -> Result<String, LlvmLoweringError> {
    let ty = llvm_type(type_id, types)?;
    let result = format!("%{label}_decoded");
    match ty.as_str() {
        "i64" => Ok(raw.to_owned()),
        "i1" | "i8" | "i16" | "i32" => {
            instructions.push(format!("{result} = trunc i64 {raw} to {ty}"));
            Ok(result)
        }
        "float" => {
            instructions.push(format!("{result}_bits = trunc i64 {raw} to i32"));
            instructions.push(format!("{result} = bitcast i32 {result}_bits to float"));
            Ok(result)
        }
        "double" => {
            instructions.push(format!("{result} = bitcast i64 {raw} to double"));
            Ok(result)
        }
        _ => Err(LlvmLoweringError::InvalidType(type_id)),
    }
}

fn store_frame_value(
    instructions: &mut Vec<String>,
    frame: &str,
    value: &str,
    layout: FrameValue,
    types: &TypeArena,
    label: &str,
) -> Result<(), LlvmLoweringError> {
    if let Some(inner) = optional_inner_type(types, layout.type_id) {
        let ty = llvm_type(inner, types)?;
        let present = format!("%{label}_present");
        let inner_value = format!("%{label}_inner");
        instructions.push(format!(
            "{present} = extractvalue {{ i1, {ty} }} {value}, 0"
        ));
        instructions.push(format!(
            "{inner_value} = extractvalue {{ i1, {ty} }} {value}, 1"
        ));
        let present_raw = encode_scalar(
            instructions,
            &present,
            types
                .source_type("Boolean")
                .ok_or(LlvmLoweringError::InvalidType(layout.type_id))?,
            types,
            &format!("{label}_present"),
        )?;
        instructions.push(format!(
            "call i8 @{}(i64 {frame}, i32 {}, i64 {present_raw})",
            native_runtime_symbol(RuntimeOperation::TaskFrameStore),
            layout.offset
        ));
        let inner_layout = FrameValue {
            offset: layout.offset + 1,
            type_id: inner,
        };
        return store_frame_value(
            instructions,
            frame,
            &inner_value,
            inner_layout,
            types,
            &format!("{label}_inner"),
        );
    }
    let raw = encode_scalar(instructions, value, layout.type_id, types, label)?;
    instructions.push(format!(
        "call i8 @{}(i64 {frame}, i32 {}, i64 {raw})",
        native_runtime_symbol(RuntimeOperation::TaskFrameStore),
        layout.offset
    ));
    Ok(())
}

fn load_frame_value(
    instructions: &mut Vec<String>,
    frame: &str,
    result: &str,
    layout: FrameValue,
    types: &TypeArena,
) -> Result<(), LlvmLoweringError> {
    if let Some(inner) = optional_inner_type(types, layout.type_id) {
        let present_raw = format!("{result}_present_raw");
        let present_out = format!("{result}_present_out");
        instructions.push(format!("{present_out} = alloca i64"));
        instructions.push(format!(
            "{result}_present_ok = call i8 @{}(i64 {frame}, i32 {}, ptr {present_out})",
            native_runtime_symbol(RuntimeOperation::TaskFrameLoad),
            layout.offset
        ));
        instructions.push(format!("{present_raw} = load i64, ptr {present_out}"));
        let present = format!("{result}_present");
        instructions.push(format!("{present} = trunc i64 {present_raw} to i1"));
        let inner_result = format!("{result}_inner");
        load_frame_value(
            instructions,
            frame,
            &inner_result,
            FrameValue {
                offset: layout.offset + 1,
                type_id: inner,
            },
            types,
        )?;
        let ty = llvm_type(inner, types)?;
        instructions.push(format!(
            "{result}_base = insertvalue {{ i1, {ty} }} zeroinitializer, i1 {present}, 0"
        ));
        instructions.push(format!(
            "{result} = insertvalue {{ i1, {ty} }} {result}_base, {ty} {inner_result}, 1"
        ));
        return Ok(());
    }
    let output = format!("{result}_out");
    let raw = format!("{result}_raw");
    instructions.push(format!("{output} = alloca i64"));
    instructions.push(format!(
        "{result}_ok = call i8 @{}(i64 {frame}, i32 {}, ptr {output})",
        native_runtime_symbol(RuntimeOperation::TaskFrameLoad),
        layout.offset
    ));
    instructions.push(format!("{raw} = load i64, ptr {output}"));
    let decoded = decode_scalar(instructions, &raw, layout.type_id, types, &result[1..])?;
    if decoded != result {
        let ty = llvm_type(layout.type_id, types)?;
        let operation = if matches!(ty.as_str(), "float" | "double") {
            "fadd"
        } else {
            "add"
        };
        let zero = if matches!(ty.as_str(), "float" | "double") {
            "0.000000e+00"
        } else {
            "0"
        };
        instructions.push(format!("{result} = {operation} {ty} {decoded}, {zero}"));
    }
    Ok(())
}

fn root_array(prefix: &str, identity: &str, roots: &[u32]) -> (Vec<String>, String) {
    if roots.is_empty() {
        return (Vec::new(), "null".to_owned());
    }
    let array = format!("%{prefix}_{identity}");
    let mut lines = vec![format!("{array} = alloca [{} x i32]", roots.len())];
    for (index, root) in roots.iter().enumerate() {
        let slot = format!("{array}_{index}");
        lines.push(format!(
            "{slot} = getelementptr [{} x i32], ptr {array}, i64 0, i64 {index}",
            roots.len()
        ));
        lines.push(format!("store i32 {root}, ptr {slot}"));
    }
    (lines, array)
}

fn lower_create_helper(
    name: String,
    poll_name: &str,
    blocks: &[pop_mir::MirBlock],
    layout: &FrameLayout,
    has_environment: bool,
    result_types: &[TypeId],
    types: &TypeArena,
    _effects: MirEffectSummary,
) -> Result<PrivateFunction, LlvmLoweringError> {
    let entry = blocks
        .first()
        .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))?;
    let mut parameters = Vec::new();
    if has_environment {
        parameters.push("i64 %environment".to_owned());
    }
    parameters.extend(
        entry
            .arguments()
            .iter()
            .map(|argument| {
                llvm_type(argument.type_id(), types)
                    .map(|ty| format!("{ty} %v{}", argument.value().raw()))
            })
            .collect::<Result<Vec<_>, _>>()?,
    );
    parameters.push("i64 %pop_cancel_token".to_owned());
    let mut roots = layout.roots_for_values(
        entry.arguments().iter().map(|argument| argument.value()),
        types,
    )?;
    if has_environment {
        roots.push(1);
        roots.sort_unstable();
        roots.dedup();
    }
    let root_setup = root_array("pop_initial_roots", "create", &roots);
    let mut instructions = root_setup.0;
    instructions.push(format!(
        "%pop_frame = call i64 @{}(i64 {}, i32 0, ptr {}, i64 {})",
        native_runtime_symbol(RuntimeOperation::TaskFrameCreate),
        layout.slot_count,
        root_setup.1,
        roots.len()
    ));
    instructions.extend([
        "%pop_frame_valid = icmp ne i64 %pop_frame, 0".to_owned(),
        "br i1 %pop_frame_valid, label %pop_frame_ready, label %pop_frame_trap".to_owned(),
        "pop_frame_trap:".to_owned(),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        "pop_frame_ready:".to_owned(),
    ]);
    instructions.push(format!(
        "call i8 @{}(i64 %pop_frame, i32 0, i64 0)",
        native_runtime_symbol(RuntimeOperation::TaskFrameStore)
    ));
    if has_environment {
        instructions.push(format!(
            "call i8 @{}(i64 %pop_frame, i32 1, i64 %environment)",
            native_runtime_symbol(RuntimeOperation::TaskFrameStore)
        ));
    }
    for argument in entry.arguments() {
        store_frame_value(
            &mut instructions,
            "%pop_frame",
            &format!("%v{}", argument.value().raw()),
            layout.value(argument.value())?,
            types,
            &format!("initial_v{}", argument.value().raw()),
        )?;
    }
    instructions.push(format!(
        "%pop_created_task = call i64 @{}(i64 %pop_frame, ptr @{poll_name}, i64 %pop_cancel_token, i8 {})",
        native_runtime_symbol(RuntimeOperation::TaskCreate),
        u8::from(match result_types {
            [] => false,
            [result] => is_managed_type(*result, types),
            _ => true,
        })
    ));
    instructions.extend([
        "%pop_created_task_valid = icmp ne i64 %pop_created_task, 0".to_owned(),
        "br i1 %pop_created_task_valid, label %pop_created_task_ready, label %pop_created_task_trap"
            .to_owned(),
        "pop_created_task_trap:".to_owned(),
        format!(
            "  call void @{}()",
            native_runtime_symbol(RuntimeOperation::Trap)
        ),
        "  unreachable".to_owned(),
        "pop_created_task_ready:".to_owned(),
    ]);
    Ok(PrivateFunction {
        name,
        parameters,
        result: "i64".to_owned(),
        blocks: vec![PrivateBlock {
            label: "entry".to_owned(),
            instructions,
            terminator: "ret i64 %pop_created_task".to_owned(),
        }],
        attributes: Vec::new(),
    })
}
