use pop_mir::{MirBubble, MirFfiLayout, MirFfiLayoutCatalog, MirFfiValueClass};
use pop_runtime_interface::ForeignAddress;
use pop_target::{CAbiScalarKind, CAbiSignedness, TargetSpec};
use pop_types::{
    FfiCIntegerKind, FloatValue, IntegerKind, IntegerValue, PrimitiveType, SemanticType, TypeArena,
    ffi_c_integer_kind,
};

use crate::{ExecutionError, MirValue};

pub(crate) fn integer_u64(value: &MirValue) -> Result<u64, ExecutionError> {
    let MirValue::Integer(value) = value else {
        return Err(ExecutionError::TypeMismatch);
    };
    value
        .unsigned()
        .or_else(|| value.signed().and_then(|value| u64::try_from(value).ok()))
        .ok_or(ExecutionError::TypeMismatch)
}

pub(crate) fn marshal(
    value: &MirValue,
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
) -> Result<Vec<u8>, ExecutionError> {
    let size = usize::try_from(layout.size()).map_err(|_| ExecutionError::InvalidControlFlow)?;
    let mut bytes = vec![0; size];
    write_value(value, layout, catalog, &mut bytes)?;
    Ok(bytes)
}

pub(crate) fn unmarshal(
    bytes: &[u8],
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
    mir: &MirBubble,
) -> Result<MirValue, ExecutionError> {
    if u64::try_from(bytes.len()) != Ok(layout.size()) {
        return Err(ExecutionError::InvalidControlFlow);
    }
    read_value(bytes, layout, catalog, types, mir)
}

fn write_value(
    value: &MirValue,
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    output: &mut [u8],
) -> Result<(), ExecutionError> {
    match layout.value_class() {
        MirFfiValueClass::Integer => {
            let MirValue::Integer(value) = value else {
                return Err(ExecutionError::TypeMismatch);
            };
            write_bits(value.bits(), output)
        }
        MirFfiValueClass::Float => {
            let MirValue::Float(value) = value else {
                return Err(ExecutionError::TypeMismatch);
            };
            write_bits(value.bits(), output)
        }
        MirFfiValueClass::Pointer => match value {
            MirValue::FfiPointer(address) => write_bits(address.raw(), output),
            MirValue::Nil => write_bits(0, output),
            _ => Err(ExecutionError::TypeMismatch),
        },
        MirFfiValueClass::FunctionPointer => match value {
            MirValue::FfiFunction(address) => write_bits(*address, output),
            MirValue::Nil => write_bits(0, output),
            _ => Err(ExecutionError::TypeMismatch),
        },
        MirFfiValueClass::Handle => {
            let MirValue::FfiHandle(handle) = value else {
                return Err(ExecutionError::TypeMismatch);
            };
            write_bits(*handle, output)
        }
        MirFfiValueClass::Record(plan) => {
            let MirValue::Record { fields, .. } = value else {
                return Err(ExecutionError::TypeMismatch);
            };
            for field in plan {
                let child = catalog
                    .get(field.layout())
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let value = fields
                    .iter()
                    .find_map(|(identity, value)| (*identity == field.field()).then_some(value))
                    .ok_or(ExecutionError::TypeMismatch)?;
                let start = usize::try_from(field.offset())
                    .map_err(|_| ExecutionError::InvalidControlFlow)?;
                let end = start
                    .checked_add(
                        usize::try_from(child.size())
                            .map_err(|_| ExecutionError::InvalidControlFlow)?,
                    )
                    .filter(|end| *end <= output.len())
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                write_value(value, child, catalog, &mut output[start..end])?;
            }
            Ok(())
        }
    }
}

fn read_value(
    bytes: &[u8],
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
    mir: &MirBubble,
) -> Result<MirValue, ExecutionError> {
    Ok(match layout.value_class() {
        MirFfiValueClass::Integer => {
            MirValue::Integer(read_integer(bytes, layout, catalog, types)?)
        }
        MirFfiValueClass::Float => match bytes.len() {
            4 => MirValue::Float(FloatValue::Float32(
                u32::try_from(read_bits(bytes)).map_err(|_| ExecutionError::InvalidControlFlow)?,
            )),
            8 => MirValue::Float(FloatValue::Float64(read_bits(bytes))),
            _ => return Err(ExecutionError::InvalidControlFlow),
        },
        MirFfiValueClass::Pointer => pointer_value(read_bits(bytes)),
        MirFfiValueClass::FunctionPointer => {
            let address = read_bits(bytes);
            if address == 0 {
                MirValue::Nil
            } else {
                MirValue::FfiFunction(address)
            }
        }
        MirFfiValueClass::Handle => MirValue::FfiHandle(read_bits(bytes)),
        MirFfiValueClass::Record(plan) => {
            let record = mir
                .declarations()
                .iter()
                .find_map(|declaration| match declaration.kind() {
                    pop_mir::MirDeclarationKind::Record(record)
                        if record.type_id() == layout.element() =>
                    {
                        Some(declaration.symbol())
                    }
                    _ => None,
                })
                .ok_or(ExecutionError::InvalidControlFlow)?;
            let mut fields = Vec::with_capacity(plan.len());
            for field in plan {
                let child = catalog
                    .get(field.layout())
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                let start = usize::try_from(field.offset())
                    .map_err(|_| ExecutionError::InvalidControlFlow)?;
                let end = start
                    .checked_add(
                        usize::try_from(child.size())
                            .map_err(|_| ExecutionError::InvalidControlFlow)?,
                    )
                    .filter(|end| *end <= bytes.len())
                    .ok_or(ExecutionError::InvalidControlFlow)?;
                fields.push((
                    field.field(),
                    read_value(&bytes[start..end], child, catalog, types, mir)?,
                ));
            }
            MirValue::Record { record, fields }
        }
    })
}

