use pop_mir::{MirInstruction, MirInstructionKind};
use pop_runtime_interface::RuntimeOperation;

use crate::lowering::native_runtime_symbol;

pub(crate) fn lower(instruction: &MirInstruction) -> Option<String> {
    let result = format!("%v{}", instruction.result().raw());
    let lines = match instruction.kind() {
        MirInstructionKind::FfiBytesBorrow { bytes, region } => {
            lower_borrow(&result, bytes.raw(), region.raw())
        }
        MirInstructionKind::FfiBytesBorrowLength { bytes: _, region } => {
            vec![format!(
                "{result} = load i64, ptr %ffi_bytes_region_{}_length",
                region.raw()
            )]
        }
        MirInstructionKind::FfiBytesEndBorrow { bytes, region } => {
            lower_end_borrow(&result, bytes.raw(), region.raw())
        }
        _ => return None,
    };
    Some(lines.join("\n"))
}

fn lower_borrow(result: &str, bytes: u32, region: u32) -> Vec<String> {
    let address = format!("%ffi_bytes_region_{region}_address");
    let length = format!("%ffi_bytes_region_{region}_length");
    let token = format!("%ffi_bytes_region_{region}_token");
    vec![
        format!("{address} = alloca i64"),
        format!("{length} = alloca i64"),
        format!("{token} = alloca i64"),
        format!(
            "{result}_token_value = call i64 @{}(i64 %v{bytes}, ptr {address}, ptr {length})",
            native_runtime_symbol(RuntimeOperation::FfiBytesBorrow)
        ),
        format!("store i64 {result}_token_value, ptr {token}"),
        format!("{result}_token_valid = icmp ne i64 {result}_token_value, 0"),
        checked_branch(&format!("{result}_token"), &format!("{result}_token_valid")),
        format!("{result} = load i64, ptr {address}"),
    ]
}

fn lower_end_borrow(result: &str, bytes: u32, region: u32) -> Vec<String> {
    let token = format!("%ffi_bytes_region_{region}_token");
    vec![
        format!("{result}_token_value = load i64, ptr {token}"),
        format!(
            "{result}_status = call i8 @{}(i64 %v{bytes}, i64 {result}_token_value)",
            native_runtime_symbol(RuntimeOperation::FfiBytesEndBorrow)
        ),
        format!("{result}_status_valid = icmp eq i8 {result}_status, 1"),
        checked_branch(
            &format!("{result}_status"),
            &format!("{result}_status_valid"),
        ),
        format!("store i64 0, ptr {token}"),
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
