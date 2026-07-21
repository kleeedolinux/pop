//! Static lowering for ADR 0092 generated codec adapters.
//!
//! The generated LLVM names below are backend-private. Canonical MIR supplies
//! the exact adapter and resolved member identities; this pass never parses a
//! retained descriptor or performs runtime name/schema lookup.

use std::collections::BTreeMap;

use pop_foundation::{FieldId, TypeId};
use pop_mir::{
    MirGeneratedCodecAdapter, MirGeneratedCodecMember, MirGeneratedCodecMemberId, MirInstruction,
    MirInstructionKind,
};
use pop_runtime_interface::RuntimeOperation;
use pop_runtime_native_abi::{CodecEventStatus, CodecEventTag};
use pop_types::{PrimitiveType, SemanticType, TypeArena};

use crate::api::LlvmLoweringError;
use crate::instruction_lowering::{is_managed_type, lower_mapped_allocation};
use crate::lowering::native_runtime_symbol;

const CODEC_ERROR_CAPABILITY_FAILURE: u8 = CodecEventStatus::CapabilityFailure as u8;

struct EventEmitter<'a> {
    prefix: String,
    lines: Vec<String>,
    next: usize,
    capability_root: String,
    failure_status: String,
    failure_label: String,
    active_roots: Vec<String>,
    read_outputs: [String; 6],
    string_literals: &'a BTreeMap<String, String>,
    writing: bool,
}

struct ReadEvent {
    tag: String,
    ordinal: String,
    label: String,
    label_length: String,
    auxiliary: String,
    scalar: String,
}

impl<'a> EventEmitter<'a> {
    fn new(
        prefix: String,
        capability: &str,
        string_literals: &'a BTreeMap<String, String>,
        writing: bool,
    ) -> Self {
        let capability_root = format!("%{prefix}_capability_root");
        let failure_status = format!("%{prefix}_failure_status");
        let failure_label = format!("{prefix}_failure");
        let read_outputs = [
            format!("%{prefix}_read_tag"),
            format!("%{prefix}_read_ordinal"),
            format!("%{prefix}_read_label"),
            format!("%{prefix}_read_label_length"),
            format!("%{prefix}_read_auxiliary"),
            format!("%{prefix}_read_scalar"),
        ];
        let lines = vec![
            format!("{failure_status} = alloca i8"),
            format!("store i8 {CODEC_ERROR_CAPABILITY_FAILURE}, ptr {failure_status}"),
            format!("{} = alloca i8", read_outputs[0]),
            format!("{} = alloca i32", read_outputs[1]),
            format!("{} = alloca ptr", read_outputs[2]),
            format!("{} = alloca i64", read_outputs[3]),
            format!("{} = alloca i64", read_outputs[4]),
            format!("{} = alloca i64", read_outputs[5]),
            format!(
                "{capability_root} = call i64 @{}(i64 {capability})",
                native_runtime_symbol(RuntimeOperation::RetainRoot)
            ),
        ];
        Self {
            prefix,
            lines,
            next: 0,
            capability_root,
            failure_status,
            failure_label,
            active_roots: Vec::new(),
            read_outputs,
            string_literals,
            writing,
        }
    }

    fn retain_root(&mut self, value: &str) -> String {
        let root = format!("%{}_root_{}", self.prefix, self.next);
        self.next += 1;
        self.lines.push(format!(
            "{root} = call i64 @{}(i64 {value})",
            native_runtime_symbol(RuntimeOperation::RetainRoot)
        ));
        self.active_roots.push(root.clone());
        let valid = format!("%{}_root_valid_{}", self.prefix, self.next);
        self.next += 1;
        self.lines.push(format!("{valid} = icmp ne i64 {root}, 0"));
        self.require(&valid, CodecEventStatus::CapabilityFailure);
        root
    }

    fn resolve_root(&mut self, root: &str) -> String {
        let value = format!("%{}_root_value_{}", self.prefix, self.next);
        self.next += 1;
        self.lines.push(format!(
            "{value} = call i64 @{}(i64 {root})",
            native_runtime_symbol(RuntimeOperation::ResolveRoot)
        ));
        let valid = format!("%{}_root_value_valid_{}", self.prefix, self.next);
        self.next += 1;
        self.lines.push(format!("{valid} = icmp ne i64 {value}, 0"));
        self.require(&valid, CodecEventStatus::CapabilityFailure);
        value
    }

    fn handoff_root(&mut self, root: &str) {
        if let Some(index) = self.active_roots.iter().rposition(|found| found == root) {
            self.active_roots.remove(index);
        }
    }

    fn release_root_if_live(&mut self, root: &str) {
        if root == "0" {
            return;
        }
        if let Some(index) = self.active_roots.iter().rposition(|found| found == root) {
            self.active_roots.remove(index);
            self.lines.push(format!(
                "call i8 @{}(i64 {root})",
                native_runtime_symbol(RuntimeOperation::ReleaseRoot)
            ));
        }
    }

    fn reject(&mut self, status: CodecEventStatus) {
        self.abort_pending_write();
        self.append_failure_cleanup("");
        self.lines.extend([
            format!("store i8 {}, ptr {}", status as u8, self.failure_status),
            format!("br label %{}", self.failure_label),
        ]);
    }

    fn abort_pending_write(&mut self) {
        if !self.writing {
            return;
        }
        let index = self.next;
        self.next += 1;
        let capability = format!("%{}_abort_capability_{index}", self.prefix);
        let status = format!("%{}_abort_status_{index}", self.prefix);
        // ABI 1.19 has exactly two codec operations. A valid SequenceStart
        // above the protocol limit is the fail-closed cancellation signal for
        // runtime-owned unpublished writer staging; its status is deliberately
        // ignored so the original typed failure remains authoritative.
        self.lines.extend([
            format!(
                "{capability} = call i64 @{}(i64 {})",
                native_runtime_symbol(RuntimeOperation::ResolveRoot),
                self.capability_root
            ),
            format!(
                "{status} = call i8 @{}(i64 {capability}, i8 {}, i32 0, ptr null, i64 0, i64 65536, i64 0)",
                native_runtime_symbol(RuntimeOperation::CodecWriteEvent),
                CodecEventTag::SequenceStart as u8,
            ),
        ]);
    }

    fn append_failure_cleanup(&mut self, indent: &str) {
        for root in self.active_roots.iter().rev() {
            self.lines.push(format!(
                "{indent}call i8 @{}(i64 {root})",
                native_runtime_symbol(RuntimeOperation::ReleaseRoot)
            ));
        }
        self.lines.push(format!(
            "{indent}call i8 @{}(i64 {})",
            native_runtime_symbol(RuntimeOperation::ReleaseRoot),
            self.capability_root
        ));
    }

