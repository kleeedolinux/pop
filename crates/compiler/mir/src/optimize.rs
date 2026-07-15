use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{BlockId, MethodId, SymbolId, ValueId};
use pop_types::{FloatValue, IntegerKind, IntegerValue, TypeArena};

use super::{
    MirBubble, MirInstruction, MirInstructionKind, MirTerminator, MirVerificationError,
    block_targets, instruction_operands, local_instruction_effects, verify_mir_bubble,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Constant {
    Integer(IntegerValue),
    Float(FloatValue),
    Boolean(bool),
    String(String),
}

/// Runs portable constant folding and conservative dead code elimination.
///
/// # Errors
///
/// Verifies both the input and optimized output and returns deterministic MIR
/// invariant failures from either boundary.
pub fn optimize_mir(
    mut bubble: MirBubble,
    arena: &TypeArena,
) -> Result<MirBubble, Vec<MirVerificationError>> {
    verify_mir_bubble(&bubble, arena)?;
    for function in &mut bubble.functions {
        summarize_constant_reduction(function);
        fold_constants(function);
        remove_unreachable_blocks(function);
        remove_dead_constants(function);
        refresh_transformed_instruction_effects(function);
        recompute_optimized_effects(function);
    }
    for method in &mut bubble.methods {
        summarize_constant_reduction(&mut method.function);
        fold_constants(&mut method.function);
        remove_unreachable_blocks(&mut method.function);
        remove_dead_constants(&mut method.function);
        refresh_transformed_instruction_effects(&mut method.function);
        recompute_optimized_effects(&mut method.function);
    }
    for nested in &mut bubble.nested_functions {
        let mut function = nested.transformation_adapter();
        summarize_constant_reduction(&mut function);
        fold_constants(&mut function);
        remove_unreachable_blocks(&mut function);
        remove_dead_constants(&mut function);
        refresh_transformed_instruction_effects(&mut function);
        recompute_optimized_effects(&mut function);
        nested.apply_transformation(function);
    }
    refresh_transitive_call_effects(&mut bubble);
    while crate::lowering::insert_gc_safe_points(&mut bubble, arena) {
        refresh_transitive_call_effects(&mut bubble);
    }
    for function in &mut bubble.functions {
        remove_redundant_gc_safe_points(function);
        recompute_optimized_effects(function);
    }
    for method in &mut bubble.methods {
        remove_redundant_gc_safe_points(&mut method.function);
        recompute_optimized_effects(&mut method.function);
    }
    for nested in &mut bubble.nested_functions {
        let mut function = nested.transformation_adapter();
        remove_redundant_gc_safe_points(&mut function);
        recompute_optimized_effects(&mut function);
        nested.apply_transformation(function);
    }
    verify_mir_bubble(&bubble, arena)?;
    Ok(bubble)
}

fn refresh_transitive_call_effects(bubble: &mut MirBubble) {
    let mut function_effects = bubble
        .functions
        .iter()
        .map(|function| (function.symbol, super::MirEffectSummary::empty()))
        .collect::<BTreeMap<SymbolId, _>>();
    let mut method_effects = bubble
        .methods
        .iter()
        .map(|method| (method.method, super::MirEffectSummary::empty()))
        .collect::<BTreeMap<MethodId, _>>();

    loop {
        let previous_functions = function_effects.clone();
        let previous_methods = method_effects.clone();
        let mut changed = false;
        for function in &mut bubble.functions {
            let effects =
                refresh_function_call_effects(function, &previous_functions, &previous_methods);
            changed |= function_effects.insert(function.symbol, effects) != Some(effects);
        }
        for method in &mut bubble.methods {
            let effects = refresh_function_call_effects(
                &mut method.function,
                &previous_functions,
                &previous_methods,
            );
            changed |= method_effects.insert(method.method, effects) != Some(effects);
        }
        for nested in &mut bubble.nested_functions {
            let mut function = nested.transformation_adapter();
            refresh_function_call_effects(&mut function, &previous_functions, &previous_methods);
            nested.apply_transformation(function);
        }
        if !changed {
            break;
        }
    }
}

fn refresh_function_call_effects(
    function: &mut super::MirFunction,
    function_effects: &BTreeMap<SymbolId, super::MirEffectSummary>,
    method_effects: &BTreeMap<MethodId, super::MirEffectSummary>,
) -> super::MirEffectSummary {
    let mut summary = super::MirEffectSummary::empty();
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            let effects = match &mut instruction.kind {
                MirInstructionKind::CallDirect {
                    function,
                    declared_effects,
                    ..
                } => {
                    let effects = function_effects.get(function).copied().unwrap_or_default();
                    *declared_effects = effects;
                    effects
                }
                MirInstructionKind::CallDirectMethod {
                    method,
                    declared_effects,
                    ..
                } => {
                    let effects = method_effects.get(method).copied().unwrap_or_default();
                    *declared_effects = effects;
                    effects
                }
                kind => local_instruction_effects(kind),
            };
            instruction.effects = effects;
            summary = summary.union(effects);
        }
        summary = summary.union(crate::lowering::terminator_effects(&block.terminator));
    }
    function.effects = summary;
    summary
}

