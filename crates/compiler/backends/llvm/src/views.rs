//! LLVM-private lowering for compiler-proven non-owning views.
//!
//! Descriptors retain only a managed lender token and checked integer ranges.
//! They never retain an interior payload pointer across a safe point.

use pop_mir::{MirBlock, MirInstruction, MirInstructionKind, MirTerminator, MirViewKind};
use pop_runtime_interface::RuntimeOperation;

use crate::lowering::native_runtime_symbol;

const VIEW_TYPE: &str = "{ i64, i64, i64, i64 }";

pub(crate) fn is_view_type(type_id: pop_foundation::TypeId, types: &pop_types::TypeArena) -> bool {
    matches!(
        types.get(type_id),
        Some(pop_types::SemanticType::Builtin { definition, arguments })
            if arguments.is_empty()
                && matches!(
                    *definition,
                    pop_types::BYTES_VIEW_TYPE_ID | pop_types::TEXT_VIEW_TYPE_ID
                )
    )
}

pub(crate) fn lower(
    instruction: &MirInstruction,
    _view_lenders: &std::collections::BTreeMap<pop_foundation::ValueId, pop_foundation::ValueId>,
) -> Option<String> {
    let result = format!("%v{}", instruction.result().raw());
    Some(match instruction.kind() {
        MirInstructionKind::ViewCreate { kind, lender, .. } => {
            let operation = match kind {
                MirViewKind::Bytes => "pop_rt_bytes_view_lengths",
                MirViewKind::Text => "pop_rt_text_view_lengths",
            };
            format!(
                "{result}_lengths = call {{ i64, i64 }} @{operation}(i64 %v{})\n\
                 {result}_byte_length = extractvalue {{ i64, i64 }} {result}_lengths, 0\n\
                 {result}_scalar_length = extractvalue {{ i64, i64 }} {result}_lengths, 1\n\
                 {result}_lender = insertvalue {VIEW_TYPE} zeroinitializer, i64 %v{}, 0\n\
                 {result}_bytes = insertvalue {VIEW_TYPE} {result}_lender, i64 {result}_byte_length, 2\n\
                 {result} = insertvalue {VIEW_TYPE} {result}_bytes, i64 {result}_scalar_length, 3",
                lender.raw(),
                lender.raw(),
            )
        }
        MirInstructionKind::ViewSlice {
            kind,
            view,
            start,
            length,
            ..
        } => {
            let operation = match kind {
                MirViewKind::Bytes => "pop_rt_bytes_view_slice",
                MirViewKind::Text => "pop_rt_text_view_slice",
            };
            let lender_line = format!(
                "{result}_lender = extractvalue {VIEW_TYPE} %v{}, 0",
                view.raw()
            );
            format!(
                "{lender_line}\n\
                 {result}_parent_offset = extractvalue {VIEW_TYPE} %v{}, 1\n\
                 {result}_parent_bytes = extractvalue {VIEW_TYPE} %v{}, 2\n\
                 {result}_parent_scalars = extractvalue {VIEW_TYPE} %v{}, 3\n\
                 {result}_range = call {{ i1, i64, i64, i64 }} @{operation}(i64 {result}_lender, i64 {result}_parent_offset, i64 {result}_parent_bytes, i64 {result}_parent_scalars, i64 %v{}, i64 %v{})\n\
                 {result}_valid = extractvalue {{ i1, i64, i64, i64 }} {result}_range, 0\n\
                 {}\n\
                 {result}_offset = extractvalue {{ i1, i64, i64, i64 }} {result}_range, 1\n\
                 {result}_byte_length = extractvalue {{ i1, i64, i64, i64 }} {result}_range, 2\n\
                 {result}_scalar_length = extractvalue {{ i1, i64, i64, i64 }} {result}_range, 3\n\
                 {result}_with_lender = insertvalue {VIEW_TYPE} zeroinitializer, i64 {result}_lender, 0\n\
                 {result}_with_offset = insertvalue {VIEW_TYPE} {result}_with_lender, i64 {result}_offset, 1\n\
                 {result}_with_bytes = insertvalue {VIEW_TYPE} {result}_with_offset, i64 {result}_byte_length, 2\n\
                 {result} = insertvalue {VIEW_TYPE} {result}_with_bytes, i64 {result}_scalar_length, 3",
                view.raw(),
                view.raw(),
                view.raw(),
                start.raw(),
                length.raw(),
                checked_branch(&result, &format!("{result}_valid")),
            )
        }
        MirInstructionKind::ViewLength { kind, view } => {
            let slot = match kind {
                MirViewKind::Bytes => 2,
                MirViewKind::Text => 3,
            };
            format!(
                "{result} = extractvalue {VIEW_TYPE} %v{}, {slot}",
                view.raw()
            )
        }
        MirInstructionKind::ViewGetByte { view, index } => {
            let lender_line = format!(
                "{result}_lender = extractvalue {VIEW_TYPE} %v{}, 0",
                view.raw()
            );
            format!(
                "{lender_line}\n\
             {result}_offset = extractvalue {VIEW_TYPE} %v{}, 1\n\
             {result}_length = extractvalue {VIEW_TYPE} %v{}, 2\n\
             {result} = call {{ i1, i8 }} @pop_rt_bytes_view_get(i64 {result}_lender, i64 {result}_offset, i64 {result}_length, i64 %v{})",
                view.raw(),
                view.raw(),
                index.raw(),
            )
        }
        MirInstructionKind::ViewMaterialize { kind, view, .. } => {
            let operation = match kind {
                MirViewKind::Bytes => "pop_rt_bytes_view_materialize",
                MirViewKind::Text => "pop_rt_text_view_materialize",
            };
            let lender_line = format!(
                "{result}_lender = extractvalue {VIEW_TYPE} %v{}, 0",
                view.raw()
            );
            format!(
                "{lender_line}\n\
                 {result}_offset = extractvalue {VIEW_TYPE} %v{}, 1\n\
                 {result}_length = extractvalue {VIEW_TYPE} %v{}, 2\n\
                 {result} = call i64 @{operation}(i64 {result}_lender, i64 {result}_offset, i64 {result}_length)",
                view.raw(),
                view.raw(),
            )
        }
        MirInstructionKind::ViewEnd { .. } => format!("{result} = xor i1 false, false"),
        _ => return None,
    })
}

