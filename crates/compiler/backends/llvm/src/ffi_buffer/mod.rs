use std::collections::BTreeMap;

use pop_foundation::{FieldId, TypeId, ValueId};
use pop_mir::{MirFfiLayoutCatalog, MirInstruction, MirInstructionKind};
use pop_runtime_interface::RuntimeOperation;
use pop_types::TypeArena;

use crate::api::LlvmLoweringError;
use crate::instruction_lowering::lower_mapped_allocation;
use crate::lowering::native_runtime_symbol;

pub(crate) mod marshalling;

pub(crate) fn lower(
    instruction: &MirInstruction,
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    layouts: &MirFfiLayoutCatalog,
    _field_layout: &BTreeMap<FieldId, u32>,
) -> Result<Option<String>, LlvmLoweringError> {
    let result = format!("%v{}", instruction.result().raw());
    let lines = match instruction.kind() {
        MirInstructionKind::FfiBufferOpen {
            length,
            layout,
            element_size,
            alignment,
            success,
            failure,
            ..
        } => lower_open(
            &result,
            *length,
            layout.raw(),
            *element_size,
            *alignment,
            success.raw(),
            failure.raw(),
        ),
        MirInstructionKind::FfiBufferLength { buffer, layout } => {
            lower_length(&result, *buffer, layout.raw())
        }
        MirInstructionKind::FfiBufferRead {
            buffer,
            index,
            layout,
        } => {
            let entry = layouts
                .get(*layout)
                .ok_or(LlvmLoweringError::InvalidType(instruction.result_type()))?;
            let mut lines = vec![format!(
                "{result}_storage = alloca [{} x i8], align {}",
                entry.size(),
                entry.alignment()
            )];
            lines.extend(status_call(
                &result,
                RuntimeOperation::FfiBufferRead,
                &format!(
                    "i64 %v{}, i64 {}, i64 %v{}, ptr {result}_storage, i64 {}",
                    buffer.raw(),
                    layout.raw(),
                    index.raw(),
                    entry.size()
                ),
            ));
            lines.extend(marshalling::unmarshal(
                &result,
                entry,
                layouts,
                types,
                &format!("{result}_storage"),
            )?);
            lines
        }
        MirInstructionKind::FfiBufferWrite {
            buffer,
            index,
            value,
            layout,
        } => {
            let entry = layouts.get(*layout).ok_or(LlvmLoweringError::InvalidType(
                *value_types
                    .get(value)
                    .unwrap_or(&TypeId::from_raw(u32::MAX)),
            ))?;
            let mut lines = vec![
                format!(
                    "{result}_storage = alloca [{} x i8], align {}",
                    entry.size(),
                    entry.alignment()
                ),
                format!(
                    "store [{} x i8] zeroinitializer, ptr {result}_storage, align {}",
                    entry.size(),
                    entry.alignment()
                ),
            ];
            lines.extend(marshalling::marshal(
                &format!("%v{}", value.raw()),
                entry,
                layouts,
                types,
                &format!("{result}_storage"),
                &format!("{result}_marshal"),
            )?);
            lines.extend(status_call(
                &result,
                RuntimeOperation::FfiBufferWrite,
                &format!(
                    "i64 %v{}, i64 {}, i64 %v{}, ptr {result}_storage, i64 {}",
                    buffer.raw(),
                    layout.raw(),
                    index.raw(),
                    entry.size()
                ),
            ));
            lines
        }
        MirInstructionKind::FfiBufferBorrow {
            buffer,
            expected_length,
            layout,
            region,
        } => lower_borrow(
            &result,
            *buffer,
            *expected_length,
            layout.raw(),
            region.raw(),
        ),
        MirInstructionKind::FfiBufferEndBorrow { buffer, region } => {
            let generation = format!("%ffi_buffer_region_{}_generation", region.raw());
            let mut lines = vec![format!("{result}_generation = load i64, ptr {generation}")];
            lines.extend(status_call(
                &result,
                RuntimeOperation::FfiBufferEndBorrow,
                &format!("i64 %v{}, i64 {result}_generation", buffer.raw()),
            ));
            lines.push(format!("store i64 0, ptr {generation}"));
            lines
        }
        MirInstructionKind::FfiBufferClose { buffer } => status_call(
            &result,
            RuntimeOperation::FfiBufferClose,
            &format!("i64 %v{}", buffer.raw()),
        ),
        _ => return Ok(None),
    };
    Ok(Some(lines.join("\n")))
}