fn refresh_transformed_instruction_effects(function: &mut super::MirFunction) {
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            if !matches!(
                instruction.kind,
                MirInstructionKind::CallDirect { .. }
                    | MirInstructionKind::CallReferenced { .. }
                    | MirInstructionKind::CallDirectMethod { .. }
            ) {
                instruction.effects = local_instruction_effects(&instruction.kind);
            }
        }
    }
}

fn remove_redundant_gc_safe_points(function: &mut super::MirFunction) {
    let requires_operation_safe_point = function.blocks.iter().any(|block| {
        block.instructions.iter().any(|instruction| {
            !matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. })
                && (instruction.effects.contains(super::MirEffect::Allocates)
                    || matches!(
                        instruction.kind,
                        MirInstructionKind::CallDirect { .. }
                            | MirInstructionKind::CallDirectMethod { .. }
                            | MirInstructionKind::CallIndirect { .. }
                    ) && instruction.effects.contains(super::MirEffect::GcSafePoint))
        })
    });
    let has_backedge = function.blocks.iter().any(|block| {
        block_targets(block)
            .into_iter()
            .any(|target| target.raw() <= block.block.raw())
    });
    let requires_periodic_safe_point = function.blocks.iter().any(|block| {
        block
            .instructions
            .iter()
            .filter(|instruction| {
                !matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. })
            })
            .count()
            >= crate::ir::MAX_STRAIGHT_LINE_WORK_BETWEEN_SAFE_POINTS
    });
    if requires_operation_safe_point || has_backedge || requires_periodic_safe_point {
        return;
    }
    for block in &mut function.blocks {
        block.instructions.retain(|instruction| {
            !matches!(instruction.kind, MirInstructionKind::GcSafePoint { .. })
        });
    }
}

fn recompute_optimized_effects(function: &mut super::MirFunction) {
    function.effects =
        function
            .blocks
            .iter()
            .fold(super::MirEffectSummary::empty(), |summary, block| {
                block
                    .instructions
                    .iter()
                    .fold(summary, |summary, instruction| {
                        summary.union(instruction.effects)
                    })
                    .union(crate::lowering::terminator_effects(&block.terminator))
            });
}

struct CountedReductionSummary {
    exit: BlockId,
    induction: IntegerValue,
    accumulator: IntegerValue,
    type_id: pop_foundation::TypeId,
    span: pop_foundation::SourceSpan,
}

