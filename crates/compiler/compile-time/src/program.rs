//! Verified compile-time program assembly and typed expression validation.
//!
//! Program construction closes direct-call identities and validates every
//! compile-time expression before interpretation or cache publication.

use std::collections::BTreeMap;

use pop_foundation::{AttributeId, FieldId, FunctionId, LocalId, TypeId};
use pop_types::{
    AttributeConstant, AttributeQueryIndex, FloatKind, NumericConversionKind, PrimitiveType,
    ResolvedAttribute, SemanticType, TypeArena,
};

use crate::model::*;

#[derive(Clone, Debug)]
pub struct CompileTimeProgram {
    pub(crate) functions: Vec<CompileTimeFunction>,
    pub(crate) types: TypeArena,
    pub(crate) metadata: CompileTimeTypeMetadata,
    pub(crate) attribute_queries: Option<AttributeQueryIndex>,
}

impl CompileTimeProgram {
    /// Builds a deterministic compile-time program.
    ///
    /// # Errors
    ///
    /// Rejects duplicate functions, unknown calls, and statically inconsistent
    /// parameter, branch, call-result, and function-result types.
    pub fn new(
        functions: Vec<CompileTimeFunction>,
        types: &TypeArena,
    ) -> Result<Self, ProgramError> {
        Self::new_with_metadata(functions, types, CompileTimeTypeMetadata::new())
    }

    /// Builds a deterministic compile-time program with compiler-owned handle
    /// and aggregate schemas.
    ///
    /// # Errors
    ///
    /// Applies the same verification as [`Self::new`] and rejects every value
    /// that does not exactly match the supplied compile-time-only metadata.
    pub fn new_with_metadata(
        mut functions: Vec<CompileTimeFunction>,
        types: &TypeArena,
        metadata: CompileTimeTypeMetadata,
    ) -> Result<Self, ProgramError> {
        functions.sort_by_key(CompileTimeFunction::function);
        if let Some(pair) = functions
            .windows(2)
            .find(|pair| pair[0].function() == pair[1].function())
        {
            return Err(ProgramError::DuplicateFunction(pair[0].function()));
        }
        let signatures: BTreeMap<_, _> = functions
            .iter()
            .map(|function| {
                (
                    function.function(),
                    (function.parameters().to_vec(), function.result()),
                )
            })
            .collect();
        for function in &functions {
            for type_id in function
                .parameters()
                .iter()
                .copied()
                .chain(std::iter::once(function.result()))
            {
                if !types.is_valid_compile_time_type(type_id) {
                    return Err(ProgramError::InvalidType(type_id));
                }
            }
            ExpressionVerifier {
                parameters: function.parameters(),
                signatures: &signatures,
                types,
                metadata: &metadata,
            }
            .verify(function.body(), &BTreeMap::new())?;
            if function.body().type_id() != function.result() {
                return Err(ProgramError::TypeMismatch {
                    expected: function.result(),
                    found: function.body().type_id(),
                });
            }
        }
        Ok(Self {
            functions,
            types: types.clone(),
            metadata,
            attribute_queries: None,
        })
    }

    #[must_use]
    pub fn with_attribute_queries(mut self, queries: AttributeQueryIndex) -> Self {
        self.attribute_queries = Some(queries);
        self
    }

    #[must_use]
    pub fn functions(&self) -> &[CompileTimeFunction] {
        &self.functions
    }

    #[must_use]
    pub const fn types(&self) -> &TypeArena {
        &self.types
    }

    #[must_use]
    pub const fn metadata(&self) -> &CompileTimeTypeMetadata {
        &self.metadata
    }

    pub(crate) fn function(&self, id: FunctionId) -> Option<&CompileTimeFunction> {
        self.functions
            .binary_search_by_key(&id, CompileTimeFunction::function)
            .ok()
            .map(|index| &self.functions[index])
    }