fn lower_open(
    result: &str,
    length: ValueId,
    layout: u64,
    element_size: u64,
    alignment: u64,
    success: u32,
    failure: u32,
) -> Vec<String> {
    let label = result.trim_start_matches('%');
    let mut lines = vec![
        format!("{result}_buffer_out = alloca i64"),
        format!("store i64 0, ptr {result}_buffer_out"),
        format!(
            "{result}_status = call i8 @{}(i64 %v{}, i64 {element_size}, i64 {alignment}, i64 {layout}, ptr {result}_buffer_out)",
            native_runtime_symbol(RuntimeOperation::FfiBufferOpen),
            length.raw()
        ),
        format!(
            "switch i8 {result}_status, label %{label}_unknown [ i8 0, label %{label}_allocation_failure i8 1, label %{label}_success i8 2, label %{label}_invariant ]"
        ),
        format!("{label}_success:"),
        format!("{result}_buffer = load i64, ptr {result}_buffer_out"),
        format!("{result}_buffer_valid = icmp ne i64 {result}_buffer, 0"),
        format!(
            "br i1 {result}_buffer_valid, label %{label}_success_make, label %{label}_invariant"
        ),
        format!("{label}_success_make:"),
    ];
    lines.extend(lower_result_object(
        &format!("{result}_success_result"),
        success,
        &format!("{result}_buffer"),
        true,
    ));
    lines.extend([
        format!("br label %{label}_ready"),
        format!("{label}_allocation_failure:"),
    ]);
    lines.extend(lower_result_object(
        &format!("{result}_failure_result"),
        failure,
        &pop_types::FFI_ALLOCATION_ERROR_TYPE_ID.raw().to_string(),
        false,
    ));
    lines.extend([
        format!("br label %{label}_ready"),
        format!("{label}_unknown:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_invariant:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_ready:"),
        format!(
            "{result} = phi i64 [ {result}_success_result, %{label}_success_make ], [ {result}_failure_result, %{label}_allocation_failure ]"
        ),
    ]);
    lines
}

fn lower_result_object(result: &str, case: u32, payload: &str, managed: bool) -> Vec<String> {
    let reference_slots: &[u32] = if managed { &[1] } else { &[] };
    let mut lines = lower_mapped_allocation(result, 2, reference_slots);
    lines.extend([
        format!(
            "call i8 @{}(i64 {result}, i64 1, i64 {case})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
        format!(
            "call i8 @{}(i64 {result}, i64 2, i64 {payload})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
    ]);
    lines
}

fn lower_length(result: &str, buffer: ValueId, layout: u64) -> Vec<String> {
    let mut lines = vec![format!("{result}_out = alloca i64")];
    lines.extend(status_call(
        result,
        RuntimeOperation::FfiBufferLength,
        &format!("i64 %v{}, i64 {layout}, ptr {result}_out", buffer.raw()),
    ));
    lines.push(format!("{result} = load i64, ptr {result}_out"));
    lines
}

fn lower_borrow(
    result: &str,
    buffer: ValueId,
    expected_length: ValueId,
    layout: u64,
    region: u32,
) -> Vec<String> {
    let pointer = format!("%ffi_buffer_region_{region}_pointer");
    let length = format!("%ffi_buffer_region_{region}_length");
    let generation = format!("%ffi_buffer_region_{region}_generation");
    let mut lines = vec![
        format!("{pointer} = alloca ptr"),
        format!("{length} = alloca i64"),
        format!("{generation} = alloca i64"),
    ];
    lines.extend(status_call(
        result,
        RuntimeOperation::FfiBufferBorrow,
        &format!(
            "i64 %v{}, i64 {layout}, ptr {pointer}, ptr {length}, ptr {generation}",
            buffer.raw()
        ),
    ));
    lines.extend([
        format!("{result}_length = load i64, ptr {length}"),
        format!(
            "{result}_length_valid = icmp eq i64 {result}_length, %v{}",
            expected_length.raw()
        ),
        format!("{result}_generation_value = load i64, ptr {generation}"),
        format!("{result}_generation_valid = icmp ne i64 {result}_generation_value, 0"),
        format!("{result}_borrow_valid = and i1 {result}_length_valid, {result}_generation_valid"),
        checked_branch(
            &format!("{result}_borrow"),
            &format!("{result}_borrow_valid"),
        ),
        format!("{result}_pointer = load ptr, ptr {pointer}"),
        format!("{result} = ptrtoint ptr {result}_pointer to i64"),
    ]);
    lines
}

fn status_call(result: &str, operation: RuntimeOperation, arguments: &str) -> Vec<String> {
    let status = format!("{result}_status");
    let valid = format!("{result}_status_valid");
    vec![
        format!(
            "{status} = call i8 @{}({arguments})",
            native_runtime_symbol(operation)
        ),
        format!("{valid} = icmp eq i8 {status}, 1"),
        checked_branch(&format!("{result}_status"), &valid),
    ]
}

fn checked_branch(result: &str, condition: &str) -> String {
    let label = result.trim_start_matches('%');
    format!(
        "br i1 {condition}, label %{label}_checked, label %{label}_trap\n{label}_trap:\n  {}\n  unreachable\n{label}_checked:",
        trap_line()
    )
}

fn trap_line() -> String {
    format!(
        "call void @{}()",
        native_runtime_symbol(RuntimeOperation::Trap)
    )
}