fn summarize_constant_reduction(function: &mut super::MirFunction) {
    let Some(summary) = constant_reduction_summary(function) else {
        return;
    };
    let Some(next_value) = function
        .blocks
        .iter()
        .flat_map(|block| {
            block.arguments.iter().map(|argument| argument.value).chain(
                block
                    .instructions
                    .iter()
                    .map(|instruction| instruction.result),
            )
        })
        .map(ValueId::raw)
        .max()
        .unwrap_or(0)
        .checked_add(1)
    else {
        return;
    };
    let Some(accumulator_value) = next_value.checked_add(1) else {
        return;
    };
    let induction = ValueId::from_raw(next_value);
    let accumulator = ValueId::from_raw(accumulator_value);
    let entry = &mut function.blocks[0];
    entry.instructions.extend([
        MirInstruction {
            result: induction,
            result_type: Some(summary.type_id),
            kind: MirInstructionKind::IntegerConstant(summary.induction),
            effects: super::MirEffectSummary::empty(),
            effects_explicit: true,
            unwind: super::MirUnwindAction::Propagate,
            span: summary.span,
        },
        MirInstruction {
            result: accumulator,
            result_type: Some(summary.type_id),
            kind: MirInstructionKind::IntegerConstant(summary.accumulator),
            effects: super::MirEffectSummary::empty(),
            effects_explicit: true,
            unwind: super::MirUnwindAction::Propagate,
            span: summary.span,
        },
    ]);
    entry.terminator = MirTerminator::Branch {
        target: summary.exit,
        arguments: vec![induction, accumulator],
    };
}

