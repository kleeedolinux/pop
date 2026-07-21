//! Typed native helpers for compiler-proven Text and Bytes view descriptors.
#![allow(unsafe_code)]

use pop_runtime_interface::{ManagedReference, RuntimeAdapter};

use crate::state::lock_abi_runtime;
use crate::text::{allocate_utf8_string, utf8_string_bytes};

#[repr(C)]
pub struct ViewLengths {
    byte_length: u64,
    scalar_length: u64,
}

#[repr(C)]
pub struct ViewRange {
    valid: bool,
    byte_offset: u64,
    byte_length: u64,
    scalar_length: u64,
}

#[repr(C)]
pub struct OptionalByte {
    present: bool,
    value: u8,
}

#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_bytes_view_lengths(reference: u64) -> ViewLengths {
    let byte_length = lock_abi_runtime()
        .ok()
        .and_then(|runtime| {
            runtime
                .immutable_bytes_length(ManagedReference::new(reference))
                .ok()
        })
        .unwrap_or(0);
    ViewLengths {
        byte_length,
        scalar_length: byte_length,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_text_view_lengths(reference: u64) -> ViewLengths {
    let Some(bytes) = lock_abi_runtime()
        .ok()
        .and_then(|runtime| utf8_string_bytes(&runtime, ManagedReference::new(reference)))
    else {
        return ViewLengths {
            byte_length: 0,
            scalar_length: 0,
        };
    };
    ViewLengths {
        byte_length: u64::try_from(bytes.len()).unwrap_or(0),
        scalar_length: std::str::from_utf8(&bytes)
            .ok()
            .and_then(|text| u64::try_from(text.chars().count()).ok())
            .unwrap_or(0),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_bytes_view_slice(
    reference: u64,
    parent_offset: u64,
    parent_bytes: u64,
    _parent_scalars: u64,
    start: i64,
    length: i64,
) -> ViewRange {
    let owner_length = lock_abi_runtime().ok().and_then(|runtime| {
        runtime
            .immutable_bytes_length(ManagedReference::new(reference))
            .ok()
    });
    let Some((length, byte_offset)) = owner_length.and_then(|owner| {
        let (relative, length) = checked_range(parent_bytes, start, length)?;
        let byte_offset = parent_offset.checked_add(relative)?;
        let byte_end = byte_offset.checked_add(length)?;
        (byte_end <= owner).then_some((length, byte_offset))
    }) else {
        return invalid_range();
    };
    ViewRange {
        valid: true,
        byte_offset,
        byte_length: length,
        scalar_length: length,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_text_view_slice(
    reference: u64,
    parent_offset: u64,
    parent_bytes: u64,
    parent_scalars: u64,
    start: i64,
    length: i64,
) -> ViewRange {
    let Some(bytes) = lock_abi_runtime()
        .ok()
        .and_then(|runtime| utf8_string_bytes(&runtime, ManagedReference::new(reference)))
    else {
        return invalid_range();
    };
    let Some(parent_end) = parent_offset
        .checked_add(parent_bytes)
        .and_then(|end| usize::try_from(end).ok())
        .filter(|end| *end <= bytes.len())
    else {
        return invalid_range();
    };
    let Ok(parent_start) = usize::try_from(parent_offset) else {
        return invalid_range();
    };
    let Some(text) = std::str::from_utf8(&bytes[parent_start..parent_end]).ok() else {
        return invalid_range();
    };
    let Some((relative, length)) = checked_range(parent_scalars, start, length) else {
        return invalid_range();
    };
    let Some(byte_start) = scalar_byte_offset(text, relative) else {
        return invalid_range();
    };
    let Some(scalar_end) = relative.checked_add(length) else {
        return invalid_range();
    };
    let Some(byte_end) = scalar_byte_offset(text, scalar_end) else {
        return invalid_range();
    };
    let Some(byte_offset) =
        parent_offset.checked_add(u64::try_from(byte_start).unwrap_or(u64::MAX))
    else {
        return invalid_range();
    };
    ViewRange {
        valid: true,
        byte_offset,
        byte_length: u64::try_from(byte_end - byte_start).unwrap_or(0),
        scalar_length: length,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_bytes_view_get(
    reference: u64,
    offset: u64,
    length: u64,
    index: i64,
) -> OptionalByte {
    let Some(relative) = index
        .checked_sub(1)
        .and_then(|index| u64::try_from(index).ok())
    else {
        return OptionalByte {
            present: false,
            value: 0,
        };
    };
    if relative >= length {
        return OptionalByte {
            present: false,
            value: 0,
        };
    }
    let mut value = [0_u8; 1];
    let present = lock_abi_runtime().is_ok_and(|runtime| {
        runtime
            .immutable_bytes_read(
                ManagedReference::new(reference),
                offset.saturating_add(relative),
                &mut value,
            )
            .is_ok()
    });
    OptionalByte {
        present,
        value: value[0],
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_bytes_view_materialize(reference: u64, offset: u64, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let mut bytes = vec![0_u8; length];
    if runtime
        .immutable_bytes_read(ManagedReference::new(reference), offset, &mut bytes)
        .is_err()
    {
        return 0;
    }
    runtime
        .allocate_immutable_bytes(&bytes)
        .map_or(0, ManagedReference::raw)
}

#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_text_view_materialize(reference: u64, offset: u64, length: u64) -> u64 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Some(bytes) = utf8_string_bytes(&runtime, ManagedReference::new(reference)) else {
        return 0;
    };
    let Some(end) = offset.checked_add(length) else {
        return 0;
    };
    let (Ok(start), Ok(end)) = (usize::try_from(offset), usize::try_from(end)) else {
        return 0;
    };
    bytes
        .get(start..end)
        .and_then(|bytes| allocate_utf8_string(&mut runtime, bytes).ok())
        .map_or(0, ManagedReference::raw)
}

fn checked_range(owner: u64, start: i64, length: i64) -> Option<(u64, u64)> {
    let start = start
        .checked_sub(1)
        .and_then(|start| u64::try_from(start).ok())?;
    let length = u64::try_from(length).ok()?;
    let end = start.checked_add(length)?;
    (end <= owner && (length == 0 || start < owner)).then_some((start, length))
}

fn scalar_byte_offset(text: &str, scalar: u64) -> Option<usize> {
    let scalar = usize::try_from(scalar).ok()?;
    if scalar == text.chars().count() {
        return Some(text.len());
    }
    text.char_indices().nth(scalar).map(|(offset, _)| offset)
}

const fn invalid_range() -> ViewRange {
    ViewRange {
        valid: false,
        byte_offset: 0,
        byte_length: 0,
        scalar_length: 0,
    }
}

#[cfg(test)]
mod tests {
    use super::{checked_range, scalar_byte_offset};

    #[test]
    fn checked_ranges_are_one_based_and_overflow_safe() {
        assert_eq!(checked_range(4, 1, 4), Some((0, 4)));
        assert_eq!(checked_range(4, 5, 0), Some((4, 0)));
        assert_eq!(checked_range(4, 0, 0), None);
        assert_eq!(checked_range(4, 6, 0), None);
        assert_eq!(checked_range(4, 4, 2), None);
        assert_eq!(checked_range(4, 1, -1), None);
        assert_eq!(checked_range(4, 2, i64::MAX), None);
    }

    #[test]
    fn scalar_offsets_never_split_utf8() {
        let text = "AéZ";
        assert_eq!(scalar_byte_offset(text, 0), Some(0));
        assert_eq!(scalar_byte_offset(text, 1), Some(1));
        assert_eq!(scalar_byte_offset(text, 2), Some(3));
        assert_eq!(scalar_byte_offset(text, 3), Some(4));
        assert_eq!(scalar_byte_offset(text, 4), None);
    }
}