pub(crate) fn collect_lenders(
    blocks: &[MirBlock],
    value_types: &std::collections::BTreeMap<pop_foundation::ValueId, pop_foundation::TypeId>,
    types: &pop_types::TypeArena,
) -> std::collections::BTreeMap<pop_foundation::ValueId, pop_foundation::ValueId> {
    use std::collections::BTreeMap;
    let mut lenders = BTreeMap::new();
    if let Some(entry) = blocks.first() {
        for argument in entry.arguments() {
            if value_types
                .get(&argument.value())
                .is_some_and(|type_id| is_view_type(*type_id, types))
            {
                lenders.insert(argument.value(), argument.value());
            }
        }
    }
    let mut changed = true;
    while changed {
        changed = false;
        for block in blocks {
            for instruction in block.instructions() {
                let lender = match instruction.kind() {
                    MirInstructionKind::ViewCreate { lender, .. } => Some(*lender),
                    MirInstructionKind::ViewSlice { view, .. } => lenders.get(view).copied(),
                    MirInstructionKind::CallDirect {
                        arguments,
                        view_result: Some(result),
                        ..
                    }
                    | MirInstructionKind::CallReferenced {
                        arguments,
                        view_result: Some(result),
                        ..
                    } => arguments
                        .get(usize::from(result.source_argument()))
                        .map(|source| lenders.get(source).copied().unwrap_or(*source)),
                    _ => None,
                };
                if let Some(lender) = lender
                    && lenders.insert(instruction.result(), lender) != Some(lender)
                {
                    changed = true;
                }
            }
            if let MirTerminator::Branch { target, arguments } = block.terminator()
                && let Some(target) = blocks.iter().find(|block| block.block() == *target)
            {
                for (source, target) in arguments.iter().zip(target.arguments()) {
                    if let Some(lender) = lenders.get(source).copied()
                        && lenders.insert(target.value(), lender) != Some(lender)
                    {
                        changed = true;
                    }
                }
            }
        }
    }
    lenders
}

fn checked_branch(result: &str, condition: &str) -> String {
    let label = result.trim_start_matches('%');
    format!(
        "br i1 {condition}, label %{label}_checked, label %{label}_trap\n{label}_trap:\n  call void @{}()\n  unreachable\n{label}_checked:",
        native_runtime_symbol(RuntimeOperation::Trap)
    )
}