fn read_integer(
    bytes: &[u8],
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
) -> Result<IntegerValue, ExecutionError> {
    let kind = integer_kind(layout, catalog, types)?;
    let bits = read_bits(bytes);
    let text = if kind.is_signed() {
        let shift = 64_u32 - u32::from(kind.bit_width());
        (i64::from_ne_bytes((bits << shift).to_ne_bytes()) >> shift).to_string()
    } else {
        bits.to_string()
    };
    IntegerValue::parse_decimal(&text, kind).map_err(|_| ExecutionError::InvalidControlFlow)
}

fn integer_kind(
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
) -> Result<IntegerKind, ExecutionError> {
    match types.get(layout.element()) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Ok(*kind),
        Some(SemanticType::Builtin { definition, .. }) => {
            let target = TargetSpec::for_triple(catalog.target())
                .map_err(|_| ExecutionError::InvalidControlFlow)?;
            let geometry = ffi_c_integer_kind(*definition)
                .and_then(|kind| target.c_abi_scalar_layout(target_integer_kind(kind)))
                .ok_or(ExecutionError::InvalidControlFlow)?;
            integer_kind_for_geometry(geometry.size(), geometry.signedness())
        }
        _ => Err(ExecutionError::InvalidControlFlow),
    }
}

const fn target_integer_kind(kind: FfiCIntegerKind) -> CAbiScalarKind {
    match kind {
        FfiCIntegerKind::Char => CAbiScalarKind::Char,
        FfiCIntegerKind::SignedChar => CAbiScalarKind::SignedChar,
        FfiCIntegerKind::UnsignedChar => CAbiScalarKind::UnsignedChar,
        FfiCIntegerKind::Short => CAbiScalarKind::Short,
        FfiCIntegerKind::UnsignedShort => CAbiScalarKind::UnsignedShort,
        FfiCIntegerKind::Int => CAbiScalarKind::Int,
        FfiCIntegerKind::UnsignedInt => CAbiScalarKind::UnsignedInt,
        FfiCIntegerKind::Long => CAbiScalarKind::Long,
        FfiCIntegerKind::UnsignedLong => CAbiScalarKind::UnsignedLong,
        FfiCIntegerKind::LongLong => CAbiScalarKind::LongLong,
        FfiCIntegerKind::UnsignedLongLong => CAbiScalarKind::UnsignedLongLong,
        FfiCIntegerKind::Size => CAbiScalarKind::Size,
        FfiCIntegerKind::PointerDifference => CAbiScalarKind::PointerDifference,
    }
}

fn integer_kind_for_geometry(
    size: u64,
    signedness: CAbiSignedness,
) -> Result<IntegerKind, ExecutionError> {
    match (size, signedness) {
        (1, CAbiSignedness::Signed) => Ok(IntegerKind::Int8),
        (2, CAbiSignedness::Signed) => Ok(IntegerKind::Int16),
        (4, CAbiSignedness::Signed) => Ok(IntegerKind::Int32),
        (8, CAbiSignedness::Signed) => Ok(IntegerKind::Int64),
        (1, CAbiSignedness::Unsigned) => Ok(IntegerKind::UInt8),
        (2, CAbiSignedness::Unsigned) => Ok(IntegerKind::UInt16),
        (4, CAbiSignedness::Unsigned) => Ok(IntegerKind::UInt32),
        (8, CAbiSignedness::Unsigned) => Ok(IntegerKind::UInt64),
        _ => Err(ExecutionError::InvalidControlFlow),
    }
}

fn pointer_value(raw: u64) -> MirValue {
    ForeignAddress::new(raw).map_or(MirValue::Nil, MirValue::FfiPointer)
}

fn write_bits(bits: u64, output: &mut [u8]) -> Result<(), ExecutionError> {
    let bytes = bits.to_le_bytes();
    let source = bytes
        .get(..output.len())
        .ok_or(ExecutionError::InvalidControlFlow)?;
    output.copy_from_slice(source);
    Ok(())
}

fn read_bits(bytes: &[u8]) -> u64 {
    let mut output = [0; 8];
    output[..bytes.len()].copy_from_slice(bytes);
    u64::from_le_bytes(output)
}