#[allow(clippy::too_many_lines)]
fn constant_reduction_summary(function: &super::MirFunction) -> Option<CountedReductionSummary> {
    let constants = function
        .blocks
        .iter()
        .flat_map(|block| &block.instructions)
        .filter_map(|instruction| match instruction.kind {
            MirInstructionKind::IntegerConstant(value) if value.kind() == IntegerKind::Int64 => {
                Some((instruction.result, value))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let entry = function.blocks.first()?;
    let MirTerminator::Branch {
        target: body_id,
        arguments: initial,
    } = &entry.terminator
    else {
        return None;
    };
    if initial.len() != 2 {
        return None;
    }
    let body = function
        .blocks
        .iter()
        .find(|block| block.block == *body_id)?;
    let [induction, accumulator] = body.arguments.as_slice() else {
        return None;
    };
    if body.instructions.len() != 3
        || !body.instructions.iter().all(|instruction| {
            matches!(
                instruction.kind,
                MirInstructionKind::IntegerConstant(_)
                    | MirInstructionKind::CheckedIntegerAdd {
                        kind: IntegerKind::Int64,
                        ..
                    }
            )
        })
    {
        return None;
    }
    let (next_induction, step_value) = body.instructions.iter().find_map(|instruction| {
        let MirInstructionKind::CheckedIntegerAdd {
            kind: IntegerKind::Int64,
            left,
            right,
        } = instruction.kind
        else {
            return None;
        };
        if left == induction.value && constants.contains_key(&right) {
            Some((instruction.result, right))
        } else if right == induction.value && constants.contains_key(&left) {
            Some((instruction.result, left))
        } else {
            None
        }
    })?;
    let reduction = body.instructions.iter().find(|instruction| {
        matches!(
            instruction.kind,
            MirInstructionKind::CheckedIntegerAdd {
                kind: IntegerKind::Int64,
                left,
                right,
            } if (left == accumulator.value && right == induction.value)
                || (right == accumulator.value && left == induction.value)
        )
    })?;
    let MirTerminator::Branch {
        target: condition_id,
        arguments,
    } = &body.terminator
    else {
        return None;
    };
    if !arguments.is_empty() {
        return None;
    }
    let condition = function
        .blocks
        .iter()
        .find(|block| block.block == *condition_id)?;
    if condition.instructions.len() != 2 {
        return None;
    }
    let (comparison, limit) =
        condition
            .instructions
            .iter()
            .find_map(|instruction| match instruction.kind {
                MirInstructionKind::CompareEqual { left, right } if left == next_induction => {
                    constants
                        .get(&right)
                        .copied()
                        .map(|limit| (instruction.result, limit))
                }
                MirInstructionKind::CompareEqual { left, right } if right == next_induction => {
                    constants
                        .get(&left)
                        .copied()
                        .map(|limit| (instruction.result, limit))
                }
                _ => None,
            })?;
    let MirTerminator::ConditionalBranch {
        condition: branch_condition,
        when_true,
        when_false,
    } = condition.terminator
    else {
        return None;
    };
    if branch_condition != comparison {
        return None;
    }
    let backedge = function
        .blocks
        .iter()
        .find(|block| block.block == when_false)?;
    if backedge.instructions.len() != 1
        || !matches!(
            backedge.instructions[0].kind,
            MirInstructionKind::GcSafePoint { .. }
        )
        || !matches!(
            &backedge.terminator,
            MirTerminator::Branch { target, arguments }
                if *target == body.block
                    && arguments == &[next_induction, reduction.result]
        )
    {
        return None;
    }
    let bridge = function
        .blocks
        .iter()
        .find(|block| block.block == when_true)?;
    let MirTerminator::Branch {
        target: exit,
        arguments: exit_arguments,
    } = &bridge.terminator
    else {
        return None;
    };
    if !bridge.instructions.is_empty()
        || exit_arguments != &[next_induction, reduction.result]
        || block_reaches(function, *exit, body.block)
    {
        return None;
    }
    let initial_induction = constants.get(&initial[0])?.signed()?;
    let initial_accumulator = constants.get(&initial[1])?.signed()?;
    let step = constants.get(&step_value)?.signed()?;
    let limit_signed = limit.signed()?;
    let accumulator_value = reduction_sum(
        i128::from(initial_induction),
        i128::from(initial_accumulator),
        i128::from(step),
        i128::from(limit_signed),
    )?;
    let accumulator =
        IntegerValue::parse_decimal(&accumulator_value.to_string(), IntegerKind::Int64).ok()?;
    Some(CountedReductionSummary {
        exit: *exit,
        induction: limit,
        accumulator,
        type_id: reduction.result_type?,
        span: reduction.span,
    })
}

fn reduction_sum(initial: i128, accumulator: i128, step: i128, limit: i128) -> Option<i128> {
    let distance = limit.checked_sub(initial)?;
    if initial < 0 || accumulator < 0 || step <= 0 || distance <= 0 || distance % step != 0 {
        return None;
    }
    let iterations = distance / step;
    let last_offset = (iterations - 1).checked_mul(step)?;
    let series_factor = initial.checked_mul(2)?.checked_add(last_offset)?;
    let series = iterations.checked_mul(series_factor)? / 2;
    accumulator.checked_add(series)
}

fn block_reaches(function: &super::MirFunction, start: BlockId, target: BlockId) -> bool {
    let mut pending = vec![start];
    let mut visited = BTreeSet::new();
    while let Some(block_id) = pending.pop() {
        if block_id == target {
            return true;
        }
        if !visited.insert(block_id) {
            continue;
        }
        if let Some(block) = function.blocks.iter().find(|block| block.block == block_id) {
            pending.extend(block_targets(block));
        }
    }
    false
}

fn fold_constants(function: &mut super::MirFunction) {
    let mut constants = BTreeMap::new();
    for block in &mut function.blocks {
        for instruction in &mut block.instructions {
            if let Some(folded) = fold_instruction(&instruction.kind, &constants) {
                instruction.kind = folded;
                instruction.effects = local_instruction_effects(&instruction.kind);
            }
            if let Some(constant) = constant_from_instruction(&instruction.kind) {
                constants.insert(instruction.result, constant);
            }
        }
        if let MirTerminator::ConditionalBranch {
            condition,
            when_true,
            when_false,
        } = block.terminator
            && let Some(Constant::Boolean(value)) = constants.get(&condition)
        {
            block.terminator = MirTerminator::Branch {
                target: if *value { when_true } else { when_false },
                arguments: Vec::new(),
            };
        }
    }
}

fn fold_instruction(
    kind: &MirInstructionKind,
    constants: &BTreeMap<ValueId, Constant>,
) -> Option<MirInstructionKind> {
    let integer = |value| match constants.get(&value) {
        Some(Constant::Integer(value)) => Some(*value),
        _ => None,
    };
    let boolean = |value| match constants.get(&value) {
        Some(Constant::Boolean(value)) => Some(*value),
        _ => None,
    };
    let float = |value| match constants.get(&value) {
        Some(Constant::Float(value)) => Some(*value),
        _ => None,
    };
    let string = |value| match constants.get(&value) {
        Some(Constant::String(value)) => Some(value.clone()),
        _ => None,
    };
    Some(match kind {
        MirInstructionKind::CheckedIntegerAdd { left, right, .. } => {
            MirInstructionKind::IntegerConstant(integer(*left)?.checked_add(integer(*right)?).ok()?)
        }
        MirInstructionKind::CheckedIntegerSubtract { left, right, .. } => {
            MirInstructionKind::IntegerConstant(
                integer(*left)?.checked_subtract(integer(*right)?).ok()?,
            )
        }
        MirInstructionKind::CheckedIntegerMultiply { left, right, .. } => {
            MirInstructionKind::IntegerConstant(
                integer(*left)?.checked_multiply(integer(*right)?).ok()?,
            )
        }
        MirInstructionKind::CheckedIntegerDivide { left, right, .. } => {
            MirInstructionKind::IntegerConstant(
                integer(*left)?.checked_divide(integer(*right)?).ok()?,
            )
        }
        MirInstructionKind::CheckedIntegerRemainder { left, right, .. } => {
            MirInstructionKind::IntegerConstant(
                integer(*left)?.checked_remainder(integer(*right)?).ok()?,
            )
        }
        MirInstructionKind::IntegerNegate { operand, .. } => {
            MirInstructionKind::IntegerConstant(integer(*operand)?.checked_negate().ok()?)
        }
        MirInstructionKind::FloatAdd { left, right, .. } => {
            MirInstructionKind::FloatConstant(float(*left)?.checked_add(float(*right)?).ok()?)
        }
        MirInstructionKind::FloatSubtract { left, right, .. } => {
            MirInstructionKind::FloatConstant(float(*left)?.checked_subtract(float(*right)?).ok()?)
        }
        MirInstructionKind::FloatMultiply { left, right, .. } => {
            MirInstructionKind::FloatConstant(float(*left)?.checked_multiply(float(*right)?).ok()?)
        }
        MirInstructionKind::FloatDivide { left, right, .. } => {
            MirInstructionKind::FloatConstant(float(*left)?.checked_divide(float(*right)?).ok()?)
        }
        MirInstructionKind::FloatNegate { operand, .. } => {
            MirInstructionKind::FloatConstant(float(*operand)?.negate())
        }
        MirInstructionKind::ConvertInteger {
            target, operand, ..
        } => MirInstructionKind::IntegerConstant(integer(*operand)?.convert(*target).ok()?),
        MirInstructionKind::ConvertIntegerToFloat {
            target, operand, ..
        } => MirInstructionKind::FloatConstant(integer(*operand)?.to_float(*target)),
        MirInstructionKind::ConvertFloatToInteger {
            target, operand, ..
        } => MirInstructionKind::IntegerConstant(float(*operand)?.to_integer(*target).ok()?),
        MirInstructionKind::ConvertFloat {
            target, operand, ..
        } => MirInstructionKind::FloatConstant(float(*operand)?.convert(*target)),
        MirInstructionKind::StringConcat { left, right } => {
            let mut value = string(*left)?;
            value.push_str(&string(*right)?);
            MirInstructionKind::StringConstant(value)
        }
        MirInstructionKind::StringFormat { kind, value } => {
            let formatted = match kind {
                pop_types::StringFormatKind::Boolean => boolean(*value)?.to_string(),
                pop_types::StringFormatKind::Integer(expected) => {
                    let value = integer(*value)?;
                    if value.kind() != *expected {
                        return None;
                    }
                    value.to_string()
                }
                pop_types::StringFormatKind::Float(expected) => {
                    let value = float(*value)?;
                    if value.kind() != *expected {
                        return None;
                    }
                    value.format_string()
                }
            };
            MirInstructionKind::StringConstant(formatted)
        }
        MirInstructionKind::BooleanNot { operand } => {
            MirInstructionKind::BooleanConstant(!boolean(*operand)?)
        }
        MirInstructionKind::BooleanAnd { left, right } => {
            MirInstructionKind::BooleanConstant(boolean(*left)? && boolean(*right)?)
        }
        MirInstructionKind::BooleanOr { left, right } => {
            MirInstructionKind::BooleanConstant(boolean(*left)? || boolean(*right)?)
        }
        MirInstructionKind::CompareEqual { left, right } => MirInstructionKind::BooleanConstant(
            constant_equal(constants.get(left)?, constants.get(right)?),
        ),
        MirInstructionKind::CompareNotEqual { left, right } => MirInstructionKind::BooleanConstant(
            !constant_equal(constants.get(left)?, constants.get(right)?),
        ),
        MirInstructionKind::CompareIntegerLess { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                integer(*left)?.compare(integer(*right)?).ok()?.is_lt(),
            )
        }
        MirInstructionKind::CompareIntegerGreater { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                integer(*left)?.compare(integer(*right)?).ok()?.is_gt(),
            )
        }
        MirInstructionKind::CompareIntegerLessOrEqual { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                integer(*left)?.compare(integer(*right)?).ok()?.is_le(),
            )
        }
        MirInstructionKind::CompareIntegerGreaterOrEqual { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                integer(*left)?.compare(integer(*right)?).ok()?.is_ge(),
            )
        }
        MirInstructionKind::CompareFloatLess { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                float(*left)?
                    .partial_compare(float(*right)?)
                    .ok()?
                    .is_some_and(std::cmp::Ordering::is_lt),
            )
        }
        MirInstructionKind::CompareFloatLessOrEqual { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                float(*left)?
                    .partial_compare(float(*right)?)
                    .ok()?
                    .is_some_and(std::cmp::Ordering::is_le),
            )
        }
        MirInstructionKind::CompareFloatGreater { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                float(*left)?
                    .partial_compare(float(*right)?)
                    .ok()?
                    .is_some_and(std::cmp::Ordering::is_gt),
            )
        }
        MirInstructionKind::CompareFloatGreaterOrEqual { left, right, .. } => {
            MirInstructionKind::BooleanConstant(
                float(*left)?
                    .partial_compare(float(*right)?)
                    .ok()?
                    .is_some_and(std::cmp::Ordering::is_ge),
            )
        }
        _ => return None,
    })
}

