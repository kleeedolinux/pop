//! Closed native event capabilities for generated typed metadata adapters.

mod capability;
mod model;
mod read;
mod scalar;
mod write;

pub use capability::{allocate_codec_reader, allocate_codec_writer};

use read::read_event;
use write::write_event;

#[cfg(test)]
use crate::state::lock_abi_runtime;
#[cfg(test)]
use crate::text::utf8_string_bytes;
#[cfg(test)]
use capability::capabilities;
#[cfg(test)]
use model::{CodecCapability, MAX_CODEC_EVENTS, MAX_CODEC_PAYLOAD_BYTES, StoredScalar};
#[cfg(test)]
use pop_runtime_interface::{ManagedReference, RuntimeAdapter};
#[cfg(test)]
use pop_runtime_native_abi::{CodecEventStatus, CodecEventTag};

/// Appends one event to a sealed native writer capability.
///
/// # Safety
///
/// A non-null `label` must address `label_length` readable bytes for the
/// duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_codec_write_event(
    capability: u64,
    tag: u8,
    ordinal: u32,
    label: *const u8,
    label_length: u64,
    auxiliary: u64,
    scalar: u64,
) -> u8 {
    write_event(
        capability,
        tag,
        ordinal,
        label,
        label_length,
        auxiliary,
        scalar,
    ) as u8
}

/// Reads the next closed event from a sealed reader capability.
///
/// The returned label pointer addresses runtime-owned non-managed bytes and is
/// valid until the next read of this capability. Managed `String` and `Bytes`
/// scalar outputs are newly materialized owned values, never interior pointers.
///
/// # Safety
///
/// Every output pointer must be non-null and writable for its exact pointee for
/// the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_codec_read_event(
    capability: u64,
    out_tag: *mut u8,
    out_ordinal: *mut u32,
    out_label: *mut *const u8,
    out_label_length: *mut u64,
    out_auxiliary: *mut u64,
    out_scalar: *mut u64,
) -> u8 {
    read_event(
        capability,
        out_tag,
        out_ordinal,
        out_label,
        out_label_length,
        out_auxiliary,
        out_scalar,
    )
}

include!("codec/tests.rs");
