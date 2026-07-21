//! LLVM-private fixed callback thunks derived from verified canonical MIR.
//!
//! The runtime sees only a closed numeric site identity and an opaque context
//! token. It never resolves a callback signature or symbol name dynamically.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{BubbleId, FfiCallbackSiteId, NestedFunctionId, SymbolId, TypeId, ValueId};
use pop_mir::{MirBubble, MirFfiCallbackSignature, MirInstruction, MirInstructionKind};
use pop_runtime_interface::{FfiCallbackLifetime, FfiCallbackThread, RuntimeOperation};
use pop_target::{PointerWidth, TargetSpec};
use pop_types::{SemanticType, TypeArena};

use crate::api::LlvmLoweringError;
use crate::instruction_lowering::{
    is_managed_type, llvm_type, llvm_value_type, lower_mapped_allocation,
};
use crate::lowering::{
    ForeignConversion, PrivateBlock, PrivateFunction, foreign_physical_type, native_runtime_symbol,
    nested_name,
};

pub(crate) const CALLBACK_REGISTRATION_SLOT: u64 = 1;
pub(crate) const CALLBACK_CONTEXT_SLOT: u64 = 2;
pub(crate) const CALLBACK_SITE_SLOT: u64 = 3;
pub(crate) const CALLBACK_CLOSED_SLOT: u64 = 5;
const CALLBACK_CONTRACT_SLOT_START: u64 = 6;
const CALLBACK_OBJECT_BASE_SLOT_COUNT: u32 = 5;
const CALLBACK_SCHEDULER: u32 = 1;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CallbackOpenSite {
    owner: SymbolId,
    function: NestedFunctionId,
    site: FfiCallbackSiteId,
    callback_type: TypeId,
    registration: ValueId,
    lifetime: FfiCallbackLifetime,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CallbackPairUse {
    owner: SymbolId,
    callback: ValueId,
    lifetime: FfiCallbackLifetime,
    signature: MirFfiCallbackSignature,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct CallbackThunk {
    owner: SymbolId,
    function: NestedFunctionId,
    site: FfiCallbackSiteId,
    callback_type: TypeId,
    signature: MirFfiCallbackSignature,
}

/// LLVM-private fixed thunk inventory derived entirely from verified MIR.
///
/// Contract slots are deterministic compile-time identities. A pair operation
/// loads one statically selected slot; no ABI, fingerprint, or symbol is
/// resolved at runtime.
#[derive(Clone, Debug, Default)]
pub(crate) struct CallbackPlan {
    contract_slots: BTreeMap<MirFfiCallbackSignature, u64>,
    thunks: Vec<CallbackThunk>,
}

impl CallbackPlan {
    fn slot(&self, signature: &MirFfiCallbackSignature) -> Option<u64> {
        self.contract_slots.get(signature).copied()
    }

    fn signatures_for_site(
        &self,
        owner: SymbolId,
        site: FfiCallbackSiteId,
        callback_type: TypeId,
    ) -> impl Iterator<Item = (&MirFfiCallbackSignature, u64)> {
        self.thunks
            .iter()
            .filter(move |thunk| {
                thunk.owner == owner && thunk.site == site && thunk.callback_type == callback_type
            })
            .filter_map(|thunk| {
                self.contract_slots
                    .get(&thunk.signature)
                    .map(|slot| (&thunk.signature, *slot))
            })
    }

    fn object_slot_count(&self) -> Result<u32, LlvmLoweringError> {
        u32::try_from(self.contract_slots.len())
            .ok()
            .and_then(|count| CALLBACK_OBJECT_BASE_SLOT_COUNT.checked_add(count))
            .ok_or(LlvmLoweringError::UnsupportedFfiCallbackInventory)
    }
}

pub(crate) fn runtime_site(owner: SymbolId, site: FfiCallbackSiteId) -> u64 {
    (u64::from(owner.raw()) << 32) | u64::from(site.raw())
}

pub(crate) fn thunk_name(
    bubble: pop_foundation::BubbleId,
    owner: SymbolId,
    site: FfiCallbackSiteId,
    contract_slot: u64,
) -> String {
    format!(
        "pop_b{}_ffi_callback_thunk_{}_{}_{}",
        bubble.raw(),
        owner.raw(),
        site.raw(),
        contract_slot
    )
}

pub(crate) fn plan_callbacks(bubble: &MirBubble) -> Result<CallbackPlan, LlvmLoweringError> {
    collect_plan(bubble)
}

pub(crate) fn lower_thunks(
    bubble: &MirBubble,
    plan: &CallbackPlan,
    types: &TypeArena,
    target: &TargetSpec,
) -> Result<Vec<PrivateFunction>, LlvmLoweringError> {
    if !plan.thunks.is_empty() && target.pointer_width() != PointerWidth::Bits64 {
        return Err(LlvmLoweringError::UnsupportedFfiCallbackTarget(
            target.triple().to_owned(),
        ));
    }
    plan.thunks
        .iter()
        .map(|thunk| lower_thunk(bubble, plan, thunk, types, target))
        .collect()
}

pub(crate) fn lower_instruction(
    bubble: BubbleId,
    owner: SymbolId,
    instruction: &MirInstruction,
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    plan: &CallbackPlan,
) -> Result<Option<String>, LlvmLoweringError> {
    let result = format!("%v{}", instruction.result().raw());
    let lines = match instruction.kind() {
        MirInstructionKind::FfiCallbackOpenScoped {
            callback,
            callback_type,
            site,
            ..
        } => lower_open_scoped(
            &result,
            bubble,
            owner,
            *callback,
            *callback_type,
            *site,
            plan,
        )?,
        MirInstructionKind::FfiCallbackOpenOwned {
            callback,
            callback_type,
            site,
            thread,
            success,
            failure,
            ..
        } => lower_open_owned(
            &result,
            bubble,
            owner,
            *callback,
            *callback_type,
            *site,
            *thread,
            success.raw(),
            failure.raw(),
            plan,
        )?,
        MirInstructionKind::CallCallbackPair {
            callback,
            signature,
            owner: body_owner,
            function,
            captures,
            lifetime,
            success,
            failure,
            ..
        } => lower_pair_call(
            &result,
            instruction.result_type(),
            bubble,
            *callback,
            *body_owner,
            *function,
            captures,
            *lifetime,
            success.map(pop_foundation::ResultCaseId::raw),
            failure.map(pop_foundation::ResultCaseId::raw),
            signature,
            value_types,
            types,
            plan,
        )?,
        MirInstructionKind::FfiCallbackCloseScoped { callback, .. } => {
            lower_close_scoped(&result, *callback)
        }
        MirInstructionKind::FfiCallbackCloseOwned {
            callback,
            success,
            failure,
            ..
        } => lower_close_owned(&result, *callback, success.raw(), failure.raw()),
        _ => return Ok(None),
    };
    Ok(Some(lines.join("\n")))
}

fn lower_open_scoped(
    result: &str,
    bubble: BubbleId,
    owner: SymbolId,
    callback: ValueId,
    callback_type: TypeId,
    site: FfiCallbackSiteId,
    plan: &CallbackPlan,
) -> Result<Vec<String>, LlvmLoweringError> {
    let label = result.trim_start_matches('%');
    let mut lines = open_registration(
        result,
        callback,
        owner,
        site,
        FfiCallbackLifetime::CallScoped,
        FfiCallbackThread::CallingThread,
    );
    lines.extend([
        format!("br i1 {result}_open_valid, label %{label}_open_make, label %{label}_open_trap"),
        format!("{label}_open_make:"),
    ]);
    lines.extend(make_registration_object(
        result,
        result,
        bubble,
        owner,
        callback_type,
        site,
        plan,
    )?);
    lines.extend([
        format!("br label %{label}_open_ready"),
        format!("{label}_open_trap:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_open_ready:"),
    ]);
    Ok(lines)
}

#[allow(clippy::too_many_arguments)]
fn lower_open_owned(
    result: &str,
    bubble: BubbleId,
    owner: SymbolId,
    callback: ValueId,
    callback_type: TypeId,
    site: FfiCallbackSiteId,
    thread: FfiCallbackThread,
    success: u32,
    failure: u32,
    plan: &CallbackPlan,
) -> Result<Vec<String>, LlvmLoweringError> {
    let label = result.trim_start_matches('%');
    let registration = format!("{result}_registration_object");
    let mut lines = open_registration(
        result,
        callback,
        owner,
        site,
        FfiCallbackLifetime::Registered,
        thread,
    );
    lines.extend([
        format!(
            "br i1 {result}_open_consistent, label %{label}_open_decide, label %{label}_open_trap"
        ),
        format!("{label}_open_decide:"),
        format!("br i1 {result}_open_valid, label %{label}_open_make, label %{label}_open_failure"),
        format!("{label}_open_make:"),
    ]);
    lines.extend(make_registration_object(
        &registration,
        result,
        bubble,
        owner,
        callback_type,
        site,
        plan,
    )?);
    lines.extend(make_result_object(
        &format!("{result}_success_result"),
        success,
        &registration,
        true,
    ));
    lines.extend([
        format!("br label %{label}_open_ready"),
        format!("{label}_open_failure:"),
    ]);
    lines.extend(make_result_object(
        &format!("{result}_failure_result"),
        failure,
        &pop_types::FFI_CALLBACK_OPEN_ERROR_TYPE_ID.raw().to_string(),
        false,
    ));
    lines.extend([
        format!("br label %{label}_open_ready"),
        format!("{label}_open_trap:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_open_ready:"),
        format!(
            "{result} = phi i64 [ {result}_success_result, %{label}_open_make ], [ {result}_failure_result, %{label}_open_failure ]"
        ),
    ]);
    Ok(lines)
}

fn open_registration(
    result: &str,
    callback: ValueId,
    owner: SymbolId,
    site: FfiCallbackSiteId,
    lifetime: FfiCallbackLifetime,
    thread: FfiCallbackThread,
) -> Vec<String> {
    let site = runtime_site(owner, site);
    vec![
        format!("{result}_context_out = alloca i64"),
        format!("store i64 0, ptr {result}_context_out"),
        format!(
            "{result}_registration = call i64 @{}(i64 %v{}, i64 {site}, i32 {CALLBACK_SCHEDULER}, i8 {}, i8 {}, ptr {result}_context_out)",
            native_runtime_symbol(RuntimeOperation::FfiCallbackOpen),
            callback.raw(),
            lifetime.raw(),
            thread.raw(),
        ),
        format!("{result}_context = load i64, ptr {result}_context_out"),
        format!("{result}_registration_valid = icmp ne i64 {result}_registration, 0"),
        format!("{result}_context_valid = icmp ne i64 {result}_context, 0"),
        format!("{result}_open_valid = and i1 {result}_registration_valid, {result}_context_valid"),
        format!(
            "{result}_open_consistent = icmp eq i1 {result}_registration_valid, {result}_context_valid"
        ),
    ]
}

fn make_registration_object(
    result: &str,
    open: &str,
    bubble: BubbleId,
    owner: SymbolId,
    callback_type: TypeId,
    site: FfiCallbackSiteId,
    plan: &CallbackPlan,
) -> Result<Vec<String>, LlvmLoweringError> {
    let signatures = plan
        .signatures_for_site(owner, site, callback_type)
        .collect::<Vec<_>>();
    if signatures.is_empty() {
        return Err(LlvmLoweringError::InvalidFfiCallbackSite { owner, site });
    }
    let mut lines = Vec::new();
    for (_, slot) in &signatures {
        lines.push(format!(
            "{result}_thunk_{slot} = ptrtoint ptr @{} to i64",
            thunk_name(bubble, owner, site, *slot)
        ));
    }
    lines.extend(lower_mapped_allocation(
        result,
        plan.object_slot_count()?,
        &[],
    ));
    for (slot, value) in [
        (CALLBACK_REGISTRATION_SLOT, format!("{open}_registration")),
        (CALLBACK_CONTEXT_SLOT, format!("{open}_context")),
        (CALLBACK_SITE_SLOT, runtime_site(owner, site).to_string()),
        (CALLBACK_CLOSED_SLOT, "0".to_owned()),
    ] {
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {slot}, i64 {value})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ));
    }
    for (_, slot) in signatures {
        lines.push(format!(
            "call i8 @{}(i64 {result}, i64 {slot}, i64 {result}_thunk_{slot})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ));
    }
    Ok(lines)
}

#[allow(clippy::too_many_arguments)]
fn lower_pair_call(
    result: &str,
    result_type: TypeId,
    bubble: BubbleId,
    callback: ValueId,
    owner: SymbolId,
    function: NestedFunctionId,
    captures: &[pop_mir::MirClosureCapture],
    lifetime: FfiCallbackLifetime,
    success: Option<u32>,
    failure: Option<u32>,
    signature: &MirFfiCallbackSignature,
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
    plan: &CallbackPlan,
) -> Result<Vec<String>, LlvmLoweringError> {
    let label = result.trim_start_matches('%');
    let thunk_slot = plan
        .slot(signature)
        .ok_or(LlvmLoweringError::UnsupportedFfiCallbackSignature { owner, function })?;
    let mut lines =
        load_callback_field(&format!("{result}_closed"), callback, CALLBACK_CLOSED_SLOT);
    lines.push(format!("{result}_is_open = icmp eq i64 {result}_closed, 0"));

    match lifetime {
        FfiCallbackLifetime::CallScoped => {
            lines.extend([
                format!(
                    "br i1 {result}_is_open, label %{label}_pair_call, label %{label}_pair_trap"
                ),
                format!("{label}_pair_call:"),
            ]);
            lines.extend(load_callback_pair(result, callback, thunk_slot));
            lines.push(callback_pair_body_call(
                result,
                result,
                result_type,
                bubble,
                owner,
                function,
                captures,
                value_types,
                types,
            )?);
            lines.extend([
                format!("br label %{label}_pair_ready"),
                format!("{label}_pair_trap:"),
                trap_line(),
                "unreachable".to_owned(),
                format!("{label}_pair_ready:"),
            ]);
        }
        FfiCallbackLifetime::Registered => {
            let (Some(success), Some(failure)) = (success, failure) else {
                return Err(LlvmLoweringError::InvalidType(result_type));
            };
            let payload_type = callback_result_payload(result_type, types)?;
            let body_result = format!("{result}_body_result");
            lines.extend([
                format!(
                    "br i1 {result}_is_open, label %{label}_pair_call, label %{label}_pair_closed"
                ),
                format!("{label}_pair_call:"),
            ]);
            lines.extend(load_callback_pair(result, callback, thunk_slot));
            lines.push(callback_pair_body_call(
                &body_result,
                result,
                payload_type,
                bubble,
                owner,
                function,
                captures,
                value_types,
                types,
            )?);
            lines.extend(make_result_object(
                &format!("{result}_success_result"),
                success,
                &body_result,
                is_managed_type(payload_type, types),
            ));
            lines.extend([
                format!("br label %{label}_pair_ready"),
                format!("{label}_pair_closed:"),
            ]);
            lines.extend(make_result_object(
                &format!("{result}_failure_result"),
                failure,
                &pop_types::FFI_CALLBACK_CLOSED_ERROR_TYPE_ID
                    .raw()
                    .to_string(),
                false,
            ));
            lines.extend([
                format!("br label %{label}_pair_ready"),
                format!("{label}_pair_ready:"),
                format!(
                    "{result} = phi i64 [ {result}_success_result, %{label}_pair_call ], [ {result}_failure_result, %{label}_pair_closed ]"
                ),
            ]);
        }
    }
    Ok(lines)
}

fn callback_pair_body_call(
    result: &str,
    pair: &str,
    result_type: TypeId,
    bubble: BubbleId,
    owner: SymbolId,
    function: NestedFunctionId,
    captures: &[pop_mir::MirClosureCapture],
    value_types: &BTreeMap<ValueId, TypeId>,
    types: &TypeArena,
) -> Result<String, LlvmLoweringError> {
    let mut arguments = captures
        .iter()
        .filter(|capture| !capture.self_reference())
        .map(|capture| {
            llvm_value_type(value_types, capture.value(), types)
                .map(|ty| format!("{ty} %v{}", capture.value().raw()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    arguments.extend([format!("i64 {pair}_thunk"), format!("i64 {pair}_context")]);
    Ok(format!(
        "{result} = call {} @{}({})",
        llvm_type(result_type, types)?,
        nested_name(bubble, owner, function),
        arguments.join(", ")
    ))
}

fn callback_result_payload(
    result_type: TypeId,
    types: &TypeArena,
) -> Result<TypeId, LlvmLoweringError> {
    match types.get(result_type) {
        Some(SemanticType::Builtin { arguments, .. }) if arguments.len() == 2 => Ok(arguments[0]),
        _ => Err(LlvmLoweringError::InvalidType(result_type)),
    }
}

fn load_callback_pair(result: &str, callback: ValueId, thunk_slot: u64) -> Vec<String> {
    let mut lines = load_callback_field(&format!("{result}_thunk"), callback, thunk_slot);
    lines.extend(load_callback_field(
        &format!("{result}_context"),
        callback,
        CALLBACK_CONTEXT_SLOT,
    ));
    lines
}

fn lower_close_scoped(result: &str, callback: ValueId) -> Vec<String> {
    let label = result.trim_start_matches('%');
    let mut lines =
        load_callback_field(&format!("{result}_closed"), callback, CALLBACK_CLOSED_SLOT);
    lines.extend([
        format!("{result}_is_open = icmp eq i64 {result}_closed, 0"),
        format!("br i1 {result}_is_open, label %{label}_close_call, label %{label}_close_trap"),
        format!("{label}_close_call:"),
    ]);
    lines.extend(load_callback_close_fields(result, callback));
    lines.extend([
        callback_close_call(result),
        format!("{result}_close_valid = icmp eq i8 {result}_close_status, 1"),
        format!("br i1 {result}_close_valid, label %{label}_close_mark, label %{label}_close_trap"),
        format!("{label}_close_mark:"),
        store_callback_closed(callback),
        format!("br label %{label}_close_ready"),
        format!("{label}_close_trap:"),
        trap_line(),
        "unreachable".to_owned(),
        format!("{label}_close_ready:"),
    ]);
    lines
}

fn lower_close_owned(result: &str, callback: ValueId, success: u32, failure: u32) -> Vec<String> {
    let label = result.trim_start_matches('%');
    let mut lines =
        load_callback_field(&format!("{result}_closed"), callback, CALLBACK_CLOSED_SLOT);
    lines.extend([
        format!("{result}_already_closed = icmp ne i64 {result}_closed, 0"),
        format!(
            "br i1 {result}_already_closed, label %{label}_close_success, label %{label}_close_call"
        ),
        format!("{label}_close_call:"),
    ]);
    lines.extend(load_callback_close_fields(result, callback));
    lines.extend([
        callback_close_call(result),
        format!("{result}_close_valid = icmp eq i8 {result}_close_status, 1"),
        format!(
            "br i1 {result}_close_valid, label %{label}_close_mark, label %{label}_close_failure"
        ),
        format!("{label}_close_mark:"),
        store_callback_closed(callback),
        format!("br label %{label}_close_success"),
        format!("{label}_close_success:"),
    ]);
    lines.extend(make_result_object(
        &format!("{result}_success_result"),
        success,
        "0",
        false,
    ));
    lines.extend([
        format!("br label %{label}_close_ready"),
        format!("{label}_close_failure:"),
    ]);
    lines.extend(make_result_object(
        &format!("{result}_failure_result"),
        failure,
        &pop_types::FFI_CALLBACK_IN_USE_ERROR_TYPE_ID
            .raw()
            .to_string(),
        false,
    ));
    lines.extend([
        format!("br label %{label}_close_ready"),
        format!("{label}_close_ready:"),
        format!(
            "{result} = phi i64 [ {result}_success_result, %{label}_close_success ], [ {result}_failure_result, %{label}_close_failure ]"
        ),
    ]);
    lines
}

fn load_callback_close_fields(result: &str, callback: ValueId) -> Vec<String> {
    let mut lines = load_callback_field(
        &format!("{result}_registration"),
        callback,
        CALLBACK_REGISTRATION_SLOT,
    );
    lines.extend(load_callback_field(
        &format!("{result}_context"),
        callback,
        CALLBACK_CONTEXT_SLOT,
    ));
    lines.extend(load_callback_field(
        &format!("{result}_site"),
        callback,
        CALLBACK_SITE_SLOT,
    ));
    lines
}

fn callback_close_call(result: &str) -> String {
    format!(
        "{result}_close_status = call i8 @{}(i64 {result}_registration, i64 {result}_context, i64 {result}_site)",
        native_runtime_symbol(RuntimeOperation::FfiCallbackClose)
    )
}

fn load_callback_field(result: &str, callback: ValueId, slot: u64) -> Vec<String> {
    vec![format!(
        "{result} = call i64 @{}(i64 %v{}, i64 {slot})",
        native_runtime_symbol(RuntimeOperation::FieldGet),
        callback.raw()
    )]
}

fn store_callback_closed(callback: ValueId) -> String {
    format!(
        "call i8 @{}(i64 %v{}, i64 {CALLBACK_CLOSED_SLOT}, i64 1)",
        native_runtime_symbol(RuntimeOperation::FieldSet),
        callback.raw()
    )
}

fn make_result_object(result: &str, case: u32, payload: &str, managed: bool) -> Vec<String> {
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

fn trap_line() -> String {
    format!(
        "call void @{}()",
        native_runtime_symbol(RuntimeOperation::Trap)
    )
}

fn collect_plan(bubble: &MirBubble) -> Result<CallbackPlan, LlvmLoweringError> {
    let mut pair_uses = Vec::new();
    for function in bubble.functions() {
        collect_pair_uses(function.symbol(), function.blocks(), &mut pair_uses);
    }
    for method in bubble.methods() {
        collect_pair_uses(
            method.function().symbol(),
            method.function().blocks(),
            &mut pair_uses,
        );
    }
    for nested in bubble.nested_functions() {
        collect_pair_uses(nested.owner(), nested.blocks(), &mut pair_uses);
    }
    let signatures = pair_uses
        .iter()
        .map(|pair| pair.signature.clone())
        .collect::<BTreeSet<_>>();
    let contract_slots = signatures
        .iter()
        .cloned()
        .enumerate()
        .map(|(index, signature)| {
            u64::try_from(index)
                .ok()
                .and_then(|index| CALLBACK_CONTRACT_SLOT_START.checked_add(index))
                .map(|slot| (signature, slot))
                .ok_or(LlvmLoweringError::UnsupportedFfiCallbackInventory)
        })
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    let mut opens = BTreeMap::new();
    for function in bubble.functions() {
        collect_owner_open_sites(function.symbol(), function.blocks(), &mut opens)?;
    }
    for method in bubble.methods() {
        collect_owner_open_sites(
            method.function().symbol(),
            method.function().blocks(),
            &mut opens,
        )?;
    }
    for nested in bubble.nested_functions() {
        collect_owner_open_sites(nested.owner(), nested.blocks(), &mut opens)?;
    }
    let mut thunks = Vec::new();
    for open in opens.into_values() {
        let compatible = pair_uses
            .iter()
            .filter(|pair| {
                pair.signature.callback_type() == open.callback_type
                    && pair.lifetime == open.lifetime
                    && (open.lifetime == FfiCallbackLifetime::Registered
                        || (pair.owner == open.owner && pair.callback == open.registration))
            })
            .map(|pair| pair.signature.clone())
            .collect::<BTreeSet<_>>();
        if compatible.is_empty() {
            return Err(LlvmLoweringError::UnsupportedFfiCallbackSignature {
                owner: open.owner,
                function: open.function,
            });
        }
        thunks.extend(compatible.into_iter().map(|signature| CallbackThunk {
            owner: open.owner,
            function: open.function,
            site: open.site,
            callback_type: open.callback_type,
            signature,
        }));
    }
    Ok(CallbackPlan {
        contract_slots,
        thunks,
    })
}

fn collect_pair_uses(
    owner: SymbolId,
    blocks: &[pop_mir::MirBlock],
    uses: &mut Vec<CallbackPairUse>,
) {
    uses.extend(
        blocks
            .iter()
            .flat_map(pop_mir::MirBlock::instructions)
            .filter_map(|instruction| {
                let MirInstructionKind::CallCallbackPair {
                    callback,
                    signature,
                    lifetime,
                    ..
                } = instruction.kind()
                else {
                    return None;
                };
                Some(CallbackPairUse {
                    owner,
                    callback: *callback,
                    lifetime: *lifetime,
                    signature: signature.clone(),
                })
            }),
    );
}

fn collect_owner_open_sites(
    owner: SymbolId,
    blocks: &[pop_mir::MirBlock],
    sites: &mut BTreeMap<(SymbolId, FfiCallbackSiteId), CallbackOpenSite>,
) -> Result<(), LlvmLoweringError> {
    for instruction in blocks.iter().flat_map(pop_mir::MirBlock::instructions) {
        let (callback_type, function, site, lifetime) = match instruction.kind() {
            MirInstructionKind::FfiCallbackOpenScoped {
                callback_type,
                owner: source_owner,
                function,
                site,
                ..
            } if *source_owner == owner => (
                *callback_type,
                *function,
                *site,
                FfiCallbackLifetime::CallScoped,
            ),
            MirInstructionKind::FfiCallbackOpenOwned {
                callback_type,
                owner: source_owner,
                function,
                site,
                ..
            } if *source_owner == owner => (
                *callback_type,
                *function,
                *site,
                FfiCallbackLifetime::Registered,
            ),
            MirInstructionKind::FfiCallbackOpenScoped { site, .. }
            | MirInstructionKind::FfiCallbackOpenOwned { site, .. } => {
                return Err(LlvmLoweringError::InvalidFfiCallbackSite { owner, site: *site });
            }
            _ => continue,
        };
        let candidate = CallbackOpenSite {
            owner,
            function,
            site,
            callback_type,
            registration: instruction.result(),
            lifetime,
        };
        if sites
            .insert((owner, site), candidate.clone())
            .is_some_and(|old| old != candidate)
        {
            return Err(LlvmLoweringError::InvalidFfiCallbackSite { owner, site });
        }
    }
    Ok(())
}

fn lower_thunk(
    bubble: &MirBubble,
    plan: &CallbackPlan,
    site: &CallbackThunk,
    types: &TypeArena,
    target: &TargetSpec,
) -> Result<PrivateFunction, LlvmLoweringError> {
    debug_assert_eq!(site.callback_type, site.signature.callback_type());
    let callback = bubble
        .nested_functions()
        .iter()
        .find(|nested| nested.owner() == site.owner && nested.function() == site.function)
        .ok_or(LlvmLoweringError::InvalidFfiCallbackSite {
            owner: site.owner,
            site: site.site,
        })?;
    if site.signature.parameter_layouts().len() != callback.parameters().len() {
        return Err(LlvmLoweringError::UnsupportedFfiCallbackSignature {
            owner: site.owner,
            function: site.function,
        });
    }
    let physical_parameters = callback
        .parameters()
        .iter()
        .zip(site.signature.parameter_layouts())
        .map(|(type_id, layout)| {
            foreign_physical_type(*type_id, *layout, types, target, bubble.ffi_layouts())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let physical_result = callback
        .results()
        .first()
        .map(|type_id| {
            foreign_physical_type(
                *type_id,
                site.signature.result_layout(),
                types,
                target,
                bubble.ffi_layouts(),
            )
        })
        .transpose()?;
    let context_index = callback
        .parameters()
        .iter()
        .position(|type_id| is_callback_context(*type_id, types))
        .ok_or(LlvmLoweringError::UnsupportedFfiCallbackSignature {
            owner: site.owner,
            function: site.function,
        })?;
    let mut entry = vec![
        "%callback_environment_out = alloca i64".to_owned(),
        "store i64 0, ptr %callback_environment_out".to_owned(),
        format!("%callback_context = ptrtoint ptr %callback_arg{context_index} to i64"),
        format!(
            "%callback_transition = call i64 @{}(i64 %callback_context, i64 {}, ptr %callback_environment_out)",
            native_runtime_symbol(RuntimeOperation::FfiCallbackEnter),
            runtime_site(site.owner, site.site)
        ),
        "%callback_entered = icmp ne i64 %callback_transition, 0".to_owned(),
    ];
    entry.push("%callback_environment = load i64, ptr %callback_environment_out".to_owned());

    let mut managed = Vec::new();
    let mut arguments = vec!["i64 %callback_environment".to_owned()];
    for (index, ((type_id, physical), internal)) in callback
        .parameters()
        .iter()
        .zip(&physical_parameters)
        .zip(
            callback
                .parameters()
                .iter()
                .map(|type_id| llvm_type(*type_id, types))
                .collect::<Result<Vec<_>, _>>()?,
        )
        .enumerate()
    {
        let source = format!("%callback_arg{index}");
        let converted = format!("%callback_managed_arg{index}");
        let value = match physical.conversion {
            ForeignConversion::Pointer => {
                managed.push(format!("{converted} = ptrtoint ptr {source} to {internal}"));
                converted
            }
            ForeignConversion::SignedInteger if internal != physical.llvm => {
                managed.push(format!(
                    "{converted} = sext {} {source} to {internal}",
                    physical.llvm
                ));
                converted
            }
            ForeignConversion::UnsignedInteger if internal != physical.llvm => {
                managed.push(format!(
                    "{converted} = zext {} {source} to {internal}",
                    physical.llvm
                ));
                converted
            }
            ForeignConversion::Direct
            | ForeignConversion::SignedInteger
            | ForeignConversion::UnsignedInteger => source,
            ForeignConversion::Layout(layout) => {
                let layout = bubble
                    .ffi_layouts()
                    .get(layout)
                    .ok_or(LlvmLoweringError::InvalidFfiLayout(layout))?;
                let storage = format!("{converted}_storage");
                managed.extend([
                    format!(
                        "{storage} = alloca [{} x i8], align {}",
                        layout.size(),
                        layout.alignment()
                    ),
                    format!(
                        "store {} {source}, ptr {storage}, align {}",
                        physical.llvm,
                        layout.alignment()
                    ),
                ]);
                managed.extend(crate::ffi_buffer::marshalling::unmarshal(
                    &converted,
                    layout,
                    bubble.ffi_layouts(),
                    types,
                    &storage,
                )?);
                converted
            }
        };
        arguments.push(format!("{internal} {value}"));
        let _ = type_id;
    }
    let managed_result_type = callback
        .results()
        .first()
        .map(|type_id| llvm_type(*type_id, types))
        .transpose()?;
    let callback_result = managed_result_type
        .as_ref()
        .map_or_else(String::new, |_| "%callback_managed_result = ".to_owned());
    let invocation = format!(
        "{callback_result}invoke {} @{}({}) to label %callback_returned unwind label %callback_panic",
        managed_result_type.as_deref().unwrap_or("void"),
        nested_name(bubble.bubble(), site.owner, site.function),
        arguments.join(", ")
    );

    let mut returned = Vec::new();
    let return_value = match (physical_result.as_ref(), managed_result_type.as_deref()) {
        (Some(physical), Some(internal)) => match physical.conversion {
            ForeignConversion::Pointer => {
                returned.push(format!(
                    "%callback_physical_result = inttoptr {internal} %callback_managed_result to ptr"
                ));
                Some("%callback_physical_result")
            }
            ForeignConversion::SignedInteger | ForeignConversion::UnsignedInteger
                if internal != physical.llvm =>
            {
                returned.push(format!(
                    "%callback_physical_result = trunc {internal} %callback_managed_result to {}",
                    physical.llvm
                ));
                Some("%callback_physical_result")
            }
            ForeignConversion::Direct
            | ForeignConversion::SignedInteger
            | ForeignConversion::UnsignedInteger => Some("%callback_managed_result"),
            ForeignConversion::Layout(layout) => {
                let layout = bubble
                    .ffi_layouts()
                    .get(layout)
                    .ok_or(LlvmLoweringError::InvalidFfiLayout(layout))?;
                returned.extend([
                    format!(
                        "%callback_physical_result_storage = alloca [{} x i8], align {}",
                        layout.size(),
                        layout.alignment()
                    ),
                    format!(
                        "store [{} x i8] zeroinitializer, ptr %callback_physical_result_storage, align {}",
                        layout.size(),
                        layout.alignment()
                    ),
                ]);
                returned.extend(crate::ffi_buffer::marshalling::marshal(
                    "%callback_managed_result",
                    layout,
                    bubble.ffi_layouts(),
                    types,
                    "%callback_physical_result_storage",
                    "%callback_physical_result_marshal",
                )?);
                returned.push(format!(
                    "%callback_physical_result = load {}, ptr %callback_physical_result_storage, align {}",
                    physical.llvm,
                    layout.alignment()
                ));
                Some("%callback_physical_result")
            }
        },
        (None, None) => None,
        _ => {
            return Err(LlvmLoweringError::UnsupportedFfiCallbackSignature {
                owner: site.owner,
                function: site.function,
            });
        }
    };
    returned.extend([
        format!(
            "%callback_left = call i8 @{}(i64 %callback_transition)",
            native_runtime_symbol(RuntimeOperation::FfiCallbackLeave)
        ),
        "%callback_leave_valid = icmp eq i8 %callback_left, 1".to_owned(),
    ]);

    let panic = vec![
        "%callback_panic_value = landingpad { ptr, i32 } cleanup".to_owned(),
        format!(
            "%callback_panic_left = call i8 @{}(i64 %callback_transition)",
            native_runtime_symbol(RuntimeOperation::FfiCallbackLeave)
        ),
    ];
    let trap = format!(
        "call void @{}()\n  unreachable",
        native_runtime_symbol(RuntimeOperation::Trap)
    );
    let returned_terminator =
        "br i1 %callback_leave_valid, label %callback_return, label %callback_trap".to_owned();
    let return_terminator = physical_result.as_ref().map_or_else(
        || "ret void".to_owned(),
        |physical| {
            format!(
                "ret {} {}",
                physical.llvm,
                return_value.expect("verified callback result")
            )
        },
    );
    let contract_slot =
        plan.slot(&site.signature)
            .ok_or(LlvmLoweringError::UnsupportedFfiCallbackSignature {
                owner: site.owner,
                function: site.function,
            })?;
    Ok(PrivateFunction {
        name: thunk_name(bubble.bubble(), site.owner, site.site, contract_slot),
        parameters: physical_parameters
            .iter()
            .enumerate()
            .map(|(index, physical)| format!("{} %callback_arg{index}", physical.llvm))
            .collect(),
        result: physical_result
            .as_ref()
            .map_or_else(|| "void".to_owned(), |physical| physical.llvm.clone()),
        blocks: vec![
            PrivateBlock {
                label: "callback_entry".to_owned(),
                instructions: entry,
                terminator:
                    "br i1 %callback_entered, label %callback_managed, label %callback_trap"
                        .to_owned(),
            },
            PrivateBlock {
                label: "callback_managed".to_owned(),
                instructions: managed,
                terminator: invocation,
            },
            PrivateBlock {
                label: "callback_returned".to_owned(),
                instructions: returned,
                terminator: returned_terminator,
            },
            PrivateBlock {
                label: "callback_return".to_owned(),
                instructions: Vec::new(),
                terminator: return_terminator,
            },
            PrivateBlock {
                label: "callback_panic".to_owned(),
                instructions: panic,
                terminator: trap.clone(),
            },
            PrivateBlock {
                label: "callback_trap".to_owned(),
                instructions: Vec::new(),
                terminator: trap,
            },
        ],
        attributes: vec!["nounwind", "personality ptr @__gcc_personality_v0"],
        internal: true,
    })
}

fn is_callback_context(type_id: TypeId, types: &TypeArena) -> bool {
    let Some(SemanticType::Builtin { definition, .. }) = types.get(type_id) else {
        return false;
    };
    *definition == pop_types::FFI_CALLBACK_CONTEXT_TYPE_ID
}
