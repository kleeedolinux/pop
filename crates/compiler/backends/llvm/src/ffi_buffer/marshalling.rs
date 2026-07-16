use pop_foundation::TypeId;
use pop_mir::{MirFfiLayout, MirFfiLayoutCatalog, MirFfiValueClass};
use pop_runtime_interface::RuntimeOperation;
use pop_target::{CAbiScalarKind, CAbiSignedness, TargetSpec};
use pop_types::{FfiCIntegerKind, PrimitiveType, SemanticType, TypeArena, ffi_c_integer_kind};

use crate::api::LlvmLoweringError;
use crate::instruction_lowering::{
    llvm_type, lower_mapped_allocation, lower_runtime_slot_load_named,
};
use crate::lowering::native_runtime_symbol;

/// Renders the target ABI value type from one verified canonical layout.
/// Record order comes from the catalog field plan; LLVM supplies only the
/// physical target calling convention for that already-authorized shape.
pub(crate) fn physical_type(
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
) -> Result<String, LlvmLoweringError> {
    match layout.value_class() {
        MirFfiValueClass::Integer => Ok(format!("i{}", layout.size() * 8)),
        MirFfiValueClass::Float if layout.size() == 4 => Ok("float".to_owned()),
        MirFfiValueClass::Float if layout.size() == 8 => Ok("double".to_owned()),
        MirFfiValueClass::Pointer | MirFfiValueClass::FunctionPointer => Ok("ptr".to_owned()),
        MirFfiValueClass::Handle => Ok("i64".to_owned()),
        MirFfiValueClass::Record(fields) => fields
            .iter()
            .map(|field| {
                catalog
                    .get(field.layout())
                    .ok_or(LlvmLoweringError::InvalidFfiLayout(field.layout()))
                    .and_then(|child| physical_type(child, catalog))
            })
            .collect::<Result<Vec<_>, _>>()
            .map(|fields| format!("{{ {} }}", fields.join(", "))),
        MirFfiValueClass::Float => Err(LlvmLoweringError::InvalidType(layout.element())),
    }
}

pub(crate) fn marshal(
    value: &str,
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
    destination: &str,
    prefix: &str,
) -> Result<Vec<String>, LlvmLoweringError> {
    match layout.value_class() {
        MirFfiValueClass::Integer => {
            marshal_integer(value, layout, catalog, types, destination, prefix)
        }
        MirFfiValueClass::Float => Ok(vec![format!(
            "store {} {value}, ptr {destination}, align {}",
            abi_type(layout)?,
            layout.alignment()
        )]),
        MirFfiValueClass::Pointer
        | MirFfiValueClass::FunctionPointer
        | MirFfiValueClass::Handle => Ok(vec![format!(
            "store i64 {value}, ptr {destination}, align {}",
            layout.alignment()
        )]),
        MirFfiValueClass::Record(fields) => {
            let mut lines = Vec::new();
            for (index, field) in fields.iter().enumerate() {
                let child = catalog
                    .get(field.layout())
                    .ok_or(LlvmLoweringError::InvalidType(layout.element()))?;
                let slot = field.source_index() + 1;
                let field_value = format!("{prefix}_field_{index}");
                lines.extend(lower_runtime_slot_load_named(
                    &field_value,
                    child.element(),
                    value,
                    slot as usize,
                    types,
                )?);
                let pointer = format!("{prefix}_pointer_{index}");
                lines.push(format!(
                    "{pointer} = getelementptr i8, ptr {destination}, i64 {}",
                    field.offset()
                ));
                lines.extend(marshal(
                    &field_value,
                    child,
                    catalog,
                    types,
                    &pointer,
                    &format!("{prefix}_{index}"),
                )?);
            }
            Ok(lines)
        }
    }
}

pub(crate) fn unmarshal(
    result: &str,
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
    source: &str,
) -> Result<Vec<String>, LlvmLoweringError> {
    match layout.value_class() {
        MirFfiValueClass::Integer => unmarshal_integer(result, layout, catalog, types, source),
        MirFfiValueClass::Float => Ok(vec![format!(
            "{result} = load {}, ptr {source}, align {}",
            abi_type(layout)?,
            layout.alignment()
        )]),
        MirFfiValueClass::Pointer
        | MirFfiValueClass::FunctionPointer
        | MirFfiValueClass::Handle => Ok(vec![format!(
            "{result} = load i64, ptr {source}, align {}",
            layout.alignment()
        )]),
        MirFfiValueClass::Record(fields) => {
            let mut lines = lower_mapped_allocation(result, fields.len() as u32, &[]);
            for (index, field) in fields.iter().enumerate() {
                let child = catalog
                    .get(field.layout())
                    .ok_or(LlvmLoweringError::InvalidType(layout.element()))?;
                let slot = field.source_index() + 1;
                let pointer = format!("{result}_pointer_{index}");
                lines.push(format!(
                    "{pointer} = getelementptr i8, ptr {source}, i64 {}",
                    field.offset()
                ));
                let field_value = format!("{result}_field_{index}");
                lines.extend(unmarshal(&field_value, child, catalog, types, &pointer)?);
                let (conversions, stored) = runtime_slot_value(
                    &field_value,
                    child.element(),
                    types,
                    &format!("{result}_field_{index}_slot"),
                )?;
                lines.extend(conversions);
                lines.push(format!(
                    "call i8 @{}(i64 {result}, i64 {slot}, i64 {stored})",
                    native_runtime_symbol(RuntimeOperation::FieldSet)
                ));
            }
            Ok(lines)
        }
    }
}

