use pop_runtime_native_abi::CodecEventStatus;

use super::capability::{capabilities, capability_key};
use super::model::{CODEC_READER_RUNTIME_TYPE, CodecCapability};
use super::scalar::{materialize_scalar, publish_materialized_scalar};

#[allow(unsafe_code)]
pub(super) fn read_event(
    capability: u64,
    out_tag: *mut u8,
    out_ordinal: *mut u32,
    out_label: *mut *const u8,
    out_label_length: *mut u64,
    out_auxiliary: *mut u64,
    out_scalar: *mut u64,
) -> u8 {
    let Some(capability_key) = capability_key(capability, CODEC_READER_RUNTIME_TYPE) else {
        return CodecEventStatus::CapabilityFailure as u8;
    };
    if out_tag.is_null()
        || out_ordinal.is_null()
        || out_label.is_null()
        || out_label_length.is_null()
        || out_auxiliary.is_null()
        || out_scalar.is_null()
    {
        return CodecEventStatus::CapabilityFailure as u8;
    }
    let (position, event) = {
        let Ok(registered) = capabilities().lock() else {
            return CodecEventStatus::CapabilityFailure as u8;
        };
        let Some(CodecCapability::Reader {
            events, position, ..
        }) = registered
            .get(&capability_key)
            .map(|entry| &entry.capability)
        else {
            return CodecEventStatus::CapabilityFailure as u8;
        };
        let Some(event) = events.get(*position) else {
            return CodecEventStatus::MalformedInput as u8;
        };
        (*position, event.clone())
    };
    let scalar = match materialize_scalar(&event.scalar) {
        Ok(scalar) => scalar,
        Err(status) => return status as u8,
    };
    let Ok(mut registered) = capabilities().lock() else {
        return CodecEventStatus::CapabilityFailure as u8;
    };
    let Some(CodecCapability::Reader {
        events,
        position: current,
        borrowed_label,
    }) = registered
        .get_mut(&capability_key)
        .map(|entry| &mut entry.capability)
    else {
        return CodecEventStatus::CapabilityFailure as u8;
    };
    if *current != position || events.get(position) != Some(&event) {
        return CodecEventStatus::CapabilityFailure as u8;
    }
    let stored = &events[position];
    borrowed_label.clone_from(&stored.label);
    let Ok(label_length) = u64::try_from(borrowed_label.len()) else {
        return CodecEventStatus::LimitExceeded as u8;
    };
    let label = if borrowed_label.is_empty() {
        std::ptr::null()
    } else {
        borrowed_label.as_ptr()
    };
    let tag = stored.tag as u8;
    let ordinal = stored.ordinal;
    let auxiliary = stored.auxiliary;
    *current += 1;
    drop(registered);
    // SAFETY: all output pointers were checked non-null and the caller contract
    // guarantees each exact pointee is writable for this call.
    let published = publish_materialized_scalar(scalar, |scalar| unsafe {
        out_tag.write(tag);
        out_ordinal.write(ordinal);
        out_label.write(label);
        out_label_length.write(label_length);
        out_auxiliary.write(auxiliary);
        out_scalar.write(scalar);
    });
    if let Err(status) = published {
        return status as u8;
    }
    CodecEventStatus::Ok as u8
}
