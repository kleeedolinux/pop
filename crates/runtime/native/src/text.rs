//! Native UTF-8 string and process-argument adapters.

use std::ffi::{CStr, c_char};

use pop_runtime_collector::StableGenerationalRuntime;
use pop_runtime_interface::{
    AllocationClass, ArrayAllocationRequest, ArrayElementMap, ManagedReference, ObjectSlot,
    RuntimeAdapter, RuntimeFailure, RuntimeTypeId,
};
use pop_runtime_native_abi::StringFormatTag;

use crate::state::lock_abi_runtime;

/// Materializes one immutable, valid UTF-8 string from compiler-emitted bytes.
///
/// # Safety
///
/// `bytes` must address `length` readable bytes for the duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_string_literal(bytes: *const u8, length: u64) -> u64 {
    let Ok(length) = usize::try_from(length) else {
        return 0;
    };
    if bytes.is_null() {
        return 0;
    }
    // SAFETY: The native backend supplies a pointer to an immutable LLVM
    // constant with exactly the declared byte length.
    let bytes = unsafe { std::slice::from_raw_parts(bytes, length) };
    allocate_utf8_string_literal(bytes)
}

/// Safe Rust adapter for the native string-literal ABI.
#[must_use]
pub fn allocate_utf8_string_literal(bytes: &[u8]) -> u64 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    allocate_utf8_string(&mut runtime, bytes).map_or(0, ManagedReference::raw)
}

/// Reads one immutable UTF-8 string through the stable native-token boundary.
///
/// The return value is the byte length plus one, reserving zero as failure.
/// Passing a null target queries the required length without copying. A
/// non-null target must provide at least the complete byte length; failures do
/// not write partial data.
///
/// # Safety
///
/// When `target` is non-null, it must address `capacity` writable bytes for the
/// duration of this call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_string_read(reference: u64, target: *mut u8, capacity: u64) -> u64 {
    let Ok(runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Some(values) =
        runtime.scalar_array_values(ManagedReference::new(reference), RuntimeTypeId::new(1))
    else {
        return 0;
    };
    let mut bytes = Vec::with_capacity(values.len());
    for value in values {
        let Ok(byte) = u8::try_from(value) else {
            return 0;
        };
        bytes.push(byte);
    }
    if std::str::from_utf8(&bytes).is_err() {
        return 0;
    }
    let Ok(length) = u64::try_from(bytes.len()) else {
        return 0;
    };
    let Some(encoded_length) = length.checked_add(1) else {
        return 0;
    };
    if target.is_null() {
        return encoded_length;
    }
    if capacity < length {
        return 0;
    }
    // SAFETY: The caller contract guarantees a writable buffer of at least
    // `length` bytes, and `bytes` owns exactly that many initialized bytes.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), target, bytes.len()) };
    encoded_length
}

fn allocate_utf8_string(
    runtime: &mut StableGenerationalRuntime,
    bytes: &[u8],
) -> Result<ManagedReference, RuntimeFailure> {
    std::str::from_utf8(bytes).map_err(|_| RuntimeFailure::runtime_invariant())?;
    let portable_length =
        u32::try_from(bytes.len()).map_err(|_| RuntimeFailure::runtime_invariant())?;
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(1),
        AllocationClass::Mature,
        portable_length,
        ArrayElementMap::Scalar,
    );
    let reference = runtime.allocate_array(&request)?;
    for (index, byte) in bytes.iter().copied().enumerate() {
        let index = u32::try_from(index).map_err(|_| RuntimeFailure::runtime_invariant())?;
        runtime.store_scalar(reference, ObjectSlot::new(index), u64::from(byte))?;
    }
    Ok(reference)
}

/// Materializes the valid UTF-8 arguments that follow the executable path.
///
/// The returned array uses a precise managed-reference element map. Zero is
/// returned when any argument is invalid UTF-8 or allocation fails.
#[must_use]
pub fn allocate_process_arguments(arguments: &[&[u8]]) -> u64 {
    if arguments
        .iter()
        .any(|argument| std::str::from_utf8(argument).is_err())
    {
        return 0;
    }
    let Ok(length) = u32::try_from(arguments.len()) else {
        return 0;
    };
    let request = ArrayAllocationRequest::new(
        RuntimeTypeId::new(2),
        AllocationClass::Mature,
        length,
        ArrayElementMap::ManagedReference,
    );
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Ok(array) = runtime.allocate_array(&request) else {
        return 0;
    };
    let Ok(root) = runtime.retain_root(array) else {
        return 0;
    };
    let result = arguments.iter().enumerate().try_for_each(|(index, bytes)| {
        let index = u32::try_from(index).map_err(|_| RuntimeFailure::runtime_invariant())?;
        let string = allocate_utf8_string(&mut runtime, bytes)?;
        runtime.store_array_value(array, ObjectSlot::new(index), string.raw())
    });
    let released = runtime.release_root(root);
    if result.is_err() || released.is_err() {
        0
    } else {
        array.raw()
    }
}

