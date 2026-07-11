use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use pop_foundation::TypeId;

use crate::{PrimitiveType, SemanticType};

#[derive(Clone, Debug)]
pub struct TypeArena {
    types: Vec<SemanticType>,
    interned: BTreeMap<SemanticType, TypeId>,
    source_names: BTreeMap<&'static str, TypeId>,
    nil: TypeId,
    never: TypeId,
}

impl TypeArena {
    #[must_use]
    pub fn new() -> Self {
        let mut arena = Self {
            types: Vec::new(),
            interned: BTreeMap::new(),
            source_names: BTreeMap::new(),
            nil: TypeId::from_raw(0),
            never: TypeId::from_raw(0),
        };
        for entry in PrimitiveType::source_schema() {
            let semantic = SemanticType::Primitive(entry.primitive());
            let id = arena.intern_canonical(semantic);
            arena.source_names.insert(entry.source_name(), id);
            match entry.source_name() {
                "nil" => arena.nil = id,
                "Never" => arena.never = id,
                _ => {}
            }
        }
        arena
    }

    #[must_use]
    pub fn source_type(&self, name: &str) -> Option<TypeId> {
        self.source_names.get(name).copied()
    }

    #[must_use]
    pub fn get(&self, id: TypeId) -> Option<&SemanticType> {
        self.types.get(id.raw() as usize)
    }

    /// Interns a semantic type after validating all referenced type IDs.
    ///
    /// # Errors
    ///
    /// Returns [`TypeArenaError::UnknownTypeId`] for an invalid referenced ID.
    pub fn intern(&mut self, semantic: SemanticType) -> Result<TypeId, TypeArenaError> {
        match semantic {
            SemanticType::Union(members) => self.union(members),
            SemanticType::Optional(inner) => self.optional(inner),
            SemanticType::Record(mut fields) => {
                self.validate_ids(fields.iter().map(|(_, field_type)| *field_type))?;
                fields.sort_by(|left, right| left.0.cmp(&right.0));
                Ok(self.intern_canonical(SemanticType::Record(fields)))
            }
            semantic => {
                self.validate_ids(referenced_types(&semantic))?;
                Ok(self.intern_canonical(semantic))
            }
        }
    }

    /// Creates a normalized deterministic union.
    ///
    /// # Errors
    ///
    /// Returns [`TypeArenaError::UnknownTypeId`] for an invalid member ID.
    pub fn union(
        &mut self,
        members: impl IntoIterator<Item = TypeId>,
    ) -> Result<TypeId, TypeArenaError> {
        let never = self.never;
        let mut normalized = BTreeSet::new();
        let mut pending: Vec<_> = members.into_iter().collect();
        while let Some(member) = pending.pop() {
            let semantic = self
                .get(member)
                .ok_or(TypeArenaError::UnknownTypeId(member))?;
            if member == never {
                continue;
            }
            if let SemanticType::Union(nested) = semantic {
                pending.extend(nested.iter().copied());
            } else {
                normalized.insert(member);
            }
        }
        match normalized.len() {
            0 => Ok(never),
            1 => Ok(normalized.first().copied().unwrap_or(never)),
            _ => Ok(self.intern_canonical(SemanticType::Union(normalized.into_iter().collect()))),
        }
    }

    /// Creates `T | nil` using normal union canonicalization.
    ///
    /// # Errors
    ///
    /// Returns [`TypeArenaError::UnknownTypeId`] when `inner` is invalid.
    pub fn optional(&mut self, inner: TypeId) -> Result<TypeId, TypeArenaError> {
        self.union([inner, self.nil])
    }

    #[must_use]
    pub fn is_valid_hir_type(&self, id: TypeId) -> bool {
        self.get(id).is_some_and(SemanticType::is_valid_hir_type)
    }

    #[must_use]
    pub fn is_valid_compile_time_type(&self, id: TypeId) -> bool {
        self.get(id)
            .is_some_and(SemanticType::is_valid_compile_time_type)
    }

    fn intern_canonical(&mut self, semantic: SemanticType) -> TypeId {
        if let Some(id) = self.interned.get(&semantic) {
            return *id;
        }
        let raw = u32::try_from(self.types.len()).expect("type arena exhausted TypeId range");
        let id = TypeId::from_raw(raw);
        self.types.push(semantic.clone());
        self.interned.insert(semantic, id);
        id
    }

    fn validate_ids(&self, ids: impl IntoIterator<Item = TypeId>) -> Result<(), TypeArenaError> {
        for id in ids {
            if self.get(id).is_none() {
                return Err(TypeArenaError::UnknownTypeId(id));
            }
        }
        Ok(())
    }
}

impl Default for TypeArena {
    fn default() -> Self {
        Self::new()
    }
}

fn referenced_types(semantic: &SemanticType) -> Vec<TypeId> {
    match semantic {
        SemanticType::Primitive(_)
        | SemanticType::TaggedUnion { .. }
        | SemanticType::TypeParameter(_)
        | SemanticType::Opaque(_)
        | SemanticType::Error => Vec::new(),
        SemanticType::Tuple(elements)
        | SemanticType::Union(elements)
        | SemanticType::Attribute {
            parameters: elements,
            ..
        } => elements.clone(),
        SemanticType::Function {
            parameters,
            results,
            ..
        } => parameters.iter().chain(results).copied().collect(),
        SemanticType::Record(fields) => fields.iter().map(|(_, field_type)| *field_type).collect(),
        SemanticType::Array(element) | SemanticType::Optional(element) => vec![*element],
        SemanticType::Table { key, value } => vec![*key, *value],
        SemanticType::Class { arguments, .. }
        | SemanticType::Interface { arguments, .. }
        | SemanticType::Builtin { arguments, .. } => arguments.clone(),
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TypeArenaError {
    UnknownTypeId(TypeId),
}

impl fmt::Display for TypeArenaError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid type arena operation: {self:?}")
    }
}

impl Error for TypeArenaError {}
