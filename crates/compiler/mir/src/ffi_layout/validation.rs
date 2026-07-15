use std::collections::{BTreeMap, BTreeSet};

use pop_runtime_interface::FfiAbiLayoutId;
use pop_target::{CAbiScalarKind, TargetSpec};
use pop_types::{
    FFI_HANDLE_TYPE_ID, FfiCIntegerKind, PrimitiveType, SemanticType, TypeArena,
    ffi_c_integer_kind, is_ffi_function_type_constructor, is_ffi_integer_abi_builtin_type,
    is_ffi_pointer_type_constructor,
};

use super::{MirFfiLayout, MirFfiLayoutError, MirFfiValueClass};

pub(super) fn validate_entry(
    entry: &MirFfiLayout,
    by_id: &BTreeMap<FfiAbiLayoutId, &MirFfiLayout>,
    types: &TypeArena,
    target: &TargetSpec,
) -> Result<(), MirFfiLayoutError> {
    if entry.size == 0
        || entry.alignment == 0
        || !entry.alignment.is_power_of_two()
        || !entry.size.is_multiple_of(entry.alignment)
    {
        return Err(MirFfiLayoutError::InvalidGeometry(entry.id));
    }
    let Some(semantic) = types.get(entry.element) else {
        return Err(MirFfiLayoutError::TypeClassMismatch(entry.id));
    };
    if !value_class_matches(&entry.value_class, semantic) {
        return Err(MirFfiLayoutError::TypeClassMismatch(entry.id));
    }
    if let Some(expected_size) = primitive_size(semantic)
        && entry.size != expected_size
    {
        return Err(MirFfiLayoutError::TypeClassMismatch(entry.id));
    }
    if !target_geometry_matches(entry, semantic, target) {
        return Err(MirFfiLayoutError::TypeClassMismatch(entry.id));
    }
    let MirFfiValueClass::Record(fields) = &entry.value_class else {
        return Ok(());
    };
    let SemanticType::Record(source_fields) = semantic else {
        return Err(MirFfiLayoutError::TypeClassMismatch(entry.id));
    };
    if fields.len() != source_fields.len() {
        return Err(MirFfiLayoutError::InvalidRecordFields(entry.id));
    }
    let mut identities = BTreeSet::new();
    let mut ranges = Vec::with_capacity(fields.len());
    for (expected_index, field) in fields.iter().enumerate() {
        if field.source_index as usize != expected_index || !identities.insert(field.field) {
            return Err(MirFfiLayoutError::InvalidRecordFields(entry.id));
        }
        let child = by_id
            .get(&field.layout)
            .copied()
            .ok_or(MirFfiLayoutError::MissingFieldLayout(field.layout))?;
        if child.abi != entry.abi {
            return Err(MirFfiLayoutError::RecordAbiMismatch(entry.id));
        }
        if source_fields[expected_index].1 != child.element {
            return Err(MirFfiLayoutError::InvalidRecordFields(entry.id));
        }
        if child.alignment > entry.alignment || field.offset % child.alignment != 0 {
            return Err(MirFfiLayoutError::MisalignedField(entry.id));
        }
        let end = field
            .offset
            .checked_add(child.size)
            .filter(|end| *end <= entry.size)
            .ok_or(MirFfiLayoutError::FieldOutsideLayout(entry.id))?;
        ranges.push((field.offset, end));
    }
    ranges.sort_unstable();
    if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
        return Err(MirFfiLayoutError::OverlappingFields(entry.id));
    }
    Ok(())
}

fn target_geometry_matches(
    entry: &MirFfiLayout,
    semantic: &SemanticType,
    target: &TargetSpec,
) -> bool {
    let geometry = match semantic {
        SemanticType::Builtin { definition, .. } => match &entry.value_class {
            MirFfiValueClass::Integer => ffi_c_integer_kind(*definition)
                .and_then(|kind| target.c_abi_scalar_layout(target_integer_kind(kind)))
                .map(|layout| (layout.size(), layout.alignment())),
            MirFfiValueClass::Pointer | MirFfiValueClass::FunctionPointer => {
                target.ffi_pointer_layout()
            }
            MirFfiValueClass::Handle if *definition == FFI_HANDLE_TYPE_ID => Some((8, 8)),
            MirFfiValueClass::Float | MirFfiValueClass::Handle | MirFfiValueClass::Record(_) => {
                None
            }
        },
        _ => return true,
    };
    geometry.is_some_and(|(size, alignment)| entry.size == size && entry.alignment == alignment)
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

fn primitive_size(semantic: &SemanticType) -> Option<u64> {
    match semantic {
        SemanticType::Primitive(PrimitiveType::Integer(kind)) => {
            Some(u64::from(kind.bit_width()) / 8)
        }
        SemanticType::Primitive(PrimitiveType::Float32) => Some(4),
        SemanticType::Primitive(PrimitiveType::Float64) => Some(8),
        _ => None,
    }
}

fn value_class_matches(value_class: &MirFfiValueClass, semantic: &SemanticType) -> bool {
    match (value_class, semantic) {
        (MirFfiValueClass::Integer, SemanticType::Primitive(PrimitiveType::Integer(_)))
        | (
            MirFfiValueClass::Float,
            SemanticType::Primitive(PrimitiveType::Float32 | PrimitiveType::Float64),
        )
        | (MirFfiValueClass::Record(_), SemanticType::Record(_)) => true,
        (class, SemanticType::Builtin { definition, .. }) => match class {
            MirFfiValueClass::Integer => is_ffi_integer_abi_builtin_type(*definition),
            MirFfiValueClass::Pointer => is_ffi_pointer_type_constructor(*definition),
            MirFfiValueClass::FunctionPointer => is_ffi_function_type_constructor(*definition),
            MirFfiValueClass::Handle => *definition == FFI_HANDLE_TYPE_ID,
            MirFfiValueClass::Float | MirFfiValueClass::Record(_) => false,
        },
        _ => false,
    }
}

pub(super) fn validate_acyclic(
    entries: &[MirFfiLayout],
    by_id: &BTreeMap<FfiAbiLayoutId, &MirFfiLayout>,
) -> Result<(), MirFfiLayoutError> {
    let mut complete = BTreeSet::new();
    for entry in entries {
        let mut active = BTreeSet::new();
        visit_layout(entry.id, by_id, &mut active, &mut complete)?;
    }
    Ok(())
}

fn visit_layout(
    id: FfiAbiLayoutId,
    by_id: &BTreeMap<FfiAbiLayoutId, &MirFfiLayout>,
    active: &mut BTreeSet<FfiAbiLayoutId>,
    complete: &mut BTreeSet<FfiAbiLayoutId>,
) -> Result<(), MirFfiLayoutError> {
    if complete.contains(&id) {
        return Ok(());
    }
    if !active.insert(id) {
        return Err(MirFfiLayoutError::RecursiveByValueLayout(id));
    }
    if let Some(MirFfiLayout {
        value_class: MirFfiValueClass::Record(fields),
        ..
    }) = by_id.get(&id).copied()
    {
        for field in fields {
            visit_layout(field.layout, by_id, active, complete)?;
        }
    }
    active.remove(&id);
    complete.insert(id);
    Ok(())
}