/// Adapts a complete platform argument vector and omits its executable path.
#[must_use]
pub fn allocate_platform_arguments(arguments: &[&CStr]) -> u64 {
    let bytes: Vec<_> = arguments
        .iter()
        .skip(1)
        .map(|argument| argument.to_bytes())
        .collect();
    allocate_process_arguments(&bytes)
}

/// Converts the platform `main` argument vector into Pop Lang's canonical
/// managed `Array<String>`, excluding the executable path.
///
/// # Safety
///
/// `arguments` must point to `argument_count` readable C-string pointers as
/// supplied to the platform process entry. Each non-null pointer must remain
/// valid for the duration of the call.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn pop_rt_process_arguments(
    argument_count: i32,
    arguments: *const *const c_char,
) -> u64 {
    let Ok(argument_count) = usize::try_from(argument_count) else {
        return 0;
    };
    if argument_count == 0 {
        return allocate_process_arguments(&[]);
    }
    if arguments.is_null() {
        return 0;
    }
    // SAFETY: The platform provides exactly `argument_count` C-string pointers
    // to `main`; the executable path occupies slot zero.
    let arguments = unsafe { std::slice::from_raw_parts(arguments, argument_count) };
    let mut platform_arguments = Vec::with_capacity(argument_count);
    for argument in arguments {
        if argument.is_null() {
            return 0;
        }
        // SAFETY: Each platform argument is a non-null, nul-terminated string.
        platform_arguments.push(unsafe { CStr::from_ptr(*argument) });
    }
    allocate_platform_arguments(&platform_arguments)
}

/// Compares two managed UTF-8 strings by their byte content.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_string_equal(left: u64, right: u64) -> u8 {
    let Ok(runtime) = lock_abi_runtime() else {
        return 0;
    };
    u8::from(runtime.strings_equal(ManagedReference::new(left), ManagedReference::new(right)))
}

/// Concatenates two managed UTF-8 strings into one owned managed string.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_string_concat(left: u64, right: u64) -> u64 {
    let Ok(mut runtime) = lock_abi_runtime() else {
        return 0;
    };
    let Some(left) =
        runtime.scalar_array_values(ManagedReference::new(left), RuntimeTypeId::new(1))
    else {
        return 0;
    };
    let Some(right) =
        runtime.scalar_array_values(ManagedReference::new(right), RuntimeTypeId::new(1))
    else {
        return 0;
    };
    let bytes = left
        .into_iter()
        .chain(right)
        .map(u8::try_from)
        .collect::<Result<Vec<_>, _>>();
    let Ok(bytes) = bytes else {
        return 0;
    };
    allocate_utf8_string(&mut runtime, &bytes).map_or(0, ManagedReference::raw)
}

/// Formats one statically selected primitive value as an owned managed string.
#[allow(unsafe_code)]
#[unsafe(no_mangle)]
pub extern "C" fn pop_rt_string_format(tag: u32, bits: u64) -> u64 {
    let Some(tag) = StringFormatTag::from_raw(tag) else {
        return 0;
    };
    let formatted = match tag {
        StringFormatTag::Boolean if bits <= 1 => (bits != 0).to_string(),
        StringFormatTag::Boolean => return 0,
        StringFormatTag::Int8 => i8::from_le_bytes([low_u8(bits)]).to_string(),
        StringFormatTag::Int16 => i16::from_le_bytes(low_u16(bits).to_le_bytes()).to_string(),
        StringFormatTag::Int32 => i32::from_le_bytes(low_u32(bits).to_le_bytes()).to_string(),
        StringFormatTag::Int64 => i64::from_ne_bytes(bits.to_ne_bytes()).to_string(),
        StringFormatTag::UInt8 => low_u8(bits).to_string(),
        StringFormatTag::UInt16 => low_u16(bits).to_string(),
        StringFormatTag::UInt32 => low_u32(bits).to_string(),
        StringFormatTag::UInt64 => bits.to_string(),
        StringFormatTag::Float32 => format_float32(f32::from_bits(low_u32(bits))),
        StringFormatTag::Float64 => format_float64(f64::from_bits(bits)),
    };
    allocate_utf8_string_literal(formatted.as_bytes())
}

fn low_u8(bits: u64) -> u8 {
    bits.to_le_bytes()[0]
}

fn low_u16(bits: u64) -> u16 {
    let bytes = bits.to_le_bytes();
    u16::from_le_bytes([bytes[0], bytes[1]])
}

fn low_u32(bits: u64) -> u32 {
    let bytes = bits.to_le_bytes();
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn format_float32(value: f32) -> String {
    if value.is_nan() {
        "nan".to_owned()
    } else if value == f32::INFINITY {
        "inf".to_owned()
    } else if value == f32::NEG_INFINITY {
        "-inf".to_owned()
    } else {
        value.to_string()
    }
}

fn format_float64(value: f64) -> String {
    if value.is_nan() {
        "nan".to_owned()
    } else if value == f64::INFINITY {
        "inf".to_owned()
    } else if value == f64::NEG_INFINITY {
        "-inf".to_owned()
    } else {
        value.to_string()
    }
}