fn marshal_integer(
    value: &str,
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
    destination: &str,
    prefix: &str,
) -> Result<Vec<String>, LlvmLoweringError> {
    let abi = abi_type(layout)?;
    let semantic = llvm_type(layout.element(), types)?;
    let stored = if semantic == abi {
        value.to_owned()
    } else if semantic == "i64" {
        format!("{prefix}_integer")
    } else {
        return Err(LlvmLoweringError::InvalidType(layout.element()));
    };
    let mut lines = if stored == value {
        Vec::new()
    } else {
        vec![format!("{stored} = trunc i64 {value} to {abi}")]
    };
    let _ = integer_signedness(layout, catalog, types)?;
    lines.push(format!(
        "store {abi} {stored}, ptr {destination}, align {}",
        layout.alignment()
    ));
    Ok(lines)
}

fn unmarshal_integer(
    result: &str,
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
    source: &str,
) -> Result<Vec<String>, LlvmLoweringError> {
    let abi = abi_type(layout)?;
    let semantic = llvm_type(layout.element(), types)?;
    if semantic == abi {
        return Ok(vec![format!(
            "{result} = load {abi}, ptr {source}, align {}",
            layout.alignment()
        )]);
    }
    if semantic != "i64" {
        return Err(LlvmLoweringError::InvalidType(layout.element()));
    }
    let extension = match integer_signedness(layout, catalog, types)? {
        CAbiSignedness::Signed => "sext",
        CAbiSignedness::Unsigned => "zext",
    };
    Ok(vec![
        format!(
            "{result}_abi = load {abi}, ptr {source}, align {}",
            layout.alignment()
        ),
        format!("{result} = {extension} {abi} {result}_abi to i64"),
    ])
}

fn abi_type(layout: &MirFfiLayout) -> Result<String, LlvmLoweringError> {
    match layout.value_class() {
        MirFfiValueClass::Integer => Ok(format!("i{}", layout.size() * 8)),
        MirFfiValueClass::Float if layout.size() == 4 => Ok("float".to_owned()),
        MirFfiValueClass::Float if layout.size() == 8 => Ok("double".to_owned()),
        _ => Err(LlvmLoweringError::InvalidType(layout.element())),
    }
}

fn integer_signedness(
    layout: &MirFfiLayout,
    catalog: &MirFfiLayoutCatalog,
    types: &TypeArena,
) -> Result<CAbiSignedness, LlvmLoweringError> {
    match types.get(layout.element()) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Ok(if kind.is_signed() {
            CAbiSignedness::Signed
        } else {
            CAbiSignedness::Unsigned
        }),
        Some(SemanticType::Builtin { definition, .. }) => TargetSpec::for_triple(catalog.target())
            .ok()
            .and_then(|target| {
                ffi_c_integer_kind(*definition)
                    .and_then(|kind| target.c_abi_scalar_layout(target_integer_kind(kind)))
            })
            .map(pop_target::CAbiScalarLayout::signedness)
            .ok_or(LlvmLoweringError::InvalidType(layout.element())),
        _ => Err(LlvmLoweringError::InvalidType(layout.element())),
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

fn runtime_slot_value(
    value: &str,
    type_id: TypeId,
    types: &TypeArena,
    converted: &str,
) -> Result<(Vec<String>, String), LlvmLoweringError> {
    let ty = llvm_type(type_id, types)?;
    Ok(match ty.as_str() {
        "i64" => (Vec::new(), value.to_owned()),
        "i1" | "i8" | "i16" | "i32" => (
            vec![format!("{converted} = zext {ty} {value} to i64")],
            converted.to_owned(),
        ),
        "float" => (
            vec![
                format!("{converted}_bits = bitcast float {value} to i32"),
                format!("{converted} = zext i32 {converted}_bits to i64"),
            ],
            converted.to_owned(),
        ),
        "double" => (
            vec![format!("{converted} = bitcast double {value} to i64")],
            converted.to_owned(),
        ),
        _ => return Err(LlvmLoweringError::InvalidType(type_id)),
    })
}
