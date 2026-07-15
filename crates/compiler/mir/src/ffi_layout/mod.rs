//! Validated target-selected ABI layouts used by canonical FFI MIR.

use std::collections::BTreeMap;

use pop_foundation::{FieldId, TypeId};
use pop_runtime_interface::FfiAbiLayoutId;
use pop_target::TargetSpec;
use pop_types::TypeArena;

use self::validation::{validate_acyclic, validate_entry};

mod validation;

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
        target: &TargetSpec,
        mut entries: Vec<MirFfiLayout>,
        types: &TypeArena,
    ) -> Result<Self, MirFfiLayoutError> {
        if target.ffi_pointer_layout().is_none() {
            return Err(MirFfiLayoutError::UnsupportedTarget);
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
            validate_entry(entry, &by_id, types, target)?;
        }
        Ok(Self {
            target: target.triple().to_owned(),
            entries,
        })
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
    UnsupportedTarget,
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