    fn store_field(&mut self, owner: &str, slot: usize, value: &str) {
        let status = format!("%{}_field_status_{}", self.prefix, self.next);
        self.next += 1;
        self.lines.push(format!(
            "{status} = call i8 @{}(i64 {owner}, i64 {slot}, i64 {value})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ));
        let valid = format!("%{}_field_valid_{}", self.prefix, self.next);
        self.next += 1;
        self.lines.push(format!("{valid} = icmp ne i8 {status}, 0"));
        self.require(&valid, CodecEventStatus::CapabilityFailure);
    }

    fn label_pointer(&self, label: Option<&str>) -> Result<(&str, usize), LlvmLoweringError> {
        let Some(label) = label else {
            return Ok(("null", 0));
        };
        self.string_literals
            .get(label)
            .map(|symbol| (symbol.as_str(), label.len()))
            .ok_or(LlvmLoweringError::InvalidType(TypeId::from_raw(u32::MAX)))
    }

    fn write_event(
        &mut self,
        tag: CodecEventTag,
        ordinal: &str,
        label: Option<&str>,
        auxiliary: &str,
        scalar: &str,
    ) -> Result<(), LlvmLoweringError> {
        let index = self.next;
        self.next += 1;
        let (label, label_length) = self.label_pointer(label)?;
        let capability = format!("%{}_capability_{index}", self.prefix);
        let status = format!("%{}_status_{index}", self.prefix);
        let ok = format!("%{}_ok_{index}", self.prefix);
        let rejected = format!("{}_event_{index}_rejected", self.prefix);
        let next = format!("{}_event_{index}_accepted", self.prefix);
        self.lines.extend([
            format!(
                "{capability} = call i64 @{}(i64 {})",
                native_runtime_symbol(RuntimeOperation::ResolveRoot),
                self.capability_root
            ),
            format!(
                "{status} = call i8 @{}(i64 {capability}, i8 {}, i32 {ordinal}, ptr {label}, i64 {label_length}, i64 {auxiliary}, i64 {scalar})",
                native_runtime_symbol(RuntimeOperation::CodecWriteEvent),
                tag as u8,
            ),
            format!("{ok} = icmp eq i8 {status}, {}", CodecEventStatus::Ok as u8),
            format!("br i1 {ok}, label %{next}, label %{rejected}"),
            format!("{rejected}:"),
        ]);
        self.append_failure_cleanup("  ");
        self.lines.extend([
            format!("  store i8 {status}, ptr {}", self.failure_status),
            format!("  br label %{}", self.failure_label),
            format!("{next}:"),
        ]);
        Ok(())
    }

    fn require(&mut self, condition: &str, status: CodecEventStatus) {
        let index = self.next;
        self.next += 1;
        let accepted = format!("{}_require_{index}_accepted", self.prefix);
        let rejected = format!("{}_require_{index}_rejected", self.prefix);
        self.lines.extend([
            format!("br i1 {condition}, label %{accepted}, label %{rejected}"),
            format!("{rejected}:"),
        ]);
        self.abort_pending_write();
        self.append_failure_cleanup("  ");
        self.lines.extend([
            format!("  store i8 {}, ptr {}", status as u8, self.failure_status),
            format!("  br label %{}", self.failure_label),
            format!("{accepted}:"),
        ]);
    }

    fn read_event(
        &mut self,
        tag: CodecEventTag,
        ordinal: &str,
        label: Option<&str>,
        auxiliary: &str,
    ) -> Result<ReadEvent, LlvmLoweringError> {
        let event = self.read_actual()?;
        let _ = self.validate_event(&event, tag, ordinal, label, auxiliary)?;
        Ok(event)
    }

    fn read_actual(&mut self) -> Result<ReadEvent, LlvmLoweringError> {
        let index = self.next;
        self.next += 1;
        let [
            tag_output,
            ordinal_output,
            label_output,
            label_length_output,
            auxiliary_output,
            scalar_output,
        ] = self.read_outputs.clone();
        let capability = format!("%{}_capability_{index}", self.prefix);
        let status = format!("%{}_status_{index}", self.prefix);
        let ok = format!("%{}_ok_{index}", self.prefix);
        let rejected = format!("{}_event_{index}_rejected", self.prefix);
        let accepted = format!("{}_event_{index}_read", self.prefix);
        let tag = format!("%{}_actual_tag_{index}", self.prefix);
        let ordinal = format!("%{}_actual_ordinal_{index}", self.prefix);
        let label = format!("%{}_actual_label_{index}", self.prefix);
        let label_length = format!("%{}_actual_label_length_{index}", self.prefix);
        let auxiliary = format!("%{}_actual_auxiliary_{index}", self.prefix);
        let scalar = format!("%{}_actual_scalar_{index}", self.prefix);
        self.lines.extend([
            format!("store i8 0, ptr {tag_output}"),
            format!("store i32 0, ptr {ordinal_output}"),
            format!("store ptr null, ptr {label_output}"),
            format!("store i64 0, ptr {label_length_output}"),
            format!("store i64 0, ptr {auxiliary_output}"),
            format!("store i64 0, ptr {scalar_output}"),
            format!(
                "{capability} = call i64 @{}(i64 {})",
                native_runtime_symbol(RuntimeOperation::ResolveRoot),
                self.capability_root
            ),
            format!(
                "{status} = call i8 @{}(i64 {capability}, ptr {tag_output}, ptr {ordinal_output}, ptr {label_output}, ptr {label_length_output}, ptr {auxiliary_output}, ptr {scalar_output})",
                native_runtime_symbol(RuntimeOperation::CodecReadEvent),
            ),
            format!("{ok} = icmp eq i8 {status}, {}", CodecEventStatus::Ok as u8),
            format!("br i1 {ok}, label %{accepted}, label %{rejected}"),
            format!("{rejected}:"),
        ]);
        self.append_failure_cleanup("  ");
        self.lines.extend([
            format!("  store i8 {status}, ptr {}", self.failure_status),
            format!("  br label %{}", self.failure_label),
            format!("{accepted}:"),
            format!("  {tag} = load i8, ptr {tag_output}"),
            format!("  {ordinal} = load i32, ptr {ordinal_output}"),
            format!("  {label} = load ptr, ptr {label_output}"),
            format!("  {label_length} = load i64, ptr {label_length_output}"),
            format!("  {auxiliary} = load i64, ptr {auxiliary_output}"),
            format!("  {scalar} = load i64, ptr {scalar_output}"),
        ]);
        Ok(ReadEvent {
            tag,
            ordinal,
            label,
            label_length,
            auxiliary,
            scalar,
        })
    }

    fn validate_event(
        &mut self,
        event: &ReadEvent,
        tag: CodecEventTag,
        ordinal: &str,
        label: Option<&str>,
        auxiliary: &str,
    ) -> Result<String, LlvmLoweringError> {
        let index = self.next;
        self.next += 1;
        let (expected_label, expected_label_length) = self.label_pointer(label)?;
        let expected_label = expected_label.to_owned();
        let malformed = format!("{}_validation_{index}_malformed", self.prefix);
        let next = format!("{}_validation_{index}_accepted", self.prefix);
        self.lines.extend([
            format!("%{}_tag_matches_{index} = icmp eq i8 {}, {}", self.prefix, event.tag, tag as u8),
            format!("%{}_ordinal_matches_{index} = icmp eq i32 {}, {ordinal}", self.prefix, event.ordinal),
            format!("%{}_label_length_matches_{index} = icmp eq i64 {}, {expected_label_length}", self.prefix, event.label_length),
            format!("%{}_auxiliary_matches_{index} = icmp eq i64 {}, {auxiliary}", self.prefix, event.auxiliary),
            format!("%{}_metadata_tag_ordinal_{index} = and i1 %{}_tag_matches_{index}, %{}_ordinal_matches_{index}", self.prefix, self.prefix, self.prefix),
            format!("%{}_metadata_left_{index} = and i1 %{}_metadata_tag_ordinal_{index}, %{}_label_length_matches_{index}", self.prefix, self.prefix, self.prefix),
            format!("%{}_metadata_matches_{index} = and i1 %{}_metadata_left_{index}, %{}_auxiliary_matches_{index}", self.prefix, self.prefix, self.prefix),
        ]);
        let mut metadata_matches = format!("%{}_metadata_matches_{index}", self.prefix);
        if (tag as u8) <= CodecEventTag::OptionalPresent as u8 {
            let exact = format!("%{}_structural_matches_{index}", self.prefix);
            self.lines.extend([
                format!(
                    "%{}_structural_scalar_zero_{index} = icmp eq i64 {}, 0",
                    self.prefix, event.scalar
                ),
                format!(
                    "{exact} = and i1 {metadata_matches}, %{}_structural_scalar_zero_{index}",
                    self.prefix
                ),
            ]);
            metadata_matches = exact;
        }
        if expected_label_length == 0 {
            self.lines.push(format!(
                "br i1 {metadata_matches}, label %{next}, label %{malformed}"
            ));
        } else {
            let compare = format!("{}_validation_{index}_compare_label", self.prefix);
            self.lines.extend([
                format!(
                    "br i1 {metadata_matches}, label %{compare}, label %{malformed}"
                ),
                format!("{compare}:"),
                format!("  %{}_label_order_{index} = call i32 @memcmp(ptr {}, ptr {expected_label}, i64 {expected_label_length})", self.prefix, event.label),
                format!("  %{}_label_matches_{index} = icmp eq i32 %{}_label_order_{index}, 0", self.prefix, self.prefix),
                format!("  br i1 %{}_label_matches_{index}, label %{next}, label %{malformed}", self.prefix),
            ]);
        }
        self.lines.push(format!("{malformed}:"));
        self.append_failure_cleanup("  ");
        self.lines.extend([
            format!(
                "  store i8 {}, ptr {}",
                CodecEventStatus::MalformedInput as u8,
                self.failure_status
            ),
            format!("  br label %{}", self.failure_label),
            format!("{next}:"),
        ]);
        Ok(next)
    }

    fn validate_event_metadata(
        &mut self,
        event: &ReadEvent,
        tag: CodecEventTag,
        ordinal: &str,
        label: Option<&str>,
    ) -> Result<String, LlvmLoweringError> {
        let auxiliary = event.auxiliary.clone();
        self.validate_event(event, tag, ordinal, label, &auxiliary)
    }
}

pub(crate) fn lower_instruction(
    instruction: &MirInstruction,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    string_literals: &BTreeMap<String, String>,
) -> Result<Option<String>, LlvmLoweringError> {
    match instruction.kind() {
        MirInstructionKind::CodecEncode {
            adapter,
            value,
            writer,
            success,
            failure,
            ..
        } => {
            let adapter = adapters
                .iter()
                .find(|candidate| candidate.symbol() == *adapter)
                .ok_or(LlvmLoweringError::InvalidType(instruction.result_type()))?;
            lower_encode(
                instruction,
                adapter,
                adapters,
                &format!("%v{}", value.raw()),
                &format!("%v{}", writer.raw()),
                success.raw(),
                failure.raw(),
                types,
                field_layout,
                string_literals,
            )
            .map(Some)
        }
        MirInstructionKind::CodecDecode {
            adapter,
            reader,
            success,
            failure,
            ..
        } => {
            let adapter = adapters
                .iter()
                .find(|candidate| candidate.symbol() == *adapter)
                .ok_or(LlvmLoweringError::InvalidType(instruction.result_type()))?;
            lower_decode(
                instruction,
                adapter,
                adapters,
                &format!("%v{}", reader.raw()),
                success.raw(),
                failure.raw(),
                types,
                field_layout,
                string_literals,
            )
            .map(Some)
        }
        _ => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn lower_encode(
    instruction: &MirInstruction,
    adapter: &MirGeneratedCodecAdapter,
    adapters: &[MirGeneratedCodecAdapter],
    value: &str,
    writer: &str,
    success: u32,
    failure: u32,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    string_literals: &BTreeMap<String, String>,
) -> Result<String, LlvmLoweringError> {
    let prefix = format!("v{}_codec_encode", instruction.result().raw());
    let mut emitter = EventEmitter::new(prefix.clone(), writer, string_literals, true);
    let value_root =
        codec_slot_is_managed(adapter.target_type(), types).then(|| emitter.retain_root(value));
    encode_adapter(
        &mut emitter,
        adapter,
        adapters,
        value,
        value_root.as_deref(),
        false,
        types,
        field_layout,
    )?;
    finish_codec_result(emitter, instruction, success, failure, None)
}

#[allow(clippy::too_many_arguments)]
fn encode_adapter(
    emitter: &mut EventEmitter<'_>,
    adapter: &MirGeneratedCodecAdapter,
    adapters: &[MirGeneratedCodecAdapter],
    value: &str,
    value_root: Option<&str>,
    value_is_slot: bool,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<(), LlvmLoweringError> {
    match adapter
        .members()
        .first()
        .map(MirGeneratedCodecMember::member)
    {
        Some(MirGeneratedCodecMemberId::Field(_)) | None => {
            let value_root =
                value_root.ok_or(LlvmLoweringError::InvalidType(adapter.target_type()))?;
            emitter.write_event(
                CodecEventTag::RecordStart,
                "0",
                None,
                &adapter.members().len().to_string(),
                "0",
            )?;
            for member in adapter.members() {
                let MirGeneratedCodecMemberId::Field(field) = member.member() else {
                    return Err(LlvmLoweringError::InvalidType(adapter.target_type()));
                };
                let member_type = member
                    .types()
                    .first()
                    .copied()
                    .ok_or(LlvmLoweringError::InvalidType(adapter.target_type()))?;
                emitter.write_event(
                    CodecEventTag::Member,
                    &u32::from(member.ordinal()).to_string(),
                    Some(member.name()),
                    "0",
                    "0",
                )?;
                let current = format!("%{}_owner_{}", emitter.prefix, emitter.next);
                let member_value = format!("%{}_member_{}", emitter.prefix, emitter.next);
                let slot = field_layout
                    .get(&field)
                    .copied()
                    .ok_or(LlvmLoweringError::InvalidFieldLayout(field))?;
                emitter.lines.extend([
                    format!(
                        "{current} = call i64 @{}(i64 {value_root})",
                        native_runtime_symbol(RuntimeOperation::ResolveRoot)
                    ),
                    format!(
                        "{member_value} = call i64 @{}(i64 {current}, i64 {slot})",
                        native_runtime_symbol(RuntimeOperation::FieldGet)
                    ),
                ]);
                encode_type(
                    emitter,
                    member_type,
                    &member_value,
                    adapters,
                    types,
                    field_layout,
                )?;
            }
            emitter.write_event(CodecEventTag::RecordEnd, "0", None, "0", "0")
        }
        Some(MirGeneratedCodecMemberId::EnumCase(_)) => {
            let enum_value = if value_is_slot {
                let converted = format!("%{}_enum_value_{}", emitter.prefix, emitter.next);
                emitter
                    .lines
                    .push(format!("{converted} = trunc i64 {value} to i32"));
                converted
            } else {
                value.to_owned()
            };
            let merge = format!("{}_enum_merge_{}", emitter.prefix, emitter.next);
            let invalid = format!("{}_enum_invalid_{}", emitter.prefix, emitter.next);
            let cases = adapter
                .members()
                .iter()
                .map(|member| {
                    let discriminant = member
                        .discriminant()
                        .ok_or(LlvmLoweringError::InvalidType(adapter.target_type()))?;
                    Ok((
                        member,
                        discriminant,
                        format!(
                            "{}_enum_case_{}_{}",
                            emitter.prefix,
                            emitter.next,
                            member.ordinal()
                        ),
                    ))
                })
                .collect::<Result<Vec<_>, LlvmLoweringError>>()?;
            emitter.lines.push(format!(
                "switch i32 {enum_value}, label %{invalid} [ {} ]",
                cases
                    .iter()
                    .map(|(_, discriminant, label)| format!("i32 {discriminant}, label %{label}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            for (member, discriminant, label) in cases {
                emitter.lines.push(format!("{label}:"));
                emitter.write_event(
                    CodecEventTag::EnumCase,
                    &u32::from(member.ordinal()).to_string(),
                    Some(member.name()),
                    &discriminant.to_string(),
                    "0",
                )?;
                emitter.lines.push(format!("br label %{merge}"));
            }
            emitter.lines.push(format!("{invalid}:"));
            emitter.reject(CodecEventStatus::CapabilityFailure);
            emitter.lines.push(format!("{merge}:"));
            Ok(())
        }
        Some(MirGeneratedCodecMemberId::UnionCase(_)) => {
            let value_root =
                value_root.ok_or(LlvmLoweringError::InvalidType(adapter.target_type()))?;
            let current = format!("%{}_union_owner_{}", emitter.prefix, emitter.next);
            let tag = format!("%{}_union_tag_{}", emitter.prefix, emitter.next);
            emitter.lines.extend([
                format!(
                    "{current} = call i64 @{}(i64 {value_root})",
                    native_runtime_symbol(RuntimeOperation::ResolveRoot)
                ),
                format!(
                    "{tag} = call i64 @{}(i64 {current}, i64 1)",
                    native_runtime_symbol(RuntimeOperation::FieldGet)
                ),
            ]);
            let merge = format!("{}_union_merge_{}", emitter.prefix, emitter.next);
            let invalid = format!("{}_union_invalid_{}", emitter.prefix, emitter.next);
            let cases = adapter
                .members()
                .iter()
                .map(|member| {
                    let MirGeneratedCodecMemberId::UnionCase(case) = member.member() else {
                        return Err(LlvmLoweringError::InvalidType(adapter.target_type()));
                    };
                    Ok((
                        member,
                        case.raw(),
                        format!(
                            "{}_union_case_{}_{}",
                            emitter.prefix,
                            emitter.next,
                            member.ordinal()
                        ),
                    ))
                })
                .collect::<Result<Vec<_>, LlvmLoweringError>>()?;
            emitter.lines.push(format!(
                "switch i64 {tag}, label %{invalid} [ {} ]",
                cases
                    .iter()
                    .map(|(_, case, label)| format!("i64 {case}, label %{label}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            ));
            for (member, _, label) in cases {
                emitter.lines.push(format!("{label}:"));
                emitter.write_event(
                    CodecEventTag::UnionStart,
                    &u32::from(member.ordinal()).to_string(),
                    Some(member.name()),
                    &member.types().len().to_string(),
                    "0",
                )?;
                for (payload_index, payload_type) in member.types().iter().enumerate() {
                    emitter.write_event(
                        CodecEventTag::Payload,
                        &payload_index.to_string(),
                        None,
                        "0",
                        "0",
                    )?;
                    let owner = format!("%{}_union_payload_owner_{}", emitter.prefix, emitter.next);
                    let payload = format!("%{}_union_payload_{}", emitter.prefix, emitter.next);
                    emitter.lines.extend([
                        format!(
                            "{owner} = call i64 @{}(i64 {value_root})",
                            native_runtime_symbol(RuntimeOperation::ResolveRoot)
                        ),
                        format!(
                            "{payload} = call i64 @{}(i64 {owner}, i64 {})",
                            native_runtime_symbol(RuntimeOperation::FieldGet),
                            payload_index + 2
                        ),
                    ]);
                    encode_type(
                        emitter,
                        *payload_type,
                        &payload,
                        adapters,
                        types,
                        field_layout,
                    )?;
                }
                emitter.write_event(CodecEventTag::UnionEnd, "0", None, "0", "0")?;
                emitter.lines.push(format!("br label %{merge}"));
            }
            emitter.lines.push(format!("{invalid}:"));
            emitter.reject(CodecEventStatus::CapabilityFailure);
            emitter.lines.push(format!("{merge}:"));
            Ok(())
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_type(
    emitter: &mut EventEmitter<'_>,
    type_id: TypeId,
    value: &str,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<(), LlvmLoweringError> {
    if let Ok(tag) = scalar_tag(type_id, types) {
        if matches!(tag, CodecEventTag::String | CodecEventTag::Bytes) {
            let root = emitter.retain_root(value);
            let current = emitter.resolve_root(&root);
            emitter.write_event(tag, "0", None, "0", &current)?;
            emitter.release_root_if_live(&root);
            return Ok(());
        }
        return emitter.write_event(tag, "0", None, "0", value);
    }
    if let Some(adapter) = adapters
        .iter()
        .find(|adapter| adapter.target_type() == type_id)
    {
        let root = codec_slot_is_managed(type_id, types).then(|| emitter.retain_root(value));
        encode_adapter(
            emitter,
            adapter,
            adapters,
            value,
            root.as_deref(),
            true,
            types,
            field_layout,
        )?;
        if let Some(root) = root {
            emitter.release_root_if_live(&root);
        }
        return Ok(());
    }
    match types.get(type_id) {
        Some(SemanticType::Tuple(elements)) => {
            let root = emitter.retain_root(value);
            emitter.write_event(
                CodecEventTag::TupleStart,
                "0",
                None,
                &elements.len().to_string(),
                "0",
            )?;
            for (index, element) in elements.iter().enumerate() {
                emitter.write_event(CodecEventTag::Element, &index.to_string(), None, "0", "0")?;
                let owner = format!("%{}_tuple_owner_{}", emitter.prefix, emitter.next);
                let element_value = format!("%{}_tuple_element_{}", emitter.prefix, emitter.next);
                emitter.lines.extend([
                    format!(
                        "{owner} = call i64 @{}(i64 {root})",
                        native_runtime_symbol(RuntimeOperation::ResolveRoot)
                    ),
                    format!(
                        "{element_value} = call i64 @{}(i64 {owner}, i64 {})",
                        native_runtime_symbol(RuntimeOperation::FieldGet),
                        index + 1
                    ),
                ]);
                encode_type(
                    emitter,
                    *element,
                    &element_value,
                    adapters,
                    types,
                    field_layout,
                )?;
            }
            emitter.write_event(CodecEventTag::TupleEnd, "0", None, "0", "0")?;
            emitter.release_root_if_live(&root);
            Ok(())
        }
        Some(SemanticType::Array(element)) => encode_sequence(
            emitter,
            *element,
            value,
            false,
            adapters,
            types,
            field_layout,
        ),
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) if definition.raw() == 101 && arguments.len() == 1 => encode_sequence(
            emitter,
            arguments[0],
            value,
            true,
            adapters,
            types,
            field_layout,
        ),
        Some(SemanticType::Optional(inner)) => {
            encode_optional(emitter, *inner, value, adapters, types, field_layout)
        }
        Some(SemanticType::Union(members)) => {
            let inner =
                optional_payload(members, types).ok_or(LlvmLoweringError::InvalidType(type_id))?;
            encode_optional(emitter, inner, value, adapters, types, field_layout)
        }
        _ => Err(LlvmLoweringError::InvalidType(type_id)),
    }
}

#[allow(clippy::too_many_arguments)]
fn encode_optional(
    emitter: &mut EventEmitter<'_>,
    inner: TypeId,
    value: &str,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<(), LlvmLoweringError> {
    let root = emitter.retain_root(value);
    let current = emitter.resolve_root(&root);
    let tag = format!("%{}_optional_tag_{}", emitter.prefix, emitter.next);
    emitter.lines.push(format!(
        "{tag} = call i64 @{}(i64 {current}, i64 1)",
        native_runtime_symbol(RuntimeOperation::FieldGet)
    ));
    let id = emitter.next;
    emitter.next += 1;
    let absent = format!("{}_optional_{id}_absent", emitter.prefix);
    let present = format!("{}_optional_{id}_present", emitter.prefix);
    let invalid = format!("{}_optional_{id}_invalid", emitter.prefix);
    let merge = format!("{}_optional_{id}_merge", emitter.prefix);
    emitter.lines.push(format!(
        "switch i64 {tag}, label %{invalid} [ i64 0, label %{absent} i64 1, label %{present} ]"
    ));
    emitter.lines.push(format!("{absent}:"));
    emitter.write_event(CodecEventTag::OptionalAbsent, "0", None, "0", "0")?;
    emitter.lines.push(format!("br label %{merge}"));
    emitter.lines.push(format!("{present}:"));
    emitter.write_event(CodecEventTag::OptionalPresent, "0", None, "0", "0")?;
    let owner = emitter.resolve_root(&root);
    let payload = format!("%{}_optional_payload_{}", emitter.prefix, emitter.next);
    emitter.lines.push(format!(
        "{payload} = call i64 @{}(i64 {owner}, i64 2)",
        native_runtime_symbol(RuntimeOperation::FieldGet)
    ));
    encode_type(emitter, inner, &payload, adapters, types, field_layout)?;
    emitter.lines.push(format!("br label %{merge}"));
    emitter.lines.push(format!("{invalid}:"));
    emitter.reject(CodecEventStatus::CapabilityFailure);
    emitter.lines.push(format!("{merge}:"));
    emitter.release_root_if_live(&root);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn encode_sequence(
    emitter: &mut EventEmitter<'_>,
    element: TypeId,
    value: &str,
    list: bool,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
) -> Result<(), LlvmLoweringError> {
    let root = emitter.retain_root(value);
    let owner = format!("%{}_sequence_owner_{}", emitter.prefix, emitter.next);
    let length_output = format!(
        "%{}_sequence_length_output_{}",
        emitter.prefix, emitter.next
    );
    let length_status = format!(
        "%{}_sequence_length_status_{}",
        emitter.prefix, emitter.next
    );
    let length_ok = format!("%{}_sequence_length_ok_{}", emitter.prefix, emitter.next);
    let length = format!("%{}_sequence_length_{}", emitter.prefix, emitter.next);
    emitter.lines.extend([
        format!("{length_output} = alloca i64"),
        format!(
            "{owner} = call i64 @{}(i64 {root})",
            native_runtime_symbol(RuntimeOperation::ResolveRoot)
        ),
        format!(
            "{length_status} = call i8 @{}(i64 {owner}, ptr {length_output})",
            native_runtime_symbol(if list {
                RuntimeOperation::ListLength
            } else {
                RuntimeOperation::ArrayLength
            })
        ),
        format!("{length_ok} = icmp ne i8 {length_status}, 0"),
    ]);
    emitter.require(&length_ok, CodecEventStatus::CapabilityFailure);
    emitter.lines.extend([
        format!("{length} = load i64, ptr {length_output}"),
        format!(
            "%{}_sequence_within_limit_{} = icmp ule i64 {length}, 65535",
            emitter.prefix, emitter.next
        ),
    ]);
    let within_limit = format!("%{}_sequence_within_limit_{}", emitter.prefix, emitter.next);
    emitter.require(&within_limit, CodecEventStatus::LimitExceeded);
    emitter.write_event(CodecEventTag::SequenceStart, "0", None, &length, "0")?;
    let loop_id = emitter.next;
    let index_slot = format!("%{}_sequence_index_slot_{loop_id}", emitter.prefix);
    let check = format!("{}_sequence_check_{loop_id}", emitter.prefix);
    let body = format!("{}_sequence_body_{loop_id}", emitter.prefix);
    let done = format!("{}_sequence_done_{loop_id}", emitter.prefix);
    emitter.lines.extend([
        format!("{index_slot} = alloca i64"),
        format!("store i64 0, ptr {index_slot}"),
        format!("br label %{check}"),
        format!("{check}:"),
        format!(
            "  %{}_sequence_index_{loop_id} = load i64, ptr {index_slot}",
            emitter.prefix
        ),
        format!(
            "  %{}_sequence_more_{loop_id} = icmp ult i64 %{}_sequence_index_{loop_id}, {length}",
            emitter.prefix, emitter.prefix
        ),
        format!(
            "  br i1 %{}_sequence_more_{loop_id}, label %{body}, label %{done}",
            emitter.prefix
        ),
        format!("{body}:"),
        format!(
            "  %{}_sequence_ordinal_{loop_id} = trunc i64 %{}_sequence_index_{loop_id} to i32",
            emitter.prefix, emitter.prefix
        ),
    ]);
    let index = format!("%{}_sequence_index_{loop_id}", emitter.prefix);
    let ordinal = format!("%{}_sequence_ordinal_{loop_id}", emitter.prefix);
    emitter.write_event(CodecEventTag::Element, &ordinal, None, "0", "0")?;
    let current = format!("%{}_sequence_current_{}", emitter.prefix, emitter.next);
    let output = format!("%{}_sequence_output_{}", emitter.prefix, emitter.next);
    let get_status = format!("%{}_sequence_get_status_{}", emitter.prefix, emitter.next);
    let get_ok = format!("%{}_sequence_get_ok_{}", emitter.prefix, emitter.next);
    let runtime_index = format!(
        "%{}_sequence_runtime_index_{}",
        emitter.prefix, emitter.next
    );
    emitter.lines.extend([
        format!(
            "{current} = call i64 @{}(i64 {root})",
            native_runtime_symbol(RuntimeOperation::ResolveRoot)
        ),
        format!("{output} = alloca i64"),
        format!("{runtime_index} = add nuw i64 {index}, 1"),
        format!(
            "{get_status} = call i8 @{}(i64 {current}, i64 {runtime_index}, ptr {output})",
            native_runtime_symbol(if list {
                RuntimeOperation::ListGet
            } else {
                RuntimeOperation::ArrayGetChecked
            })
        ),
        format!("{get_ok} = icmp ne i8 {get_status}, 0"),
    ]);
    emitter.require(&get_ok, CodecEventStatus::CapabilityFailure);
    let element_value = format!("%{}_sequence_element_{}", emitter.prefix, emitter.next);
    emitter
        .lines
        .push(format!("{element_value} = load i64, ptr {output}"));
    encode_type(
        emitter,
        element,
        &element_value,
        adapters,
        types,
        field_layout,
    )?;
    emitter.lines.extend([
        format!(
            "%{}_sequence_next_{loop_id} = add nuw i64 {index}, 1",
            emitter.prefix
        ),
        format!(
            "store i64 %{}_sequence_next_{loop_id}, ptr {index_slot}",
            emitter.prefix
        ),
        format!("br label %{check}"),
        format!("{done}:"),
    ]);
    emitter.write_event(CodecEventTag::SequenceEnd, "0", None, "0", "0")?;
    emitter.release_root_if_live(&root);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_decode(
    instruction: &MirInstruction,
    adapter: &MirGeneratedCodecAdapter,
    adapters: &[MirGeneratedCodecAdapter],
    reader: &str,
    success: u32,
    failure: u32,
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    string_literals: &BTreeMap<String, String>,
) -> Result<String, LlvmLoweringError> {
    let prefix = format!("v{}_codec_decode", instruction.result().raw());
    let mut emitter = EventEmitter::new(prefix.clone(), reader, string_literals, false);
    let decoded = decode_adapter(&mut emitter, adapter, adapters, types, field_layout, 0)?;
    finish_codec_result(emitter, instruction, success, failure, Some(&decoded))
}

struct DecodedValue {
    /// Runtime slot representation. All generated aggregates and managed values
    /// are handles; narrower scalars and floats retain their exact raw bits.
    slot: String,
    /// A precise root keeping every managed value reachable while a containing
    /// aggregate is constructed. Scalar values use the zero sentinel here.
    root: String,
}

#[allow(clippy::too_many_arguments)]
fn decode_adapter(
    emitter: &mut EventEmitter<'_>,
    adapter: &MirGeneratedCodecAdapter,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    depth: u8,
) -> Result<DecodedValue, LlvmLoweringError> {
    require_codec_depth(adapter.target_type(), depth)?;
    match types.get(adapter.target_type()) {
        Some(SemanticType::Record(_)) => {
            let _ = emitter.read_event(
                CodecEventTag::RecordStart,
                "0",
                None,
                &adapter.members().len().to_string(),
            )?;
            let reference_slots = adapter
                .members()
                .iter()
                .filter_map(|member| {
                    let MirGeneratedCodecMemberId::Field(field) = member.member() else {
                        return None;
                    };
                    member
                        .types()
                        .first()
                        .copied()
                        .filter(|type_id| codec_slot_is_managed(*type_id, types))
                        .and_then(|_| field_layout.get(&field).copied())
                        .map(|slot| slot - 1)
                })
                .collect::<Vec<_>>();
            let target = format!("%{}_record_{}", emitter.prefix, emitter.next);
            emitter.lines.extend(lower_mapped_allocation(
                &target,
                u32::try_from(adapter.members().len())
                    .map_err(|_| LlvmLoweringError::InvalidType(adapter.target_type()))?,
                &reference_slots,
            ));
            let root = emitter.retain_root(&target);
            for member in adapter.members() {
                let MirGeneratedCodecMemberId::Field(field) = member.member() else {
                    return Err(LlvmLoweringError::InvalidType(adapter.target_type()));
                };
                let member_type = member
                    .types()
                    .first()
                    .copied()
                    .ok_or(LlvmLoweringError::InvalidType(adapter.target_type()))?;
                let _ = emitter.read_event(
                    CodecEventTag::Member,
                    &u32::from(member.ordinal()).to_string(),
                    Some(member.name()),
                    "0",
                )?;
                let decoded = decode_type(
                    emitter,
                    member_type,
                    adapters,
                    types,
                    field_layout,
                    depth + 1,
                )?;
                let current = emitter.resolve_root(&root);
                let slot = field_layout
                    .get(&field)
                    .copied()
                    .ok_or(LlvmLoweringError::InvalidFieldLayout(field))?;
                emitter.store_field(&current, slot as usize, &decoded.slot);
                emitter.release_root_if_live(&decoded.root);
            }
            let _ = emitter.read_event(CodecEventTag::RecordEnd, "0", None, "0")?;
            let slot = emitter.resolve_root(&root);
            Ok(DecodedValue { slot, root })
        }
        Some(SemanticType::Enum { .. }) => decode_enum(emitter, adapter),
        Some(SemanticType::TaggedUnion { arguments, .. }) if arguments.is_empty() => {
            decode_tagged_union(emitter, adapter, adapters, types, field_layout, depth)
        }
        _ => Err(LlvmLoweringError::InvalidType(adapter.target_type())),
    }
}

fn decode_enum(
    emitter: &mut EventEmitter<'_>,
    adapter: &MirGeneratedCodecAdapter,
) -> Result<DecodedValue, LlvmLoweringError> {
    let event = emitter.read_actual()?;
    let id = emitter.next;
    emitter.next += 1;
    let invalid = format!("{}_enum_{id}_invalid", emitter.prefix);
    let merge = format!("{}_enum_{id}_merge", emitter.prefix);
    let result = format!("%{}_enum_{id}", emitter.prefix);
    let cases = adapter
        .members()
        .iter()
        .map(|member| {
            let discriminant = member
                .discriminant()
                .ok_or(LlvmLoweringError::InvalidType(adapter.target_type()))?;
            Ok((
                member,
                discriminant,
                format!("{}_enum_{id}_case_{}", emitter.prefix, member.ordinal()),
            ))
        })
        .collect::<Result<Vec<_>, LlvmLoweringError>>()?;
    let mut incoming = Vec::new();
    emitter.lines.push(format!(
        "switch i32 {}, label %{invalid} [ {} ]",
        event.ordinal,
        cases
            .iter()
            .map(|(member, _, label)| format!(
                "i32 {}, label %{label}",
                u32::from(member.ordinal())
            ))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    for (member, discriminant, label) in &cases {
        emitter.lines.push(format!("{label}:"));
        let _ = emitter.validate_event(
            &event,
            CodecEventTag::EnumCase,
            &u32::from(member.ordinal()).to_string(),
            Some(member.name()),
            &discriminant.to_string(),
        )?;
        let predecessor = format!("{}_enum_{id}_ready_{}", emitter.prefix, member.ordinal());
        emitter.lines.extend([
            format!("br label %{predecessor}"),
            format!("{predecessor}:"),
        ]);
        emitter.lines.push(format!("br label %{merge}"));
        incoming.push((discriminant, predecessor));
    }
    emitter.lines.push(format!("{invalid}:"));
    emitter.reject(CodecEventStatus::MalformedInput);
    emitter.lines.push(format!("{merge}:"));
    emitter.lines.push(format!(
        "{result} = phi i64 {}",
        incoming
            .iter()
            .map(|(discriminant, label)| format!("[ {discriminant}, %{label} ]"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    Ok(DecodedValue {
        slot: result,
        root: "0".to_owned(),
    })
}

#[allow(clippy::too_many_arguments)]
fn decode_tagged_union(
    emitter: &mut EventEmitter<'_>,
    adapter: &MirGeneratedCodecAdapter,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    depth: u8,
) -> Result<DecodedValue, LlvmLoweringError> {
    let event = emitter.read_actual()?;
    let id = emitter.next;
    emitter.next += 1;
    let invalid = format!("{}_union_{id}_invalid", emitter.prefix);
    let merge = format!("{}_union_{id}_merge", emitter.prefix);
    let result = format!("%{}_union_{id}", emitter.prefix);
    let root_result = format!("%{}_union_{id}_root", emitter.prefix);
    let cases = adapter
        .members()
        .iter()
        .map(|member| {
            let MirGeneratedCodecMemberId::UnionCase(case) = member.member() else {
                return Err(LlvmLoweringError::InvalidType(adapter.target_type()));
            };
            Ok((
                member,
                case.raw(),
                format!("{}_union_{id}_case_{}", emitter.prefix, member.ordinal()),
            ))
        })
        .collect::<Result<Vec<_>, LlvmLoweringError>>()?;
    emitter.lines.push(format!(
        "switch i32 {}, label %{invalid} [ {} ]",
        event.ordinal,
        cases
            .iter()
            .map(|(member, _, label)| format!(
                "i32 {}, label %{label}",
                u32::from(member.ordinal())
            ))
            .collect::<Vec<_>>()
            .join(" ")
    ));
    let mut incoming = Vec::new();
    for (member, case, label) in &cases {
        emitter.lines.push(format!("{label}:"));
        emitter.validate_event(
            &event,
            CodecEventTag::UnionStart,
            &u32::from(member.ordinal()).to_string(),
            Some(member.name()),
            &member.types().len().to_string(),
        )?;
        let reference_slots = member
            .types()
            .iter()
            .enumerate()
            .filter_map(|(index, type_id)| {
                codec_slot_is_managed(*type_id, types)
                    .then(|| u32::try_from(index + 1).ok())
                    .flatten()
            })
            .collect::<Vec<_>>();
        let value = format!("%{}_union_{id}_value_{}", emitter.prefix, member.ordinal());
        emitter.lines.extend(lower_mapped_allocation(
            &value,
            u32::try_from(member.types().len() + 1)
                .map_err(|_| LlvmLoweringError::InvalidType(adapter.target_type()))?,
            &reference_slots,
        ));
        let root = emitter.retain_root(&value);
        emitter.store_field(&value, 1, &case.to_string());
        for (ordinal, payload_type) in member.types().iter().enumerate() {
            let _ = emitter.read_event(CodecEventTag::Payload, &ordinal.to_string(), None, "0")?;
            let payload = decode_type(
                emitter,
                *payload_type,
                adapters,
                types,
                field_layout,
                depth + 1,
            )?;
            let current = emitter.resolve_root(&root);
            emitter.store_field(&current, ordinal + 2, &payload.slot);
            emitter.release_root_if_live(&payload.root);
        }
        let union_end = emitter.read_actual()?;
        let _ = emitter.validate_event(&union_end, CodecEventTag::UnionEnd, "0", None, "0")?;
        let predecessor = format!("{}_union_{id}_ready_{}", emitter.prefix, member.ordinal());
        let current = emitter.resolve_root(&root);
        emitter.handoff_root(&root);
        emitter.lines.extend([
            format!("br label %{predecessor}"),
            format!("{predecessor}:"),
        ]);
        incoming.push((current, root, predecessor));
        emitter.lines.push(format!("br label %{merge}"));
    }
    emitter.lines.push(format!("{invalid}:"));
    emitter.reject(CodecEventStatus::MalformedInput);
    emitter.lines.push(format!("{merge}:"));
    emitter.lines.push(format!(
        "{result} = phi i64 {}",
        incoming
            .iter()
            .map(|(value, _, label)| format!("[ {value}, %{label} ]"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    emitter.lines.push(format!(
        "{root_result} = phi i64 {}",
        incoming
            .iter()
            .map(|(_, root, label)| format!("[ {root}, %{label} ]"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    emitter.active_roots.push(root_result.clone());
    Ok(DecodedValue {
        slot: result,
        root: root_result,
    })
}

#[allow(clippy::too_many_arguments)]
fn decode_type(
    emitter: &mut EventEmitter<'_>,
    type_id: TypeId,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    depth: u8,
) -> Result<DecodedValue, LlvmLoweringError> {
    require_codec_depth(type_id, depth)?;
    if let Some(adapter) = adapters
        .iter()
        .find(|adapter| adapter.target_type() == type_id)
    {
        return decode_adapter(emitter, adapter, adapters, types, field_layout, depth);
    }
    match types.get(type_id) {
        Some(SemanticType::Tuple(elements)) => {
            let _ = emitter.read_event(
                CodecEventTag::TupleStart,
                "0",
                None,
                &elements.len().to_string(),
            )?;
            let reference_slots = elements
                .iter()
                .enumerate()
                .filter_map(|(index, type_id)| {
                    codec_slot_is_managed(*type_id, types)
                        .then(|| u32::try_from(index).ok())
                        .flatten()
                })
                .collect::<Vec<_>>();
            let value = format!("%{}_tuple_{}", emitter.prefix, emitter.next);
            emitter.lines.extend(lower_mapped_allocation(
                &value,
                u32::try_from(elements.len())
                    .map_err(|_| LlvmLoweringError::InvalidType(type_id))?,
                &reference_slots,
            ));
            let root = emitter.retain_root(&value);
            for (ordinal, element) in elements.iter().enumerate() {
                let _ =
                    emitter.read_event(CodecEventTag::Element, &ordinal.to_string(), None, "0")?;
                let decoded =
                    decode_type(emitter, *element, adapters, types, field_layout, depth + 1)?;
                let current = emitter.resolve_root(&root);
                emitter.store_field(&current, ordinal + 1, &decoded.slot);
                emitter.release_root_if_live(&decoded.root);
            }
            let _ = emitter.read_event(CodecEventTag::TupleEnd, "0", None, "0")?;
            let slot = emitter.resolve_root(&root);
            Ok(DecodedValue { slot, root })
        }
        Some(SemanticType::Array(element)) => decode_sequence(
            emitter,
            *element,
            false,
            adapters,
            types,
            field_layout,
            depth,
        ),
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) if definition.raw() == 101 && arguments.len() == 1 => decode_sequence(
            emitter,
            arguments[0],
            true,
            adapters,
            types,
            field_layout,
            depth,
        ),
        Some(SemanticType::Optional(inner)) => {
            decode_optional(emitter, *inner, adapters, types, field_layout, depth)
        }
        Some(SemanticType::Union(members)) => {
            let inner =
                optional_payload(members, types).ok_or(LlvmLoweringError::InvalidType(type_id))?;
            decode_optional(emitter, inner, adapters, types, field_layout, depth)
        }
        _ => decode_scalar(emitter, type_id, types),
    }
}

#[allow(clippy::too_many_arguments)]
fn decode_optional(
    emitter: &mut EventEmitter<'_>,
    inner: TypeId,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    depth: u8,
) -> Result<DecodedValue, LlvmLoweringError> {
    let event = emitter.read_actual()?;
    let tag_absent = format!("%{}_optional_absent_{}", emitter.prefix, emitter.next);
    let tag_present = format!("%{}_optional_present_{}", emitter.prefix, emitter.next);
    emitter.lines.extend([
        format!(
            "{tag_absent} = icmp eq i8 {}, {}",
            event.tag,
            CodecEventTag::OptionalAbsent as u8
        ),
        format!(
            "{tag_present} = icmp eq i8 {}, {}",
            event.tag,
            CodecEventTag::OptionalPresent as u8
        ),
    ]);
    let either = format!("%{}_optional_known_{}", emitter.prefix, emitter.next);
    emitter
        .lines
        .push(format!("{either} = or i1 {tag_absent}, {tag_present}"));
    emitter.require(&either, CodecEventStatus::MalformedInput);
    let id = emitter.next;
    emitter.next += 1;
    let absent = format!("{}_optional_{id}_absent", emitter.prefix);
    let present = format!("{}_optional_{id}_present", emitter.prefix);
    let merge = format!("{}_optional_{id}_merge", emitter.prefix);
    let slot = format!("%{}_optional_{id}", emitter.prefix);
    let root = format!("%{}_optional_{id}_root", emitter.prefix);
    emitter.lines.push(format!(
        "br i1 {tag_absent}, label %{absent}, label %{present}"
    ));
    emitter.lines.push(format!("{absent}:"));
    let _ = emitter.validate_event(&event, CodecEventTag::OptionalAbsent, "0", None, "0")?;
    let absent_predecessor = format!("{}_optional_{id}_absent_ready", emitter.prefix);
    let absent_value = format!("%{}_optional_{id}_absent_value", emitter.prefix);
    emitter.lines.extend(lower_mapped_allocation(
        &absent_value,
        2,
        if codec_slot_is_managed(inner, types) {
            &[1]
        } else {
            &[]
        },
    ));
    let absent_root = emitter.retain_root(&absent_value);
    let absent_current = emitter.resolve_root(&absent_root);
    emitter.store_field(&absent_current, 1, "0");
    emitter.handoff_root(&absent_root);
    emitter.lines.extend([
        format!("br label %{absent_predecessor}"),
        format!("{absent_predecessor}:"),
    ]);
    emitter.lines.push(format!("br label %{merge}"));
    emitter.lines.push(format!("{present}:"));
    let _ = emitter.validate_event(&event, CodecEventTag::OptionalPresent, "0", None, "0")?;
    let payload = decode_type(emitter, inner, adapters, types, field_layout, depth + 1)?;
    let present_predecessor = format!("{}_optional_{id}_present_ready", emitter.prefix);
    let present_value = format!("%{}_optional_{id}_present_value", emitter.prefix);
    emitter.lines.extend(lower_mapped_allocation(
        &present_value,
        2,
        if codec_slot_is_managed(inner, types) {
            &[1]
        } else {
            &[]
        },
    ));
    let present_root = emitter.retain_root(&present_value);
    let present_current = emitter.resolve_root(&present_root);
    emitter.store_field(&present_current, 1, "1");
    emitter.store_field(&present_current, 2, &payload.slot);
    emitter.release_root_if_live(&payload.root);
    emitter.handoff_root(&present_root);
    emitter.lines.extend([
        format!("br label %{present_predecessor}"),
        format!("{present_predecessor}:"),
    ]);
    emitter.lines.push(format!("br label %{merge}"));
    emitter.lines.push(format!("{merge}:"));
    emitter.lines.extend([
        format!(
            "{slot} = phi i64 [ {absent_value}, %{absent_predecessor} ], [ {present_value}, %{present_predecessor} ]"
        ),
        format!(
            "{root} = phi i64 [ {absent_root}, %{absent_predecessor} ], [ {present_root}, %{present_predecessor} ]"
        ),
    ]);
    emitter.active_roots.push(root.clone());
    Ok(DecodedValue { slot, root })
}

#[allow(clippy::too_many_arguments)]
fn decode_sequence(
    emitter: &mut EventEmitter<'_>,
    element: TypeId,
    list: bool,
    adapters: &[MirGeneratedCodecAdapter],
    types: &TypeArena,
    field_layout: &BTreeMap<FieldId, u32>,
    depth: u8,
) -> Result<DecodedValue, LlvmLoweringError> {
    let event = emitter.read_actual()?;
    emitter.validate_event_metadata(&event, CodecEventTag::SequenceStart, "0", None)?;
    let count = event.auxiliary;
    let within_limit = format!("%{}_sequence_count_valid_{}", emitter.prefix, emitter.next);
    emitter
        .lines
        .push(format!("{within_limit} = icmp ule i64 {count}, 65535"));
    emitter.require(&within_limit, CodecEventStatus::LimitExceeded);
    let value = format!("%{}_sequence_{}", emitter.prefix, emitter.next);
    if list {
        emitter.lines.push(format!(
            "{value} = call i64 @{}(i64 {count}, i1 {})",
            native_runtime_symbol(RuntimeOperation::ListCreate),
            u8::from(codec_slot_is_managed(element, types))
        ));
    } else {
        emitter.lines.push(format!(
            "{value} = call i64 @{}(i64 {count}, i1 {})",
            native_runtime_symbol(RuntimeOperation::AllocateArray),
            u8::from(codec_slot_is_managed(element, types))
        ));
    }
    let allocated = format!("%{}_sequence_allocated_{}", emitter.prefix, emitter.next);
    emitter
        .lines
        .push(format!("{allocated} = icmp ne i64 {value}, 0"));
    emitter.require(&allocated, CodecEventStatus::CapabilityFailure);
    let root = emitter.retain_root(&value);
    let id = emitter.next;
    emitter.next += 1;
    let index_slot = format!("%{}_sequence_{id}_index_slot", emitter.prefix);
    let check = format!("{}_sequence_{id}_check", emitter.prefix);
    let body = format!("{}_sequence_{id}_body", emitter.prefix);
    let done = format!("{}_sequence_{id}_done", emitter.prefix);
    emitter.lines.extend([
        format!("{index_slot} = alloca i64"),
        format!("store i64 0, ptr {index_slot}"),
        format!("br label %{check}"),
        format!("{check}:"),
        format!(
            "  %{}_sequence_{id}_index = load i64, ptr {index_slot}",
            emitter.prefix
        ),
        format!(
            "  %{}_sequence_{id}_more = icmp ult i64 %{}_sequence_{id}_index, {count}",
            emitter.prefix, emitter.prefix
        ),
        format!(
            "  br i1 %{}_sequence_{id}_more, label %{body}, label %{done}",
            emitter.prefix
        ),
        format!("{body}:"),
        format!(
            "  %{}_sequence_{id}_ordinal = trunc i64 %{}_sequence_{id}_index to i32",
            emitter.prefix, emitter.prefix
        ),
    ]);
    let index = format!("%{}_sequence_{id}_index", emitter.prefix);
    let ordinal = format!("%{}_sequence_{id}_ordinal", emitter.prefix);
    let _ = emitter.read_event(CodecEventTag::Element, &ordinal, None, "0")?;
    let decoded = decode_type(emitter, element, adapters, types, field_layout, depth + 1)?;
    let current = emitter.resolve_root(&root);
    let stored = format!("%{}_sequence_{id}_stored", emitter.prefix);
    if list {
        emitter.lines.push(format!(
            "{stored} = call i8 @{}(i64 {current}, i64 {}, i1 {})",
            native_runtime_symbol(RuntimeOperation::ListAdd),
            decoded.slot,
            u8::from(codec_slot_is_managed(element, types))
        ));
    } else {
        let runtime_index = format!("%{}_sequence_{id}_runtime_index", emitter.prefix);
        emitter
            .lines
            .push(format!("{runtime_index} = add nuw i64 {index}, 1"));
        emitter.lines.push(format!(
            "{stored} = call i8 @{}(i64 {current}, i64 {runtime_index}, i64 {})",
            native_runtime_symbol(RuntimeOperation::ArraySet),
            decoded.slot
        ));
    }
    let stored_ok = format!("%{}_sequence_{id}_stored_ok", emitter.prefix);
    emitter
        .lines
        .push(format!("{stored_ok} = icmp ne i8 {stored}, 0"));
    emitter.require(&stored_ok, CodecEventStatus::CapabilityFailure);
    emitter.release_root_if_live(&decoded.root);
    emitter.lines.extend([
        format!(
            "%{}_sequence_{id}_next = add nuw i64 {index}, 1",
            emitter.prefix
        ),
        format!(
            "store i64 %{}_sequence_{id}_next, ptr {index_slot}",
            emitter.prefix
        ),
        format!("br label %{check}"),
        format!("{done}:"),
    ]);
    let _ = emitter.read_event(CodecEventTag::SequenceEnd, "0", None, "0")?;
    let slot = emitter.resolve_root(&root);
    Ok(DecodedValue { slot, root })
}

fn decode_scalar(
    emitter: &mut EventEmitter<'_>,
    type_id: TypeId,
    types: &TypeArena,
) -> Result<DecodedValue, LlvmLoweringError> {
    let tag = scalar_tag(type_id, types)?;
    let event = emitter.read_event(tag, "0", None, "0")?;
    if tag == CodecEventTag::Boolean {
        let valid = format!("%{}_boolean_valid_{}", emitter.prefix, emitter.next);
        emitter
            .lines
            .push(format!("{valid} = icmp ule i64 {}, 1", event.scalar));
        emitter.require(&valid, CodecEventStatus::MalformedInput);
    }
    let bounded_bits = match types.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) if kind.bit_width() < 64 => {
            Some(kind.bit_width())
        }
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => Some(32),
        _ => None,
    };
    if let Some(bits) = bounded_bits {
        let valid = format!("%{}_scalar_bits_valid_{}", emitter.prefix, emitter.next);
        let maximum = (1_u128 << bits) - 1;
        emitter.lines.push(format!(
            "{valid} = icmp ule i64 {}, {maximum}",
            event.scalar
        ));
        emitter.require(&valid, CodecEventStatus::MalformedInput);
    }
    let managed = matches!(tag, CodecEventTag::String | CodecEventTag::Bytes);
    if managed {
        let valid = format!("%{}_managed_scalar_valid_{}", emitter.prefix, emitter.next);
        emitter
            .lines
            .push(format!("{valid} = icmp ne i64 {}, 0", event.scalar));
        emitter.require(&valid, CodecEventStatus::CapabilityFailure);
    }
    let root = if managed {
        emitter.retain_root(&event.scalar)
    } else {
        "0".to_owned()
    };
    Ok(DecodedValue {
        slot: event.scalar,
        root,
    })
}

fn optional_payload(members: &[TypeId], types: &TypeArena) -> Option<TypeId> {
    let nil = types.source_type("nil")?;
    if members.len() != 2 || !members.contains(&nil) {
        return None;
    }
    members.iter().copied().find(|member| *member != nil)
}

fn codec_slot_is_managed(type_id: TypeId, types: &TypeArena) -> bool {
    is_managed_type(type_id, types)
        || matches!(
            types.get(type_id),
            Some(SemanticType::Record(_) | SemanticType::TaggedUnion { .. })
        )
        || matches!(types.get(type_id), Some(SemanticType::Optional(_)))
        || matches!(
            types.get(type_id),
            Some(SemanticType::Union(members)) if optional_payload(members, types).is_some()
        )
}

fn require_codec_depth(type_id: TypeId, depth: u8) -> Result<(), LlvmLoweringError> {
    if depth > 32 {
        Err(LlvmLoweringError::InvalidType(type_id))
    } else {
        Ok(())
    }
}

fn scalar_tag(type_id: TypeId, types: &TypeArena) -> Result<CodecEventTag, LlvmLoweringError> {
    match types.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Boolean)) => Ok(CodecEventTag::Boolean),
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Ok(match kind {
            pop_types::IntegerKind::Int8 => CodecEventTag::Int8,
            pop_types::IntegerKind::Int16 => CodecEventTag::Int16,
            pop_types::IntegerKind::Int32 => CodecEventTag::Int32,
            pop_types::IntegerKind::Int64 => CodecEventTag::Int64,
            pop_types::IntegerKind::UInt8 => CodecEventTag::UInt8,
            pop_types::IntegerKind::UInt16 => CodecEventTag::UInt16,
            pop_types::IntegerKind::UInt32 => CodecEventTag::UInt32,
            pop_types::IntegerKind::UInt64 => CodecEventTag::UInt64,
        }),
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => Ok(CodecEventTag::Float32),
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => Ok(CodecEventTag::Float64),
        Some(SemanticType::Primitive(PrimitiveType::String)) => Ok(CodecEventTag::String),
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) if *definition == pop_types::BYTES_TYPE_ID && arguments.is_empty() => {
            Ok(CodecEventTag::Bytes)
        }
        _ => Err(LlvmLoweringError::InvalidType(type_id)),
    }
}

fn finish_codec_result(
    mut emitter: EventEmitter<'_>,
    instruction: &MirInstruction,
    success: u32,
    failure: u32,
    decoded: Option<&DecodedValue>,
) -> Result<String, LlvmLoweringError> {
    let prefix = emitter.prefix.clone();
    let success_label = format!("{prefix}_success");
    let success_initialize = format!("{prefix}_success_result_initialize");
    let success_ready = format!("{prefix}_success_result_ready");
    let success_invalid = format!("{prefix}_success_result_invalid");
    let failure_initialize = format!("{prefix}_failure_result_initialize");
    let failure_ready = format!("{prefix}_failure_result_ready");
    let capability_failure = format!("{prefix}_capability_failure_result_recover");
    let merge_label = format!("{prefix}_merge");
    let success_result = format!("%{prefix}_success_result");
    let failure_result = format!("%{prefix}_failure_result");
    let capability_failure_result = format!("%{prefix}_capability_failure_result");
    let final_result = format!("%v{}", instruction.result().raw());
    emitter.lines.push(format!("br label %{success_label}"));
    emitter.lines.push(format!("{success_label}:"));
    let payload_is_managed = decoded.is_some_and(|value| value.root != "0");
    emitter.lines.extend(lower_mapped_allocation(
        &success_result,
        2,
        if payload_is_managed { &[1] } else { &[] },
    ));
    let success_valid = format!("%{prefix}_success_result_valid");
    emitter.lines.extend([
        format!("{success_valid} = icmp ne i64 {success_result}, 0"),
        format!("br i1 {success_valid}, label %{success_initialize}, label %{success_invalid}"),
        format!("{success_initialize}:"),
    ]);
    let success_case_status = format!("%{prefix}_success_result_case_status");
    emitter.lines.push(format!(
        "{success_case_status} = call i8 @{}(i64 {success_result}, i64 1, i64 {success})",
        native_runtime_symbol(RuntimeOperation::FieldSet)
    ));
    let payload = if let Some(decoded) = decoded {
        if decoded.root == "0" {
            decoded.slot.clone()
        } else {
            emitter.resolve_root(&decoded.root)
        }
    } else {
        "0".to_owned()
    };
    let success_payload_status = format!("%{prefix}_success_result_payload_status");
    let success_case_ok = format!("%{prefix}_success_result_case_ok");
    let success_payload_ok = format!("%{prefix}_success_result_payload_ok");
    let success_initialized = format!("%{prefix}_success_result_initialized");
    emitter.lines.extend([
        format!(
            "{success_payload_status} = call i8 @{}(i64 {success_result}, i64 2, i64 {payload})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
        format!("{success_case_ok} = icmp ne i8 {success_case_status}, 0"),
        format!("{success_payload_ok} = icmp ne i8 {success_payload_status}, 0"),
        format!("{success_initialized} = and i1 {success_case_ok}, {success_payload_ok}"),
        format!("br i1 {success_initialized}, label %{success_ready}, label %{success_invalid}"),
        format!("{success_invalid}:"),
    ]);
    for root in emitter.active_roots.iter().rev() {
        emitter.lines.push(format!(
            "call i8 @{}(i64 {root})",
            native_runtime_symbol(RuntimeOperation::ReleaseRoot)
        ));
    }
    emitter.lines.push(format!(
        "call i8 @{}(i64 {})",
        native_runtime_symbol(RuntimeOperation::ReleaseRoot),
        emitter.capability_root
    ));
    emitter.lines.extend([
        format!("br label %{capability_failure}"),
        format!("{success_ready}:"),
    ]);
    for root in emitter.active_roots.iter().rev() {
        emitter.lines.push(format!(
            "call i8 @{}(i64 {root})",
            native_runtime_symbol(RuntimeOperation::ReleaseRoot)
        ));
    }
    emitter.lines.extend([
        format!(
            "call i8 @{}(i64 {})",
            native_runtime_symbol(RuntimeOperation::ReleaseRoot),
            emitter.capability_root
        ),
        format!("br label %{merge_label}"),
    ]);
    emitter.lines.push(format!("{}:", emitter.failure_label));
    let status = format!("%{prefix}_failed_status");
    let status_nonzero = format!("%{prefix}_failed_status_nonzero");
    let status_bounded = format!("%{prefix}_failed_status_bounded");
    let status_valid = format!("%{prefix}_failed_status_valid");
    let status_closed = format!("%{prefix}_failed_status_closed");
    let error = format!("%{prefix}_error");
    emitter.lines.extend([
        format!("{status} = load i8, ptr {}", emitter.failure_status),
        format!("{status_nonzero} = icmp uge i8 {status}, 1"),
        format!("{status_bounded} = icmp ule i8 {status}, 3"),
        format!("{status_valid} = and i1 {status_nonzero}, {status_bounded}"),
        format!(
            "{status_closed} = select i1 {status_valid}, i8 {status}, i8 {}",
            CodecEventStatus::CapabilityFailure as u8
        ),
        format!("{error}_case = sub i8 {status_closed}, 1"),
        format!("{error} = zext i8 {error}_case to i64"),
    ]);
    emitter
        .lines
        .extend(lower_mapped_allocation(&failure_result, 2, &[]));
    let failure_valid = format!("%{prefix}_failure_result_valid");
    emitter.lines.extend([
        format!("{failure_valid} = icmp ne i64 {failure_result}, 0"),
        format!("br i1 {failure_valid}, label %{failure_initialize}, label %{capability_failure}"),
        format!("{failure_initialize}:"),
    ]);
    let failure_case_status = format!("%{prefix}_failure_result_case_status");
    let failure_payload_status = format!("%{prefix}_failure_result_payload_status");
    let failure_case_ok = format!("%{prefix}_failure_result_case_ok");
    let failure_payload_ok = format!("%{prefix}_failure_result_payload_ok");
    let failure_initialized = format!("%{prefix}_failure_result_initialized");
    emitter.lines.extend([
        format!(
            "{failure_case_status} = call i8 @{}(i64 {failure_result}, i64 1, i64 {failure})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
        format!(
            "{failure_payload_status} = call i8 @{}(i64 {failure_result}, i64 2, i64 {error})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
        format!("{failure_case_ok} = icmp ne i8 {failure_case_status}, 0"),
        format!("{failure_payload_ok} = icmp ne i8 {failure_payload_status}, 0"),
        format!("{failure_initialized} = and i1 {failure_case_ok}, {failure_payload_ok}"),
        format!("br i1 {failure_initialized}, label %{failure_ready}, label %{capability_failure}"),
        format!("{failure_ready}:"),
        format!("br label %{merge_label}"),
        format!("{capability_failure}:"),
    ]);
    emitter
        .lines
        .extend(lower_mapped_allocation(&capability_failure_result, 2, &[]));
    let capability_result_valid = format!("%{prefix}_capability_failure_result_valid");
    let capability_case_status = format!("%{prefix}_capability_failure_result_case_status");
    let capability_payload_status = format!("%{prefix}_capability_failure_result_payload_status");
    let capability_case_ok = format!("%{prefix}_capability_failure_result_case_ok");
    let capability_payload_ok = format!("%{prefix}_capability_failure_result_payload_ok");
    let capability_fields_valid = format!("%{prefix}_capability_failure_result_fields_valid");
    let capability_result_complete = format!("%{prefix}_capability_failure_result_complete");
    let capability_result = format!("%{prefix}_capability_failure_result_selected");
    emitter.lines.extend([
        format!(
            "{capability_result_valid} = icmp ne i64 {capability_failure_result}, 0"
        ),
        format!(
            "{capability_case_status} = call i8 @{}(i64 {capability_failure_result}, i64 1, i64 {failure})",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
        format!(
            "{capability_payload_status} = call i8 @{}(i64 {capability_failure_result}, i64 2, i64 2)",
            native_runtime_symbol(RuntimeOperation::FieldSet)
        ),
        format!("{capability_case_ok} = icmp ne i8 {capability_case_status}, 0"),
        format!("{capability_payload_ok} = icmp ne i8 {capability_payload_status}, 0"),
        format!(
            "{capability_fields_valid} = and i1 {capability_case_ok}, {capability_payload_ok}"
        ),
        format!(
            "{capability_result_complete} = and i1 {capability_result_valid}, {capability_fields_valid}"
        ),
        format!(
            "{capability_result} = select i1 {capability_result_complete}, i64 {capability_failure_result}, i64 0"
        ),
        format!("br label %{merge_label}"),
        format!("{merge_label}:"),
        format!(
            "{final_result} = phi i64 [ {success_result}, %{success_ready} ], [ {failure_result}, %{failure_ready} ], [ {capability_result}, %{capability_failure} ]"
        ),
    ]);
    Ok(emitter.lines.join("\n"))
}