    pub(crate) const fn attribute_queries(&self) -> Option<&AttributeQueryIndex> {
        self.attribute_queries.as_ref()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgramError {
    DuplicateFunction(FunctionId),
    InvalidType(TypeId),
    UnknownFunction(FunctionId),
    UnknownParameter(u32),
    UnknownLocal(LocalId),
    DuplicateLocal(LocalId),
    WrongArity {
        function: FunctionId,
        expected: usize,
        found: usize,
    },
    TypeMismatch {
        expected: TypeId,
        found: TypeId,
    },
    ValueTypeMismatch {
        expected: TypeId,
    },
    InvalidUnaryOperator {
        operator: CompileTimeUnaryOperator,
        operand: TypeId,
        result: TypeId,
    },
    InvalidBinaryOperator {
        operator: CompileTimeBinaryOperator,
        left: TypeId,
        right: TypeId,
        result: TypeId,
    },
    InvalidNumericConversion {
        conversion: NumericConversionKind,
        source: TypeId,
        target: TypeId,
    },
}

struct ExpressionVerifier<'program> {
    parameters: &'program [TypeId],
    signatures: &'program BTreeMap<FunctionId, (Vec<TypeId>, TypeId)>,
    types: &'program TypeArena,
    metadata: &'program CompileTimeTypeMetadata,
}

impl ExpressionVerifier<'_> {
    fn verify(
        &self,
        expression: &CompileTimeExpression,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        if !self.types.is_valid_hir_type(expression.type_id()) {
            return Err(ProgramError::InvalidType(expression.type_id()));
        }
        match expression.kind() {
            CompileTimeExpressionKind::Constant(value) => {
                if !value_matches_type(value, expression.type_id(), self.types, self.metadata) {
                    return Err(ProgramError::ValueTypeMismatch {
                        expected: expression.type_id(),
                    });
                }
            }
            CompileTimeExpressionKind::Parameter(index) => {
                let Some(parameter_type) = usize::try_from(*index)
                    .ok()
                    .and_then(|index| self.parameters.get(index))
                else {
                    return Err(ProgramError::UnknownParameter(*index));
                };
                require_type(*parameter_type, expression.type_id())?;
            }
            CompileTimeExpressionKind::Local(local) => {
                let Some(local_type) = locals.get(local) else {
                    return Err(ProgramError::UnknownLocal(*local));
                };
                require_type(*local_type, expression.type_id())?;
            }
            CompileTimeExpressionKind::Let {
                local,
                local_type,
                initializer,
                body,
            } => self.verify_let(expression, *local, *local_type, initializer, body, locals)?,
            CompileTimeExpressionKind::LetTuple {
                locals: bindings,
                initializer,
                body,
            } => self.verify_let_tuple(expression, bindings, initializer, body, locals)?,
            CompileTimeExpressionKind::Unary { operator, operand } => {
                self.verify(operand, locals)?;
                if !valid_unary_operator(
                    *operator,
                    operand.type_id(),
                    expression.type_id(),
                    self.types,
                ) {
                    return Err(ProgramError::InvalidUnaryOperator {
                        operator: *operator,
                        operand: operand.type_id(),
                        result: expression.type_id(),
                    });
                }
            }
            CompileTimeExpressionKind::Binary {
                operator,
                left,
                right,
            } => self.verify_binary(*operator, left, right, expression.type_id(), locals)?,
            CompileTimeExpressionKind::NumericConvert { conversion, value } => {
                self.verify(value, locals)?;
                if !valid_numeric_conversion(
                    *conversion,
                    value.type_id(),
                    expression.type_id(),
                    self.types,
                ) {
                    return Err(ProgramError::InvalidNumericConversion {
                        conversion: *conversion,
                        source: value.type_id(),
                        target: expression.type_id(),
                    });
                }
            }
            CompileTimeExpressionKind::Conditional {
                condition,
                when_true,
                when_false,
            } => {
                self.verify(condition, locals)?;
                self.verify(when_true, locals)?;
                self.verify(when_false, locals)?;
                require_type(boolean_type(self.types)?, condition.type_id())?;
                require_type(when_true.type_id(), when_false.type_id())?;
                require_type(expression.type_id(), when_true.type_id())?;
            }
            CompileTimeExpressionKind::Tuple(elements) => {
                self.verify_tuple(expression.type_id(), elements, locals)?;
            }
            CompileTimeExpressionKind::Call {
                function,
                arguments,
            } => self.verify_call(*function, arguments, expression.type_id(), locals)?,
            CompileTimeExpressionKind::AttributeQuery {
                attribute,
                has_only,
                ..
            } => {
                if *has_only {
                    require_type(boolean_type(self.types)?, expression.type_id())?;
                } else if !type_contains_attribute(self.types, expression.type_id(), *attribute) {
                    return Err(ProgramError::ValueTypeMismatch {
                        expected: expression.type_id(),
                    });
                }
            }
        }
        Ok(())
    }

