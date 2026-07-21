use pop_runtime_native_abi::{CodecEventStatus, CodecEventTag};

use super::capability::{abort_pending, capabilities, capability_key};
use super::model::{
    CODEC_WRITER_RUNTIME_TYPE, CodecCapability, MAX_CODEC_EVENTS, MAX_CODEC_PAYLOAD_BYTES,
    StoredEvent, StoredScalar,
};
use super::scalar::{copy_label, store_scalar};

pub(super) fn write_event(
    capability: u64,
    tag: u8,
    ordinal: u32,
    label: *const u8,
    label_length: u64,
    auxiliary: u64,
    scalar: u64,
) -> CodecEventStatus {
    let Some(capability_key) = capability_key(capability, CODEC_WRITER_RUNTIME_TYPE) else {
        return CodecEventStatus::CapabilityFailure;
    };
    let Some(tag) = CodecEventTag::from_raw(tag) else {
        abort_pending(capability_key);
        return CodecEventStatus::MalformedInput;
    };
    let label = match copy_label(label, label_length) {
        Ok(label) => label,
        Err(status) => {
            abort_pending(capability_key);
            return status;
        }
    };
    let scalar = match store_scalar(tag, scalar) {
        Ok(scalar) => scalar,
        Err(status) => {
            abort_pending(capability_key);
            return status;
        }
    };
    let Ok(mut registered) = capabilities().lock() else {
        return CodecEventStatus::CapabilityFailure;
    };
    let Some(CodecCapability::Writer {
        events,
        pending,
        containers,
    }) = registered
        .get_mut(&capability_key)
        .map(|entry| &mut entry.capability)
    else {
        return CodecEventStatus::CapabilityFailure;
    };
    if events.len().saturating_add(pending.len()) >= MAX_CODEC_EVENTS {
        pending.clear();
        containers.clear();
        return CodecEventStatus::LimitExceeded;
    }
    if is_structural(tag) && !matches!(scalar, StoredScalar::Bits(0)) {
        pending.clear();
        containers.clear();
        return CodecEventStatus::MalformedInput;
    }
    if tag == CodecEventTag::SequenceStart && auxiliary > MAX_CODEC_PAYLOAD_BYTES as u64 {
        pending.clear();
        containers.clear();
        return CodecEventStatus::LimitExceeded;
    }
    if !label.is_empty()
        && !matches!(
            tag,
            CodecEventTag::Member | CodecEventTag::EnumCase | CodecEventTag::UnionStart
        )
    {
        pending.clear();
        containers.clear();
        return CodecEventStatus::MalformedInput;
    }
    let event = StoredEvent {
        tag,
        ordinal,
        label,
        auxiliary,
        scalar,
    };
    if let Some(expected_start) = container_start_for_end(tag) {
        if containers.pop() != Some(expected_start) {
            pending.clear();
            containers.clear();
            return CodecEventStatus::MalformedInput;
        }
        pending.push(event);
        if containers.is_empty() {
            events.append(pending);
        }
        return CodecEventStatus::Ok;
    }
    if pending.is_empty() && !is_container_start(tag) {
        events.push(event);
        return CodecEventStatus::Ok;
    }
    if is_container_start(tag) {
        if containers.len() >= 32 {
            pending.clear();
            containers.clear();
            return CodecEventStatus::LimitExceeded;
        }
        containers.push(tag);
        pending.push(event);
        return CodecEventStatus::Ok;
    }
    pending.push(event);
    CodecEventStatus::Ok
}

const fn is_structural(tag: CodecEventTag) -> bool {
    (tag as u8) <= CodecEventTag::OptionalPresent as u8
}

const fn is_container_start(tag: CodecEventTag) -> bool {
    matches!(
        tag,
        CodecEventTag::RecordStart
            | CodecEventTag::UnionStart
            | CodecEventTag::TupleStart
            | CodecEventTag::SequenceStart
    )
}

const fn container_start_for_end(tag: CodecEventTag) -> Option<CodecEventTag> {
    match tag {
        CodecEventTag::RecordEnd => Some(CodecEventTag::RecordStart),
        CodecEventTag::UnionEnd => Some(CodecEventTag::UnionStart),
        CodecEventTag::TupleEnd => Some(CodecEventTag::TupleStart),
        CodecEventTag::SequenceEnd => Some(CodecEventTag::SequenceStart),
        _ => None,
    }
}
