use pop_foundation::{FieldId, SymbolId, TypeId};
use pop_syntax::ExpressionSyntax;

use crate::{AttributeConstant, FieldDefault, FloatKind, PrimitiveType, SemanticType, TypeArena};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct AttributeParameterId(u32);

impl AttributeParameterId {
    #[must_use]
    pub const fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    #[must_use]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PendingConstantExpression {
    expression: ExpressionSyntax,
    expected_type: TypeId,
}

impl PendingConstantExpression {
    pub(crate) const fn new(expression: ExpressionSyntax, expected_type: TypeId) -> Self {
        Self {
            expression,
            expected_type,
        }
    }

    #[must_use]
    pub const fn expression(&self) -> &ExpressionSyntax {
        &self.expression
    }

    #[must_use]
    pub const fn expected_type(&self) -> TypeId {
        self.expected_type
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredConstantTarget {
    AttributeParameter {
        definition: SymbolId,
        parameter: AttributeParameterId,
    },
    RecordField {
        definition: SymbolId,
        field: FieldId,
    },
    ClassField {
        definition: SymbolId,
        field: FieldId,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RequiredConstantError {
    UnknownTarget(RequiredConstantTarget),
    NoPendingDefault(RequiredConstantTarget),
    TypeMismatch {
        target: RequiredConstantTarget,
        expected: TypeId,
    },
}

pub(crate) fn attribute_constant_matches_type(
    arena: &TypeArena,
    value: &AttributeConstant,
    expected: TypeId,
) -> bool {
    match arena.get(expected) {
        Some(SemanticType::Union(members)) => members
            .iter()
            .any(|member| attribute_constant_matches_type(arena, value, *member)),
        Some(SemanticType::Optional(inner)) => {
            matches!(value, AttributeConstant::Nil)
                || attribute_constant_matches_type(arena, value, *inner)
        }
        Some(SemanticType::Tuple(element_types)) => {
            let AttributeConstant::Tuple(elements) = value else {
                return false;
            };
            elements.len() == element_types.len()
                && elements
                    .iter()
                    .zip(element_types)
                    .all(|(element, expected)| {
                        attribute_constant_matches_type(arena, element, *expected)
                    })
        }
        Some(SemanticType::Primitive(primitive)) => {
            attribute_constant_matches_primitive(value, *primitive)
        }
        _ => false,
    }
}

fn attribute_constant_matches_primitive(
    value: &AttributeConstant,
    expected: PrimitiveType,
) -> bool {
    match (value, expected) {
        (AttributeConstant::Nil, PrimitiveType::Nil)
        | (AttributeConstant::Boolean(_), PrimitiveType::Boolean)
        | (AttributeConstant::String(_), PrimitiveType::String) => true,
        (AttributeConstant::Integer(value), PrimitiveType::Integer(kind)) => value.kind() == kind,
        (AttributeConstant::Float(value), PrimitiveType::Float32) => {
            value.kind() == FloatKind::Float32
        }
        (AttributeConstant::Float(value), PrimitiveType::Float64) => {
            value.kind() == FloatKind::Float64
        }
        _ => false,
    }
}

pub(crate) fn field_default_matches_type(
    arena: &TypeArena,
    value: &FieldDefault,
    expected: TypeId,
) -> bool {
    match arena.get(expected) {
        Some(SemanticType::Union(members)) => members
            .iter()
            .any(|member| field_default_matches_type(arena, value, *member)),
        Some(SemanticType::Optional(inner)) => {
            matches!(value, FieldDefault::Nil) || field_default_matches_type(arena, value, *inner)
        }
        Some(SemanticType::Primitive(primitive)) => match (value, primitive) {
            (FieldDefault::Nil, PrimitiveType::Nil)
            | (FieldDefault::Boolean(_), PrimitiveType::Boolean)
            | (FieldDefault::String(_), PrimitiveType::String) => true,
            (FieldDefault::Integer(value), PrimitiveType::Integer(kind)) => value.kind() == *kind,
            (FieldDefault::Float(value), PrimitiveType::Float32) => {
                value.kind() == FloatKind::Float32
            }
            (FieldDefault::Float(value), PrimitiveType::Float64) => {
                value.kind() == FloatKind::Float64
            }
            _ => false,
        },
        _ => false,
    }
}
