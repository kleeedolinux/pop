use std::collections::BTreeMap;

use pop_foundation::FieldId;
use pop_mir::{MirFfiLayoutCatalog, MirInstruction, MirInstructionKind};
use pop_runtime_interface::RuntimeOperation;
use pop_types::TypeArena;

use crate::api::LlvmLoweringError;
use crate::ffi_buffer::marshalling;
use crate::lowering::native_runtime_symbol;

pub(crate) fn lower(
    instruction: &MirInstruction,
    types: &TypeArena,
    layouts: &MirFfiLayoutCatalog,
    _field_layout: &BTreeMap<FieldId, u32>,
) -> Result<Option<String>, LlvmLoweringError> {
    let result = format!("%v{}", instruction.result().raw());
    let lines = match instruction.kind() {
        MirInstructionKind::FfiUnsafeLoad { pointer, layout } => {
            let entry = layout_entry(instruction, layouts, *layout)?;
            let mut lines = checked_pointer(&result, "pointer", *pointer, entry.alignment());
            lines.extend(marshalling::unmarshal(
                &result,
                entry,
                layouts,
                types,
                &format!("{result}_pointer"),
            )?);
            lines
        }
        MirInstructionKind::FfiUnsafeStore {
            pointer,
            value,
            layout,
        } => {
            let entry = layout_entry(instruction, layouts, *layout)?;
            let mut lines = checked_pointer(&result, "pointer", *pointer, entry.alignment());
            lines.extend(marshalling::marshal(
                &format!("%v{}", value.raw()),
                entry,
                layouts,
                types,
                &format!("{result}_pointer"),
                &format!("{result}_marshal"),
            )?);
            lines
        }
        MirInstructionKind::FfiUnsafeAdvance {
            pointer,
            elements,
            layout,
            ..
        } => {
            let entry = layout_entry(instruction, layouts, *layout)?;
            lower_advance(&result, *pointer, *elements, entry.size())
        }
        MirInstructionKind::FfiUnsafeCopy {
            source,
            destination,
            count,
            layout,
        } => {
            let entry = layout_entry(instruction, layouts, *layout)?;
            lower_copy(
                &result,
                *source,
                *destination,
                *count,
                entry.size(),
                entry.alignment(),
            )
        }
        MirInstructionKind::FfiUnsafeAddress { pointer, layout } => {
            let _ = layout_entry(instruction, layouts, *layout)?;
            vec![format!("{result} = add i64 %v{}, 0", pointer.raw())]
        }
        MirInstructionKind::FfiUnsafePointerFromAddress { address, layout } => {
            let _ = layout_entry(instruction, layouts, *layout)?;
            vec![format!("{result} = add i64 %v{}, 0", address.raw())]
        }
        _ => return Ok(None),
    };
    Ok(Some(lines.join("\n")))
}

fn layout_entry<'a>(
    _instruction: &MirInstruction,
    layouts: &'a MirFfiLayoutCatalog,
    layout: pop_runtime_interface::FfiAbiLayoutId,
) -> Result<&'a pop_mir::MirFfiLayout, LlvmLoweringError> {
    layouts
        .get(layout)
        .ok_or(LlvmLoweringError::InvalidFfiLayout(layout))
}

fn checked_pointer(
    result: &str,
    name: &str,
    pointer: pop_foundation::ValueId,
    alignment: u64,
) -> Vec<String> {
    let prefix = format!("{result}_{name}");
    let label = prefix.trim_start_matches('%');
    vec![
        format!("{prefix}_nonzero = icmp ne i64 %v{}, 0", pointer.raw()),
        format!(
            "{prefix}_remainder = urem i64 %v{}, {alignment}",
            pointer.raw()
        ),
        format!("{prefix}_aligned = icmp eq i64 {prefix}_remainder, 0"),
        format!("{prefix}_valid = and i1 {prefix}_nonzero, {prefix}_aligned"),
        format!("br i1 {prefix}_valid, label %{label}_checked, label %{label}_trap"),
        format!("{label}_trap:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_checked:"),
        format!("{prefix} = inttoptr i64 %v{} to ptr", pointer.raw()),
    ]
}