    fn verify_let(
        &self,
        expression: &CompileTimeExpression,
        local: LocalId,
        local_type: TypeId,
        initializer: &CompileTimeExpression,
        body: &CompileTimeExpression,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        self.verify(initializer, locals)?;
        require_type(local_type, initializer.type_id())?;
        if locals.contains_key(&local) {
            return Err(ProgramError::DuplicateLocal(local));
        }
        let mut body_locals = locals.clone();
        body_locals.insert(local, local_type);
        self.verify(body, &body_locals)?;
        require_type(expression.type_id(), body.type_id())
    }

    fn verify_let_tuple(
        &self,
        expression: &CompileTimeExpression,
        bindings: &[(LocalId, TypeId)],
        initializer: &CompileTimeExpression,
        body: &CompileTimeExpression,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        self.verify(initializer, locals)?;
        let Some(SemanticType::Tuple(elements)) = self.types.get(initializer.type_id()) else {
            return Err(ProgramError::ValueTypeMismatch {
                expected: initializer.type_id(),
            });
        };
        if elements.len() != bindings.len()
            || elements
                .iter()
                .zip(bindings)
                .any(|(element, (_, binding_type))| element != binding_type)
        {
            return Err(ProgramError::ValueTypeMismatch {
                expected: initializer.type_id(),
            });
        }
        let mut body_locals = locals.clone();
        for (local, local_type) in bindings {
            if body_locals.insert(*local, *local_type).is_some() {
                return Err(ProgramError::DuplicateLocal(*local));
            }
        }
        self.verify(body, &body_locals)?;
        require_type(expression.type_id(), body.type_id())
    }

    fn verify_binary(
        &self,
        operator: CompileTimeBinaryOperator,
        left: &CompileTimeExpression,
        right: &CompileTimeExpression,
        result: TypeId,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        self.verify(left, locals)?;
        self.verify(right, locals)?;
        if valid_binary_operator(
            operator,
            left.type_id(),
            right.type_id(),
            result,
            self.types,
        ) {
            Ok(())
        } else {
            Err(ProgramError::InvalidBinaryOperator {
                operator,
                left: left.type_id(),
                right: right.type_id(),
                result,
            })
        }
    }

    fn verify_tuple(
        &self,
        result: TypeId,
        elements: &[CompileTimeExpression],
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        let Some(SemanticType::Tuple(element_types)) = self.types.get(result) else {
            return Err(ProgramError::ValueTypeMismatch { expected: result });
        };
        if element_types.len() != elements.len() {
            return Err(ProgramError::ValueTypeMismatch { expected: result });
        }
        for (element, expected) in elements.iter().zip(element_types) {
            self.verify(element, locals)?;
            require_type(*expected, element.type_id())?;
        }
        Ok(())
    }