fn constant_from_instruction(kind: &MirInstructionKind) -> Option<Constant> {
    Some(match kind {
        MirInstructionKind::IntegerConstant(value) => Constant::Integer(*value),
        MirInstructionKind::FloatConstant(value) => Constant::Float(*value),
        MirInstructionKind::BooleanConstant(value) => Constant::Boolean(*value),
        MirInstructionKind::StringConstant(value) => Constant::String(value.clone()),
        _ => return None,
    })
}

fn constant_equal(left: &Constant, right: &Constant) -> bool {
    match (left, right) {
        (Constant::Float(left), Constant::Float(right)) => left
            .partial_compare(*right)
            .is_ok_and(|ordering| ordering.is_some_and(std::cmp::Ordering::is_eq)),
        _ => left == right,
    }
}

fn remove_unreachable_blocks(function: &mut super::MirFunction) {
    let mut reachable = BTreeSet::new();
    let mut pending = vec![BlockId::from_raw(0)];
    while let Some(block) = pending.pop() {
        if !reachable.insert(block) {
            continue;
        }
        let Some(block) = function.blocks.get(block.raw() as usize) else {
            continue;
        };
        pending.extend(block_targets(block));
    }
    let mapping: BTreeMap<_, _> = reachable
        .iter()
        .enumerate()
        .map(|(new, old)| {
            (
                *old,
                BlockId::from_raw(u32::try_from(new).unwrap_or(u32::MAX)),
            )
        })
        .collect();
    function
        .blocks
        .retain(|block| reachable.contains(&block.block));
    for block in &mut function.blocks {
        block.block = mapping[&block.block];
        for instruction in &mut block.instructions {
            match &mut instruction.kind {
                MirInstructionKind::CallDirect {
                    unwind: super::MirUnwindAction::Cleanup(target),
                    ..
                }
                | MirInstructionKind::CallDirectMethod {
                    unwind: super::MirUnwindAction::Cleanup(target),
                    ..
                }
                | MirInstructionKind::CallIndirect {
                    unwind: super::MirUnwindAction::Cleanup(target),
                    ..
                } => *target = mapping[target],
                _ => {}
            }
        }
        remap_terminator(&mut block.terminator, &mapping);
    }
}