fn lower_advance(
    result: &str,
    pointer: pop_foundation::ValueId,
    elements: pop_foundation::ValueId,
    size: u64,
) -> Vec<String> {
    let label = result.trim_start_matches('%');
    vec![
        format!(
            "{result}_offset_pair = call {{ i64, i1 }} @llvm.smul.with.overflow.i64(i64 %v{}, i64 {size})",
            elements.raw()
        ),
        format!("{result}_offset = extractvalue {{ i64, i1 }} {result}_offset_pair, 0"),
        format!("{result}_multiply_overflow = extractvalue {{ i64, i1 }} {result}_offset_pair, 1"),
        format!("{result}_negative = icmp slt i64 {result}_offset, 0"),
        format!(
            "{result}_magnitude_pair = call {{ i64, i1 }} @llvm.ssub.with.overflow.i64(i64 0, i64 {result}_offset)"
        ),
        format!("{result}_magnitude = extractvalue {{ i64, i1 }} {result}_magnitude_pair, 0"),
        format!(
            "{result}_magnitude_overflow = extractvalue {{ i64, i1 }} {result}_magnitude_pair, 1"
        ),
        format!(
            "{result}_add_pair = call {{ i64, i1 }} @llvm.uadd.with.overflow.i64(i64 %v{}, i64 {result}_offset)",
            pointer.raw()
        ),
        format!("{result}_added = extractvalue {{ i64, i1 }} {result}_add_pair, 0"),
        format!("{result}_add_overflow = extractvalue {{ i64, i1 }} {result}_add_pair, 1"),
        format!(
            "{result}_subtract_pair = call {{ i64, i1 }} @llvm.usub.with.overflow.i64(i64 %v{}, i64 {result}_magnitude)",
            pointer.raw()
        ),
        format!("{result}_subtracted = extractvalue {{ i64, i1 }} {result}_subtract_pair, 0"),
        format!(
            "{result}_subtract_overflow = extractvalue {{ i64, i1 }} {result}_subtract_pair, 1"
        ),
        format!(
            "{result}_address = select i1 {result}_negative, i64 {result}_subtracted, i64 {result}_added"
        ),
        format!(
            "{result}_direction_overflow = select i1 {result}_negative, i1 {result}_subtract_overflow, i1 {result}_add_overflow"
        ),
        format!(
            "{result}_negative_overflow = and i1 {result}_negative, {result}_magnitude_overflow"
        ),
        format!(
            "{result}_overflow_a = or i1 {result}_multiply_overflow, {result}_direction_overflow"
        ),
        format!("{result}_overflow = or i1 {result}_overflow_a, {result}_negative_overflow"),
        format!("{result}_nonzero = icmp ne i64 {result}_address, 0"),
        format!("{result}_no_overflow = xor i1 {result}_overflow, true"),
        format!("{result}_valid = and i1 {result}_nonzero, {result}_no_overflow"),
        format!("br i1 {result}_valid, label %{label}_checked, label %{label}_trap"),
        format!("{label}_trap:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_checked:"),
        format!("{result} = add i64 {result}_address, 0"),
    ]
}

fn lower_copy(
    result: &str,
    source: pop_foundation::ValueId,
    destination: pop_foundation::ValueId,
    count: pop_foundation::ValueId,
    size: u64,
    alignment: u64,
) -> Vec<String> {
    let mut lines = checked_pointer(result, "source", source, alignment);
    lines.extend(checked_pointer(
        result,
        "destination",
        destination,
        alignment,
    ));
    let label = result.trim_start_matches('%');
    lines.extend([
        format!("{result}_bytes_pair = call {{ i64, i1 }} @llvm.umul.with.overflow.i64(i64 %v{}, i64 {size})", count.raw()),
        format!("{result}_bytes = extractvalue {{ i64, i1 }} {result}_bytes_pair, 0"),
        format!("{result}_bytes_overflow = extractvalue {{ i64, i1 }} {result}_bytes_pair, 1"),
        format!("{result}_bytes_valid = xor i1 {result}_bytes_overflow, true"),
        format!("br i1 {result}_bytes_valid, label %{label}_copy, label %{label}_bytes_trap"),
        format!("{label}_bytes_trap:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_copy:"),
        format!("call void @llvm.memmove.p0.p0.i64(ptr align {alignment} {result}_destination, ptr align {alignment} {result}_source, i64 {result}_bytes, i1 false)"),
    ]);
    lines
}

fn trap_line() -> String {
    format!(
        "call void @{}()",
        native_runtime_symbol(RuntimeOperation::Trap)
    )
}