    fn verify_call(
        &self,
        function: FunctionId,
        arguments: &[CompileTimeExpression],
        result_type: TypeId,
        locals: &BTreeMap<LocalId, TypeId>,
    ) -> Result<(), ProgramError> {
        let Some((parameter_types, result)) = self.signatures.get(&function) else {
            return Err(ProgramError::UnknownFunction(function));
        };
        if parameter_types.len() != arguments.len() {
            return Err(ProgramError::WrongArity {
                function,
                expected: parameter_types.len(),
                found: arguments.len(),
            });
        }
        for (argument, parameter_type) in arguments.iter().zip(parameter_types) {
            self.verify(argument, locals)?;
            require_type(*parameter_type, argument.type_id())?;
        }
        require_type(*result, result_type)
    }
}

pub(crate) fn value_matches_type(
    value: &CompileTimeValue,
    type_id: TypeId,
    types: &TypeArena,
    metadata: &CompileTimeTypeMetadata,
) -> bool {
    match (value, types.get(type_id)) {
        (
            CompileTimeValue::Nil,
            Some(SemanticType::Primitive(PrimitiveType::Nil) | SemanticType::Optional(_)),
        )
        | (CompileTimeValue::Boolean(_), Some(SemanticType::Primitive(PrimitiveType::Boolean)))
        | (CompileTimeValue::String(_), Some(SemanticType::Primitive(PrimitiveType::String))) => {
            true
        }
        (
            CompileTimeValue::Integer(value),
            Some(SemanticType::Primitive(PrimitiveType::Integer(kind))),
        ) => value.kind() == *kind,
        (CompileTimeValue::Float(value), Some(SemanticType::Primitive(PrimitiveType::Float32))) => {
            value.kind() == FloatKind::Float32
        }
        (CompileTimeValue::Float(value), Some(SemanticType::Primitive(PrimitiveType::Float64))) => {
            value.kind() == FloatKind::Float64
        }
        (CompileTimeValue::Tuple(values), Some(SemanticType::Tuple(element_types))) => {
            values.len() == element_types.len()
                && values
                    .iter()
                    .zip(element_types)
                    .all(|(value, type_id)| value_matches_type(value, *type_id, types, metadata))
        }
        (CompileTimeValue::Array(values), Some(SemanticType::Array(element_type))) => values
            .iter()
            .all(|value| value_matches_type(value, *element_type, types, metadata)),
        (
            CompileTimeValue::Attribute {
                attribute: value_attribute,
                arguments,
            },
            Some(SemanticType::Attribute {
                attribute,
                parameters,
            }),
        ) => {
            value_attribute == attribute
                && arguments.len() == parameters.len()
                && arguments.iter().zip(parameters).all(|(value, parameter)| {
                    value_matches_type(value, *parameter, types, metadata)
                })
        }
        (CompileTimeValue::Record(values), Some(SemanticType::Record(semantic_fields))) => {
            let Some(fields) = metadata.record(type_id) else {
                return false;
            };
            if values.len() != fields.len()
                || fields.windows(2).any(|pair| pair[0].0 == pair[1].0)
                || !record_metadata_matches_semantic_type(fields, semantic_fields)
            {
                return false;
            }
            values
                .iter()
                .zip(fields)
                .all(|((value_field, value), (field, field_type))| {
                    value_field == field && value_matches_type(value, *field_type, types, metadata)
                })
        }
        (
            CompileTimeValue::Union {
                union,
                case,
                arguments,
            },
            Some(SemanticType::TaggedUnion { definition }),
        ) => {
            let Some(argument_types) = metadata.union_case(*union, *case) else {
                return false;
            };
            union == definition
                && arguments.len() == argument_types.len()
                && arguments
                    .iter()
                    .zip(argument_types)
                    .all(|(argument, argument_type)| {
                        value_matches_type(argument, *argument_type, types, metadata)
                    })
        }
        (CompileTimeValue::TypeReference(referenced), Some(SemanticType::Opaque(_))) => {
            metadata.handle_kind(type_id) == Some(CompileTimeHandleKind::Type)
                && types.get(*referenced).is_some()
                && metadata.contains_type(*referenced)
        }
        (CompileTimeValue::SymbolReference(symbol), Some(SemanticType::Opaque(_))) => {
            metadata.handle_kind(type_id) == Some(CompileTimeHandleKind::Symbol)
                && metadata.contains_symbol(*symbol)
        }
        (value, Some(SemanticType::Optional(inner))) => {
            value_matches_type(value, *inner, types, metadata)
        }
        (_, Some(SemanticType::Union(members))) => members
            .iter()
            .any(|member| value_matches_type(value, *member, types, metadata)),
        _ => false,
    }
}

