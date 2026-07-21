use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use pop_foundation::{ClassId, TypeId};
use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

use crate::{PrimitiveType, SemanticType};
use pop_foundation::ParameterId;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TypeArena {
    types: Vec<SemanticType>,
    interned: BTreeMap<SemanticType, TypeId>,
    class_specializations: BTreeMap<(ClassId, Vec<TypeId>), TypeId>,
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
            class_specializations: BTreeMap::new(),
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
    pub fn view_kind(&self, type_id: TypeId) -> Option<crate::ViewKind> {
        match self.get(type_id) {
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if arguments.is_empty() => {
                if *definition == crate::BYTES_VIEW_TYPE_ID {
                    Some(crate::ViewKind::Bytes)
                } else if *definition == crate::TEXT_VIEW_TYPE_ID {
                    Some(crate::ViewKind::Text)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn contains_view(&self, type_id: TypeId) -> bool {
        self.view_kind(type_id).is_some()
            || self.get(type_id).is_some_and(|semantic| {
                referenced_types(semantic)
                    .into_iter()
                    .any(|nested| self.contains_view(nested))
            })
    }

    #[must_use]
    pub fn get(&self, id: TypeId) -> Option<&SemanticType> {
        self.types.get(id.raw() as usize)
    }

    #[must_use]
    pub fn find(&self, semantic: &SemanticType) -> Option<TypeId> {
        self.interned.get(semantic).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.types.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.types.is_empty()
    }

    #[must_use]
    pub fn contains_type_parameter(&self, type_id: TypeId) -> bool {
        match self.get(type_id) {
            Some(SemanticType::TypeParameter(_)) => true,
            Some(semantic) => referenced_types(semantic)
                .into_iter()
                .any(|nested| self.contains_type_parameter(nested)),
            None => false,
        }
    }

    #[must_use]
    pub fn substitute_existing(
        &self,
        type_id: TypeId,
        substitutions: &BTreeMap<ParameterId, TypeId>,
    ) -> Option<TypeId> {
        self.substitute_existing_with_classes(type_id, substitutions, &self.class_specializations)
    }

    /// Records the concrete nominal type selected for one generic class
    /// template and canonical argument list.
    pub fn register_class_specialization(
        &mut self,
        source: ClassId,
        arguments: Vec<TypeId>,
        concrete: TypeId,
    ) -> Result<(), TypeArenaError> {
        self.validate_ids(arguments.iter().copied().chain([concrete]))?;
        self.class_specializations
            .insert((source, arguments), concrete);
        Ok(())
    }

    pub fn class_specializations(&self) -> impl Iterator<Item = (ClassId, &[TypeId], TypeId)> {
        self.class_specializations
            .iter()
            .map(|((class, arguments), concrete)| (*class, arguments.as_slice(), *concrete))
    }

    #[must_use]
    pub fn substitute_existing_with_classes(
        &self,
        type_id: TypeId,
        substitutions: &BTreeMap<ParameterId, TypeId>,
        class_types: &BTreeMap<(ClassId, Vec<TypeId>), TypeId>,
    ) -> Option<TypeId> {
        let semantic = self.get(type_id)?.clone();
        if let SemanticType::TypeParameter(parameter) = semantic {
            return substitutions.get(&parameter).copied();
        }
        let map = |id| self.substitute_existing_with_classes(id, substitutions, class_types);
        let substituted = match semantic {
            SemanticType::Primitive(_)
            | SemanticType::Enum { .. }
            | SemanticType::Opaque(_)
            | SemanticType::Error => return Some(type_id),
            SemanticType::TypeParameter(_) => unreachable!("handled above"),
            SemanticType::TaggedUnion {
                definition,
                source,
                arguments,
            } => {
                let substituted_arguments =
                    arguments.into_iter().map(map).collect::<Option<Vec<_>>>()?;
                if let Some((_, concrete)) = self.interned.iter().find(|(semantic, _)| {
                    matches!(
                        semantic,
                        SemanticType::TaggedUnion {
                            source: candidate_source,
                            arguments: candidate_arguments,
                            ..
                        } if *candidate_source == source
                            && candidate_arguments == &substituted_arguments
                    )
                }) {
                    return Some(*concrete);
                }
                SemanticType::TaggedUnion {
                    definition,
                    source,
                    arguments: substituted_arguments,
                }
            }
            SemanticType::ErrorUnion {
                definition,
                source,
                arguments,
            } => SemanticType::ErrorUnion {
                definition,
                source,
                arguments: arguments.into_iter().map(map).collect::<Option<Vec<_>>>()?,
            },
            SemanticType::Tuple(values) => {
                SemanticType::Tuple(values.into_iter().map(map).collect::<Option<_>>()?)
            }
            SemanticType::Union(values) => {
                SemanticType::Union(values.into_iter().map(map).collect::<Option<_>>()?)
            }
            SemanticType::Record(fields) => SemanticType::Record(
                fields
                    .into_iter()
                    .map(|(name, value)| Some((name, map(value)?)))
                    .collect::<Option<_>>()?,
            ),
            SemanticType::Array(value) => SemanticType::Array(map(value)?),
            SemanticType::Table { key, value } => SemanticType::Table {
                key: map(key)?,
                value: map(value)?,
            },
            SemanticType::Optional(value) => SemanticType::Optional(map(value)?),
            SemanticType::Function {
                is_async,
                parameters,
                results,
                effects,
                lifetime_summary,
            } => SemanticType::Function {
                is_async,
                parameters: parameters.into_iter().map(map).collect::<Option<_>>()?,
                results: results.into_iter().map(map).collect::<Option<_>>()?,
                effects,
                lifetime_summary,
            },
            SemanticType::Class { class, arguments } => {
                let arguments = arguments.into_iter().map(map).collect::<Option<Vec<_>>>()?;
                if let Some(concrete) = class_types.get(&(class, arguments.clone())) {
                    return Some(*concrete);
                }
                if let Some(source) = class_types
                    .iter()
                    .find_map(|((source, _), instance)| (*instance == type_id).then_some(*source))
                    && let Some(concrete) = class_types.get(&(source, arguments.clone()))
                {
                    return Some(*concrete);
                }
                SemanticType::Class { class, arguments }
            }
            SemanticType::Interface {
                interface,
                arguments,
            } => SemanticType::Interface {
                interface,
                arguments: arguments.into_iter().map(map).collect::<Option<_>>()?,
            },
            SemanticType::Builtin {
                definition,
                arguments,
            } => SemanticType::Builtin {
                definition,
                arguments: arguments.into_iter().map(map).collect::<Option<_>>()?,
            },
            SemanticType::Attribute {
                attribute,
                parameters,
            } => SemanticType::Attribute {
                attribute,
                parameters: parameters.into_iter().map(map).collect::<Option<_>>()?,
            },
        };
        self.find(&substituted).or_else(|| {
            let (source, arguments, is_error) = match &substituted {
                SemanticType::TaggedUnion {
                    source, arguments, ..
                } => (*source, arguments, false),
                SemanticType::ErrorUnion {
                    source, arguments, ..
                } => (*source, arguments, true),
                _ => return None,
            };
            self.interned
                .iter()
                .find_map(|(candidate, type_id)| match candidate {
                    SemanticType::TaggedUnion {
                        source: candidate_source,
                        arguments: candidate_arguments,
                        ..
                    } if !is_error
                        && *candidate_source == source
                        && candidate_arguments == arguments =>
                    {
                        Some(*type_id)
                    }
                    SemanticType::ErrorUnion {
                        source: candidate_source,
                        arguments: candidate_arguments,
                        ..
                    } if is_error
                        && *candidate_source == source
                        && candidate_arguments == arguments =>
                    {
                        Some(*type_id)
                    }
                    _ => None,
                })
        })
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

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TypeArenaSnapshot {
    types: Vec<SemanticType>,
    class_specializations: Vec<(ClassId, Vec<TypeId>, TypeId)>,
}

impl Serialize for TypeArena {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        TypeArenaSnapshot {
            types: self.types.clone(),
            class_specializations: self
                .class_specializations()
                .map(|(class, arguments, concrete)| (class, arguments.to_vec(), concrete))
                .collect(),
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for TypeArena {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let snapshot = TypeArenaSnapshot::deserialize(deserializer)?;
        let mut arena = Self::new();
        if snapshot.types.len() < arena.types.len()
            || snapshot.types[..arena.types.len()] != arena.types
        {
            return Err(de::Error::custom("invalid primitive type prefix"));
        }
        for (raw, semantic) in snapshot
            .types
            .into_iter()
            .enumerate()
            .skip(arena.types.len())
        {
            let id = arena
                .intern(semantic)
                .map_err(|_| de::Error::custom("invalid type dependency"))?;
            if id.raw() as usize != raw {
                return Err(de::Error::custom("noncanonical type arena"));
            }
        }
        for (class, arguments, concrete) in snapshot.class_specializations {
            arena
                .register_class_specialization(class, arguments, concrete)
                .map_err(|_| de::Error::custom("invalid class specialization"))?;
        }
        Ok(arena)
    }
}

fn referenced_types(semantic: &SemanticType) -> Vec<TypeId> {
    match semantic {
        SemanticType::Primitive(_)
        | SemanticType::Enum { .. }
        | SemanticType::TypeParameter(_)
        | SemanticType::Opaque(_)
        | SemanticType::Error => Vec::new(),
        SemanticType::TaggedUnion { arguments, .. }
        | SemanticType::ErrorUnion { arguments, .. } => arguments.clone(),
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
