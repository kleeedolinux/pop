use pop_runtime_interface::{ManagedReference, RootHandle, RuntimeAdapter};
use pop_runtime_native_abi::{CodecEventStatus, CodecEventTag};

use super::model::{MAX_CODEC_LABEL_BYTES, MAX_CODEC_PAYLOAD_BYTES, StoredScalar};
use crate::state::lock_abi_runtime;
use crate::text::{allocate_utf8_string, utf8_string_bytes};

#[allow(unsafe_code)]
pub(super) fn copy_label(label: *const u8, length: u64) -> Result<Vec<u8>, CodecEventStatus> {
    let length = usize::try_from(length).map_err(|_| CodecEventStatus::LimitExceeded)?;
    if length > MAX_CODEC_LABEL_BYTES {
        return Err(CodecEventStatus::LimitExceeded);
    }
    if length == 0 {
        return Ok(Vec::new());
    }
    if label.is_null() {
        return Err(CodecEventStatus::MalformedInput);
    }
    // SAFETY: the ABI caller guarantees `length` readable bytes for this call.
    let label = unsafe { std::slice::from_raw_parts(label, length) };
    std::str::from_utf8(label).map_err(|_| CodecEventStatus::MalformedInput)?;
    Ok(label.to_vec())
}

pub(super) enum MaterializedScalar {
    Bits(u64),
    Managed(RootHandle),
}

pub(super) fn materialize_scalar(
    scalar: &StoredScalar,
) -> Result<MaterializedScalar, CodecEventStatus> {
    match scalar {
        StoredScalar::Bits(bits) => Ok(MaterializedScalar::Bits(*bits)),
        StoredScalar::String(bytes) => {
            let mut runtime =
                lock_abi_runtime().map_err(|_| CodecEventStatus::CapabilityFailure)?;
            let reference = allocate_utf8_string(&mut runtime, bytes)
                .map_err(|_| CodecEventStatus::CapabilityFailure)?;
            runtime
                .retain_root(reference)
                .map(MaterializedScalar::Managed)
                .map_err(|_| CodecEventStatus::CapabilityFailure)
        }
        StoredScalar::Bytes(bytes) => {
            let mut runtime =
                lock_abi_runtime().map_err(|_| CodecEventStatus::CapabilityFailure)?;
            let reference = runtime
                .allocate_immutable_bytes(bytes)
                .map_err(|_| CodecEventStatus::CapabilityFailure)?;
            runtime
                .retain_root(reference)
                .map(MaterializedScalar::Managed)
                .map_err(|_| CodecEventStatus::CapabilityFailure)
        }
    }
}

pub(super) fn publish_materialized_scalar(
    scalar: MaterializedScalar,
    publish: impl FnOnce(u64),
) -> Result<(), CodecEventStatus> {
    match scalar {
        MaterializedScalar::Bits(bits) => {
            publish(bits);
            Ok(())
        }
        MaterializedScalar::Managed(root) => {
            let mut runtime =
                lock_abi_runtime().map_err(|_| CodecEventStatus::CapabilityFailure)?;
            let reference = match runtime.resolve_root(root) {
                Ok(reference) => reference,
                Err(_) => {
                    let _ = runtime.release_root(root);
                    return Err(CodecEventStatus::CapabilityFailure);
                }
            };
            publish(reference.raw());
            runtime
                .release_root(root)
                .map_err(|_| CodecEventStatus::CapabilityFailure)
        }
    }
}

pub(super) fn store_scalar(
    tag: CodecEventTag,
    scalar: u64,
) -> Result<StoredScalar, CodecEventStatus> {
    match tag {
        CodecEventTag::Boolean if scalar > 1 => Err(CodecEventStatus::MalformedInput),
        CodecEventTag::String => {
            let runtime = lock_abi_runtime().map_err(|_| CodecEventStatus::CapabilityFailure)?;
            let bytes = utf8_string_bytes(&runtime, ManagedReference::new(scalar))
                .ok_or(CodecEventStatus::CapabilityFailure)?;
            Ok(StoredScalar::String(bytes))
        }
        CodecEventTag::Bytes => {
            let runtime = lock_abi_runtime().map_err(|_| CodecEventStatus::CapabilityFailure)?;
            let reference = ManagedReference::new(scalar);
            let length = runtime
                .immutable_bytes_length(reference)
                .map_err(|_| CodecEventStatus::CapabilityFailure)?;
            let length = usize::try_from(length).map_err(|_| CodecEventStatus::LimitExceeded)?;
            if length > MAX_CODEC_PAYLOAD_BYTES {
                return Err(CodecEventStatus::LimitExceeded);
            }
            let mut bytes = vec![0; length];
            runtime
                .immutable_bytes_read(reference, 0, &mut bytes)
                .map_err(|_| CodecEventStatus::CapabilityFailure)?;
            Ok(StoredScalar::Bytes(bytes))
        }
        _ => Ok(StoredScalar::Bits(scalar)),
    }
}
