use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{BlockId, ValueId};
use pop_types::{IntegerValue, TypeArena};

use super::{
    MirBubble, MirInstructionKind, MirTerminator, MirVerificationError, block_targets,
    instruction_operands, local_instruction_effects, verify_mir_bubble,
};

#[derive(Clone, Debug, Eq, PartialEq)]
enum Constant {
    Integer(IntegerValue),
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
        fold_constants(function);
        remove_unreachable_blocks(function);
        remove_dead_constants(function);
    }
    for method in &mut bubble.methods {
        fold_constants(&mut method.function);
        remove_unreachable_blocks(&mut method.function);
        remove_dead_constants(&mut method.function);
    }
    verify_mir_bubble(&bubble, arena)?;
    Ok(bubble)
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
        MirInstructionKind::BooleanNot { operand } => {
            MirInstructionKind::BooleanConstant(!boolean(*operand)?)
        }
        MirInstructionKind::BooleanAnd { left, right } => {
            MirInstructionKind::BooleanConstant(boolean(*left)? && boolean(*right)?)
        }
        MirInstructionKind::BooleanOr { left, right } => {
            MirInstructionKind::BooleanConstant(boolean(*left)? || boolean(*right)?)
        }
        MirInstructionKind::CompareEqual { left, right } => {
            MirInstructionKind::BooleanConstant(constants.get(left)? == constants.get(right)?)
        }
        MirInstructionKind::CompareNotEqual { left, right } => {
            MirInstructionKind::BooleanConstant(constants.get(left)? != constants.get(right)?)
        }
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
        _ => return None,
    })
}

fn constant_from_instruction(kind: &MirInstructionKind) -> Option<Constant> {
    Some(match kind {
        MirInstructionKind::IntegerConstant(value) => Constant::Integer(*value),
        MirInstructionKind::BooleanConstant(value) => Constant::Boolean(*value),
        MirInstructionKind::StringConstant(value) => Constant::String(value.clone()),
        _ => return None,
    })
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
        MirTerminator::Missing
        | MirTerminator::Return { .. }
        | MirTerminator::Trap(_)
        | MirTerminator::Panic(_)
        | MirTerminator::ContinueUnwind(_)
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
            MirTerminator::Missing
            | MirTerminator::Trap(_)
            | MirTerminator::Panic(_)
            | MirTerminator::ContinueUnwind(_)
            | MirTerminator::Unreachable => {}
        }
    }
    used
}