fn type_contains_attribute(types: &TypeArena, type_id: TypeId, attribute: AttributeId) -> bool {
    match types.get(type_id) {
        Some(SemanticType::Attribute {
            attribute: found, ..
        }) => *found == attribute,
        Some(SemanticType::Array(element) | SemanticType::Optional(element)) => {
            type_contains_attribute(types, *element, attribute)
        }
        Some(SemanticType::Union(members)) => members
            .iter()
            .any(|member| type_contains_attribute(types, *member, attribute)),
        _ => false,
    }
}

pub(crate) fn resolved_attribute_value(attribute: &ResolvedAttribute) -> CompileTimeValue {
    CompileTimeValue::Attribute {
        attribute: attribute.attribute(),
        arguments: attribute
            .arguments()
            .iter()
            .map(|argument| attribute_constant_value(argument.value()))
            .collect(),
    }
}

fn attribute_constant_value(value: &AttributeConstant) -> CompileTimeValue {
    match value {
        AttributeConstant::Nil => CompileTimeValue::Nil,
        AttributeConstant::Boolean(value) => CompileTimeValue::Boolean(*value),
        AttributeConstant::Integer(value) => CompileTimeValue::Integer(*value),
        AttributeConstant::Float(value) => CompileTimeValue::Float(*value),
        AttributeConstant::String(value) => CompileTimeValue::String(value.clone()),
        AttributeConstant::Tuple(values) => {
            CompileTimeValue::Tuple(values.iter().map(attribute_constant_value).collect())
        }
    }
}

fn record_metadata_matches_semantic_type(
    fields: &[(FieldId, TypeId)],
    semantic_fields: &[(String, TypeId)],
) -> bool {
    if fields.len() != semantic_fields.len() {
        return false;
    }
    let mut metadata_types: Vec<_> = fields.iter().map(|(_, type_id)| *type_id).collect();
    let mut declared_types: Vec<_> = semantic_fields
        .iter()
        .map(|(_, type_id)| *type_id)
        .collect();
    metadata_types.sort_unstable();
    declared_types.sort_unstable();
    metadata_types == declared_types
}

fn valid_unary_operator(
    operator: CompileTimeUnaryOperator,
    operand: TypeId,
    result: TypeId,
    types: &TypeArena,
) -> bool {
    operand == result
        && match operator {
            CompileTimeUnaryOperator::CheckedIntegerNegate => {
                matches!(
                    types.get(operand),
                    Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) if kind.is_signed()
                )
            }
            CompileTimeUnaryOperator::FloatNegate => is_float_type(types, operand),
            CompileTimeUnaryOperator::BooleanNot => boolean_type(types) == Ok(operand),
        }
}

fn valid_binary_operator(
    operator: CompileTimeBinaryOperator,
    left: TypeId,
    right: TypeId,
    result: TypeId,
    types: &TypeArena,
) -> bool {
    if left != right {
        return false;
    }
    match operator {
        CompileTimeBinaryOperator::CheckedAdd
        | CompileTimeBinaryOperator::CheckedSubtract
        | CompileTimeBinaryOperator::CheckedMultiply
        | CompileTimeBinaryOperator::CheckedDivide
        | CompileTimeBinaryOperator::CheckedRemainder => {
            left == result && is_integer_type(types, left)
        }
        CompileTimeBinaryOperator::FloatAdd
        | CompileTimeBinaryOperator::FloatSubtract
        | CompileTimeBinaryOperator::FloatMultiply
        | CompileTimeBinaryOperator::FloatDivide => left == result && is_float_type(types, left),
        CompileTimeBinaryOperator::LessThan
        | CompileTimeBinaryOperator::LessThanOrEqual
        | CompileTimeBinaryOperator::GreaterThan
        | CompileTimeBinaryOperator::GreaterThanOrEqual => {
            (is_integer_type(types, left) || is_float_type(types, left))
                && boolean_type(types) == Ok(result)
        }
        CompileTimeBinaryOperator::Equal | CompileTimeBinaryOperator::NotEqual => {
            boolean_type(types) == Ok(result) && supports_compile_time_equality(types, left)
        }
        CompileTimeBinaryOperator::And | CompileTimeBinaryOperator::Or => {
            boolean_type(types) == Ok(left) && left == result
        }
    }
}