fn remap_terminator(terminator: &mut MirTerminator, mapping: &BTreeMap<BlockId, BlockId>) {
    match terminator {
        MirTerminator::Branch { target, .. } => *target = mapping[target],
        MirTerminator::ConditionalBranch {
            when_true,
            when_false,
            ..
        } => {
            *when_true = mapping[when_true];
            *when_false = mapping[when_false];
        }
        MirTerminator::UnionSwitch { arms, .. } => {
            for arm in arms {
                arm.target = mapping[&arm.target];
            }
        }
        MirTerminator::ErrorSwitch { arms, .. } => {
            for arm in arms {
                arm.target = mapping[&arm.target];
            }
        }
        MirTerminator::Suspend {
            resume,
            cancellation,
            unwind,
            ..
        } => {
            *resume = mapping[resume];
            *cancellation = mapping[cancellation];
            if let super::MirUnwindAction::Cleanup(target) = unwind {
                *target = mapping[target];
            }
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

fn remove_dead_constants(function: &mut super::MirFunction) {
    loop {
        let used = used_values(function);
        let mut changed = false;
        for block in &mut function.blocks {
            block.instructions.retain(|instruction| {
                let remove = !used.contains(&instruction.result)
                    && matches!(
                        instruction.kind,
                        MirInstructionKind::IntegerConstant(_)
                            | MirInstructionKind::FloatConstant(_)
                            | MirInstructionKind::StringConstant(_)
                            | MirInstructionKind::BooleanConstant(_)
                            | MirInstructionKind::NilConstant
                            | MirInstructionKind::FunctionReference(_)
                    );
                changed |= remove;
                !remove
            });
        }
        if !changed {
            break;
        }
    }
}

fn used_values(function: &super::MirFunction) -> BTreeSet<ValueId> {
    let mut used = BTreeSet::new();
    for block in &function.blocks {
        for instruction in &block.instructions {
            used.extend(instruction_operands(&instruction.kind));
        }
        match &block.terminator {
            MirTerminator::Branch { arguments, .. }
            | MirTerminator::Return { values: arguments } => {
                used.extend(arguments.iter().copied());
            }
            MirTerminator::ConditionalBranch { condition, .. } => {
                used.insert(*condition);
            }
            MirTerminator::UnionSwitch { scrutinee, .. } => {
                used.insert(*scrutinee);
            }
            MirTerminator::ErrorSwitch { scrutinee, .. } => {
                used.insert(*scrutinee);
            }
            MirTerminator::Suspend {
                operation,
                live_frame,
                ..
            } => {
                match operation {
                    super::MirSuspendOperation::Task { task, .. } => {
                        used.insert(*task);
                    }
                }
                used.extend(live_frame.slots.iter().map(|slot| slot.value));
            }
            MirTerminator::Missing
            | MirTerminator::Trap(_)
            | MirTerminator::Panic(_)
            | MirTerminator::ContinueUnwind(_)
            | MirTerminator::ResumeUnwind
            | MirTerminator::Unreachable => {}
        }
    }
    used
}
