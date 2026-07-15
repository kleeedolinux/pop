//! Validated target-selected ABI layouts used by canonical FFI MIR.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{FieldId, TypeId};
use pop_runtime_interface::FfiAbiLayoutId;
use pop_types::{
    FFI_HANDLE_TYPE_ID, PrimitiveType, SemanticType, TypeArena, is_ffi_function_type_constructor,
    is_ffi_integer_abi_builtin_type, is_ffi_pointer_type_constructor,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum MirFfiValueClass {
    Integer,
    Float,
    Pointer,
    FunctionPointer,
    Handle,
    Record(Vec<MirFfiLayoutField>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MirFfiLayoutField {
    field: FieldId,
    source_index: u32,
    layout: FfiAbiLayoutId,
    offset: u64,
}

impl MirFfiLayoutField {
    #[must_use]
    pub const fn new(
        field: FieldId,
        source_index: u32,
        layout: FfiAbiLayoutId,
        offset: u64,
    ) -> Self {
        Self {
            field,
            source_index,
            layout,
            offset,
        }
    }

    #[must_use]
    pub const fn field(self) -> FieldId {
        self.field
    }

    #[must_use]
    pub const fn source_index(self) -> u32 {
        self.source_index
    }

    #[must_use]
    pub const fn layout(self) -> FfiAbiLayoutId {
        self.layout
    }

    #[must_use]
    pub const fn offset(self) -> u64 {
        self.offset
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirFfiLayout {
    id: FfiAbiLayoutId,
    element: TypeId,
    size: u64,
    alignment: u64,
    value_class: MirFfiValueClass,
}

impl MirFfiLayout {
    #[must_use]
    pub const fn new(
        id: FfiAbiLayoutId,
        element: TypeId,
        size: u64,
        alignment: u64,
        value_class: MirFfiValueClass,
    ) -> Self {
        Self {
            id,
            element,
            size,
            alignment,
            value_class,
        }
    }

    #[must_use]
    pub const fn id(&self) -> FfiAbiLayoutId {
        self.id
    }

    #[must_use]
    pub const fn element(&self) -> TypeId {
        self.element
    }

    #[must_use]
    pub const fn size(&self) -> u64 {
        self.size
    }

    #[must_use]
    pub const fn alignment(&self) -> u64 {
        self.alignment
    }

    #[must_use]
    pub const fn value_class(&self) -> &MirFfiValueClass {
        &self.value_class
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MirFfiLayoutCatalog {
    target: String,
    entries: Vec<MirFfiLayout>,
}

impl MirFfiLayoutCatalog {
    /// Validates and canonicalizes one exact target layout catalog.
    ///
    /// # Errors
    ///
    /// Returns the first deterministic geometry, type, field-plan, or graph
    /// violation.
    pub fn new(
        target: impl Into<String>,
        mut entries: Vec<MirFfiLayout>,
        types: &TypeArena,
    ) -> Result<Self, MirFfiLayoutError> {
        let target = target.into();
        if target.trim().is_empty() {
            return Err(MirFfiLayoutError::EmptyTarget);
        }
        entries.sort_by_key(MirFfiLayout::id);
        for pair in entries.windows(2) {
            if pair[0].id == pair[1].id {
                return Err(MirFfiLayoutError::DuplicateLayout(pair[0].id));
            }
        }
        let by_id = entries
            .iter()
            .map(|entry| (entry.id, entry))
            .collect::<BTreeMap<_, _>>();
        validate_acyclic(&entries, &by_id)?;
        for entry in &entries {
            validate_entry(entry, &by_id, types)?;
        }
        Ok(Self { target, entries })
    }

    #[must_use]
    pub fn target(&self) -> &str {
        &self.target
    }

    #[must_use]
    pub fn entries(&self) -> &[MirFfiLayout] {
        &self.entries
    }

    #[must_use]
    pub fn get(&self, id: FfiAbiLayoutId) -> Option<&MirFfiLayout> {
        self.entries
            .binary_search_by_key(&id, MirFfiLayout::id)
            .ok()
            .map(|index| &self.entries[index])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MirFfiLayoutError {
    EmptyTarget,
    DuplicateLayout(FfiAbiLayoutId),
    InvalidGeometry(FfiAbiLayoutId),
    TypeClassMismatch(FfiAbiLayoutId),
    InvalidRecordFields(FfiAbiLayoutId),
    MissingFieldLayout(FfiAbiLayoutId),
    MisalignedField(FfiAbiLayoutId),
    FieldOutsideLayout(FfiAbiLayoutId),
    OverlappingFields(FfiAbiLayoutId),
    RecursiveByValueLayout(FfiAbiLayoutId),
}

fn validate_entry(
    entry: &MirFfiLayout,
    by_id: &BTreeMap<FfiAbiLayoutId, &MirFfiLayout>,
    types: &TypeArena,
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

fn validate_acyclic(
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