fn valid_numeric_conversion(
    conversion: NumericConversionKind,
    source: TypeId,
    target: TypeId,
    types: &TypeArena,
) -> bool {
    match conversion {
        NumericConversionKind::IntegerToInteger {
            source: source_kind,
            target: target_kind,
        } => {
            integer_kind(types, source) == Some(source_kind)
                && integer_kind(types, target) == Some(target_kind)
        }
        NumericConversionKind::IntegerToFloat {
            source: source_kind,
            target: target_kind,
        } => {
            integer_kind(types, source) == Some(source_kind)
                && float_kind(types, target) == Some(target_kind)
        }
        NumericConversionKind::FloatToInteger {
            source: source_kind,
            target: target_kind,
        } => {
            float_kind(types, source) == Some(source_kind)
                && integer_kind(types, target) == Some(target_kind)
        }
        NumericConversionKind::FloatToFloat {
            source: source_kind,
            target: target_kind,
        } => {
            float_kind(types, source) == Some(source_kind)
                && float_kind(types, target) == Some(target_kind)
        }
    }
}

fn integer_kind(types: &TypeArena, type_id: TypeId) -> Option<pop_types::IntegerKind> {
    match types.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Integer(kind))) => Some(*kind),
        _ => None,
    }
}

fn float_kind(types: &TypeArena, type_id: TypeId) -> Option<FloatKind> {
    match types.get(type_id) {
        Some(SemanticType::Primitive(PrimitiveType::Float32)) => Some(FloatKind::Float32),
        Some(SemanticType::Primitive(PrimitiveType::Float64)) => Some(FloatKind::Float64),
        _ => None,
    }
}

fn supports_compile_time_equality(types: &TypeArena, type_id: TypeId) -> bool {
    match types.get(type_id) {
        Some(SemanticType::Primitive(
            PrimitiveType::Nil
            | PrimitiveType::Boolean
            | PrimitiveType::Integer(_)
            | PrimitiveType::String,
        )) => true,
        Some(SemanticType::Tuple(elements) | SemanticType::Union(elements)) => elements
            .iter()
            .all(|element| supports_compile_time_equality(types, *element)),
        Some(SemanticType::Record(fields)) => fields
            .iter()
            .all(|(_, field_type)| supports_compile_time_equality(types, *field_type)),
        _ => false,
    }
}

pub(crate) fn is_integer_type(types: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        types.get(type_id),
        Some(SemanticType::Primitive(PrimitiveType::Integer(_)))
    )
}

pub(crate) fn is_float_type(types: &TypeArena, type_id: TypeId) -> bool {
    matches!(
        types.get(type_id),
        Some(SemanticType::Primitive(
            PrimitiveType::Float32 | PrimitiveType::Float64
        ))
    )
}

fn boolean_type(types: &TypeArena) -> Result<TypeId, ProgramError> {
    types
        .source_type("Boolean")
        .ok_or(ProgramError::InvalidType(TypeId::from_raw(u32::MAX)))
}

fn require_type(expected: TypeId, found: TypeId) -> Result<(), ProgramError> {
    if expected == found {
        Ok(())
    } else {
        Err(ProgramError::TypeMismatch { expected, found })
    }
}
