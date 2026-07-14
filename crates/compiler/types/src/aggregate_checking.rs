//! Union, class, record, array, table, field, and update expression checking.
//!
//! Aggregate shapes remain distinct semantic types. Every field/case access
//! resolves to a stable typed ID; no table-like or string lookup fallback is
//! introduced by these shared checking mechanics.

use std::collections::BTreeMap;

use pop_diagnostics::{resolution as resolution_diagnostics, types as type_diagnostics};
use pop_foundation::{SourceSpan, TypeId};
use pop_resolve::SymbolSpace;
use pop_syntax::{ExpressionSyntax, ExpressionSyntaxKind, FieldInitializerSyntax};

use crate::SemanticType;
use crate::body_checking::{
    BodyChecker, ErrorCaseLookup, ExpectedExpressionType, UnionCaseLookup, typed_field_default,
};
use crate::typed_body::*;

impl<'resolver, 'index> BodyChecker<'resolver, 'index> {
    pub(crate) fn lookup_error_case(
        &mut self,
        path: &[String],
        span: SourceSpan,
    ) -> ErrorCaseLookup {
        if path.len() < 2 {
            return ErrorCaseLookup::NotError;
        }
        let type_name = path[..path.len() - 1].join(".");
        let resolution =
            self.resolver
                .database()
                .resolve(self.module, &type_name, SymbolSpace::Type, span);
        let Some(symbol) = resolution.symbol() else {
            return ErrorCaseLookup::NotError;
        };
        let Some(definition) = self.resolver.error_definition(symbol).cloned() else {
            return ErrorCaseLookup::NotError;
        };
        let case_name = &path[path.len() - 1];
        let Some(case) = definition
            .cases()
            .iter()
            .find(|case| case.name() == case_name)
            .cloned()
        else {
            self.diagnostics
                .push(resolution_diagnostics::unknown_name(span, path.join(".")));
            return ErrorCaseLookup::Missing;
        };
        ErrorCaseLookup::Found(definition, case)
    }

    pub(crate) fn check_error_case_call(
        &mut self,
        definition: &crate::ErrorDefinition,
        case: &crate::ErrorCaseDefinition,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if case.parameters().len() != arguments.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "error case",
                case.parameters().len(),
                arguments.len(),
            ));
            return None;
        }
        let mut typed_arguments = Vec::new();
        for (argument, (_, parameter_type, parameter_span)) in
            arguments.iter().zip(case.parameters())
        {
            let typed = self.check_expression_expected(
                argument,
                Some(ExpectedExpressionType::plain(*parameter_type)),
            )?;
            self.require_same_type(
                *parameter_type,
                typed.type_id(),
                typed.span(),
                *parameter_span,
            );
            typed_arguments.push(typed);
        }
        Some(TypedExpression {
            kind: TypedExpressionKind::ErrorCase {
                error: definition.error(),
                case: case.case(),
                arguments: typed_arguments,
            },
            type_id: definition.type_id(),
            span,
        })
    }

    pub(crate) fn lookup_union_case(
        &mut self,
        path: &[String],
        span: SourceSpan,
    ) -> UnionCaseLookup {
        if path.len() < 2 {
            return UnionCaseLookup::NotUnion;
        }
        let type_name = path[..path.len() - 1].join(".");
        let resolution =
            self.resolver
                .database()
                .resolve(self.module, &type_name, SymbolSpace::Type, span);
        let Some(symbol) = resolution.symbol() else {
            return UnionCaseLookup::NotUnion;
        };
        let Some(definition) = self.resolver.union_definition(symbol).cloned() else {
            return UnionCaseLookup::NotUnion;
        };
        let case_name = &path[path.len() - 1];
        let Some(case) = definition
            .cases()
            .iter()
            .find(|case| case.name() == case_name)
            .cloned()
        else {
            self.diagnostics
                .push(resolution_diagnostics::unknown_name(span, path.join(".")));
            return UnionCaseLookup::Missing;
        };
        UnionCaseLookup::Found(definition, case)
    }

    pub(crate) fn check_union_case_call(
        &mut self,
        definition: &crate::UnionDefinition,
        case: &crate::UnionCaseDefinition,
        arguments: &[ExpressionSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        if case.parameters().len() != arguments.len() {
            self.diagnostics.push(type_diagnostics::wrong_value_arity(
                span,
                "union case",
                case.parameters().len(),
                arguments.len(),
            ));
            return None;
        }
        let mut typed_arguments = Vec::new();
        for (argument, (_, parameter_type, parameter_span)) in
            arguments.iter().zip(case.parameters())
        {
            let typed = self.check_expression_expected(
                argument,
                Some(ExpectedExpressionType::plain(*parameter_type)),
            )?;
            self.require_same_type(
                *parameter_type,
                typed.type_id(),
                typed.span(),
                *parameter_span,
            );
            typed_arguments.push(typed);
        }
        Some(TypedExpression {
            kind: TypedExpressionKind::UnionCase {
                union: definition.symbol(),
                case: case.case(),
                arguments: typed_arguments,
            },
            type_id: definition.type_id(),
            span,
        })
    }

    pub(crate) fn check_class_construct(
        &mut self,
        type_name: &[String],
        fields: &[FieldInitializerSyntax],
        expected: Option<TypeId>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let resolution = self.resolver.database().resolve(
            self.module,
            &type_name.join("."),
            SymbolSpace::Type,
            span,
        );
        if !resolution.diagnostics().is_empty() {
            self.diagnostics
                .extend(resolution.diagnostics().iter().cloned());
            return None;
        }
        let symbol = resolution.symbol()?;
        let Some(definition) = self.resolver.class_definition(symbol).cloned() else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "construct",
                type_name.join("."),
            ));
            return None;
        };
        if let Some(expected) = expected {
            self.require_same_type(expected, definition.type_id(), span, span);
        }
        let typed_fields = self.check_class_fields(&definition, fields, span)?;
        self.diagnostics.is_empty().then_some(TypedExpression {
            kind: TypedExpressionKind::ClassConstruct {
                class: definition.class(),
                definition: definition.symbol(),
                fields: typed_fields,
            },
            type_id: definition.type_id(),
            span,
        })
    }

    pub(crate) fn check_class_fields(
        &mut self,
        definition: &crate::ClassDefinition,
        fields: &[FieldInitializerSyntax],
        span: SourceSpan,
    ) -> Option<Vec<TypedFieldValue>> {
        let mut seen = BTreeMap::new();
        let mut typed = Vec::new();
        for field_syntax in fields {
            if let Some(original) = seen.insert(field_syntax.name().to_owned(), field_syntax.span())
            {
                self.diagnostics
                    .push(type_diagnostics::duplicate_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                        original,
                    ));
                continue;
            }
            let Some(field) = definition
                .fields()
                .iter()
                .find(|field| field.name() == field_syntax.name())
            else {
                self.diagnostics
                    .push(type_diagnostics::unknown_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                    ));
                continue;
            };
            if !self.can_access_class_member(definition, field.visibility()) {
                self.diagnostics
                    .push(resolution_diagnostics::inaccessible_name(
                        field_syntax.span(),
                        field.name(),
                        field.span(),
                    ));
                continue;
            }
            let value = self.check_expression_expected(
                field_syntax.value(),
                Some(ExpectedExpressionType::plain(field.field_type())),
            )?;
            self.require_same_type(
                field.field_type(),
                value.type_id(),
                value.span(),
                field.span(),
            );
            typed.push(TypedFieldValue {
                field: field.field(),
                value,
                span: field_syntax.span(),
            });
        }
        for field in definition.fields() {
            if seen.contains_key(field.name()) {
                continue;
            }
            if let Some(default) = field.default() {
                typed.push(TypedFieldValue {
                    field: field.field(),
                    value: typed_field_default(default, field.field_type(), field.span()),
                    span: field.span(),
                });
            } else {
                self.diagnostics
                    .push(type_diagnostics::missing_record_field(span, field.name()));
            }
        }
        self.diagnostics.is_empty().then_some(typed)
    }

    pub(crate) fn check_array_literal(
        &mut self,
        elements: &[ExpressionSyntax],
        expected: Option<TypeId>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let Some(expected) = expected else {
            self.diagnostics
                .push(type_diagnostics::aggregate_needs_context(span));
            return None;
        };
        let Some(SemanticType::Array(element_type)) = self.resolver.arena().get(expected).cloned()
        else {
            self.diagnostics
                .push(type_diagnostics::aggregate_needs_context(span));
            return None;
        };
        let mut typed_elements = Vec::with_capacity(elements.len());
        for element in elements {
            let typed = self.check_expression_expected(
                element,
                Some(ExpectedExpressionType::plain(element_type)),
            )?;
            self.require_same_type(element_type, typed.type_id(), typed.span(), span);
            typed_elements.push(typed);
        }
        self.diagnostics.is_empty().then_some(TypedExpression {
            kind: TypedExpressionKind::Array(typed_elements),
            type_id: expected,
            span,
        })
    }

    pub(crate) fn check_array_get(
        &mut self,
        base: &ExpressionSyntax,
        index: &ExpressionSyntax,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let base = self.check_expression(base)?;
        match self.resolver.arena().get(base.type_id()).cloned() {
            Some(SemanticType::Array(element_type)) => {
                let index_type = self.resolver.arena().source_type("Int")?;
                let typed_index = self.check_expression_expected(
                    index,
                    Some(ExpectedExpressionType::plain(index_type)),
                )?;
                self.require_same_type(index_type, typed_index.type_id(), typed_index.span(), span);
                let result_type = self.resolver.arena_mut().optional(element_type).ok()?;
                self.diagnostics.is_empty().then_some(TypedExpression {
                    kind: TypedExpressionKind::ArrayGet {
                        array: Box::new(base),
                        index: Box::new(typed_index),
                    },
                    type_id: result_type,
                    span,
                })
            }
            Some(SemanticType::Builtin {
                definition,
                arguments,
            }) if self
                .resolver
                .schema()
                .iteration_protocol()
                .is_some_and(|protocol| definition == protocol.list())
                && arguments.len() == 1 =>
            {
                let element_type = arguments[0];
                let index_type = self.resolver.arena().source_type("Int")?;
                let typed_index = self.check_expression_expected(
                    index,
                    Some(ExpectedExpressionType::plain(index_type)),
                )?;
                self.require_same_type(index_type, typed_index.type_id(), typed_index.span(), span);
                let result_type = self.resolver.arena_mut().optional(element_type).ok()?;
                self.diagnostics.is_empty().then_some(TypedExpression {
                    kind: TypedExpressionKind::ListGet {
                        list: Box::new(base),
                        index: Box::new(typed_index),
                    },
                    type_id: result_type,
                    span,
                })
            }
            Some(SemanticType::Tuple(elements)) => {
                let ExpressionSyntaxKind::Integer(spelling) = index.kind() else {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        index.span(),
                        "tuple index",
                        "non-literal index",
                    ));
                    return None;
                };
                let normalized = spelling.replace('_', "");
                let Ok(one_based) = normalized.parse::<usize>() else {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        index.span(),
                        "tuple index",
                        "invalid integer literal",
                    ));
                    return None;
                };
                let Some(zero_based) = one_based.checked_sub(1) else {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        index.span(),
                        "tuple index",
                        "index zero",
                    ));
                    return None;
                };
                let Some(element_type) = elements.get(zero_based).copied() else {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        index.span(),
                        "tuple index",
                        "index outside tuple arity",
                    ));
                    return None;
                };
                let index = u32::try_from(zero_based).ok()?;
                Some(TypedExpression {
                    kind: TypedExpressionKind::TupleGet {
                        tuple: Box::new(base),
                        index,
                    },
                    type_id: element_type,
                    span,
                })
            }
            Some(SemanticType::Table { key, value }) => {
                if !self.is_supported_table_key(key) {
                    self.diagnostics.push(type_diagnostics::invalid_operator(
                        index.span(),
                        "table index",
                        self.type_name(key),
                    ));
                    return None;
                }
                let typed_key = self
                    .check_expression_expected(index, Some(ExpectedExpressionType::plain(key)))?;
                self.require_same_type(key, typed_key.type_id(), typed_key.span(), span);
                let result_type = self.resolver.arena_mut().optional(value).ok()?;
                self.diagnostics.is_empty().then_some(TypedExpression {
                    kind: TypedExpressionKind::TableGet {
                        table: Box::new(base),
                        key: Box::new(typed_key),
                    },
                    type_id: result_type,
                    span,
                })
            }
            _ => {
                self.diagnostics.push(type_diagnostics::invalid_operator(
                    span,
                    "[]",
                    self.type_name(base.type_id()),
                ));
                None
            }
        }
    }

    fn is_supported_table_key(&self, type_id: TypeId) -> bool {
        matches!(
            self.resolver.arena().get(type_id),
            Some(SemanticType::Primitive(
                crate::PrimitiveType::Boolean
                    | crate::PrimitiveType::Integer(_)
                    | crate::PrimitiveType::String
            ))
        )
    }

    pub(crate) fn check_aggregate_literal(
        &mut self,
        fields: &[FieldInitializerSyntax],
        expected: Option<ExpectedExpressionType>,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let Some(expected) = expected else {
            self.diagnostics
                .push(type_diagnostics::aggregate_needs_context(span));
            return None;
        };
        let definition = expected
            .declaration
            .and_then(|symbol| self.resolver.record_definition(symbol))
            .filter(|definition| definition.type_id() == expected.type_id)
            .cloned()
            .or_else(|| {
                self.resolver
                    .record_definition_for_type(expected.type_id)
                    .cloned()
            });
        if let Some(definition) = definition {
            let typed_fields = self.check_record_fields(&definition, fields, true, span)?;
            return Some(TypedExpression {
                kind: TypedExpressionKind::Record {
                    record: definition.symbol(),
                    fields: typed_fields,
                },
                type_id: expected.type_id,
                span,
            });
        }
        match self.resolver.arena().get(expected.type_id).cloned() {
            Some(SemanticType::Array(_)) if fields.is_empty() => {
                self.check_array_literal(&[], Some(expected.type_id), span)
            }
            Some(SemanticType::Table { key, value }) => {
                self.check_named_table_literal(fields, expected.type_id, key, value, span)
            }
            _ => {
                self.diagnostics
                    .push(type_diagnostics::aggregate_needs_context(span));
                None
            }
        }
    }

    pub(crate) fn check_named_table_literal(
        &mut self,
        fields: &[FieldInitializerSyntax],
        table_type: TypeId,
        key_type: TypeId,
        value_type: TypeId,
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let string_type = self.resolver.arena().source_type("String")?;
        let mut entries = Vec::with_capacity(fields.len());
        for field in fields {
            let key = TypedExpression {
                kind: TypedExpressionKind::String(field.name().to_owned()),
                type_id: string_type,
                span: field.span(),
            };
            self.require_same_type(key_type, string_type, field.span(), span);
            let value = self.check_expression_expected(
                field.value(),
                Some(ExpectedExpressionType::plain(value_type)),
            )?;
            self.require_same_type(value_type, value.type_id(), value.span(), span);
            entries.push(TypedTableEntry {
                key,
                value,
                span: field.span(),
            });
        }
        self.diagnostics.is_empty().then_some(TypedExpression {
            kind: TypedExpressionKind::Table(entries),
            type_id: table_type,
            span,
        })
    }

    pub(crate) fn check_record_update(
        &mut self,
        base: &ExpressionSyntax,
        fields: &[FieldInitializerSyntax],
        span: SourceSpan,
    ) -> Option<TypedExpression> {
        let base = self.check_expression(base)?;
        let Some(definition) = self
            .resolver
            .record_definition_for_type(base.type_id())
            .cloned()
        else {
            self.diagnostics.push(type_diagnostics::invalid_operator(
                span,
                "with",
                self.type_name(base.type_id()),
            ));
            return None;
        };
        let typed_fields = self.check_record_fields(&definition, fields, false, span)?;
        let type_id = base.type_id();
        Some(TypedExpression {
            kind: TypedExpressionKind::RecordUpdate {
                record: definition.symbol(),
                base: Box::new(base),
                fields: typed_fields,
            },
            type_id,
            span,
        })
    }

    pub(crate) fn check_record_fields(
        &mut self,
        definition: &crate::RecordDefinition,
        fields: &[FieldInitializerSyntax],
        require_complete: bool,
        aggregate_span: SourceSpan,
    ) -> Option<Vec<TypedFieldValue>> {
        let mut seen = BTreeMap::new();
        let mut typed = Vec::new();
        for field_syntax in fields {
            if let Some(original) = seen.insert(field_syntax.name().to_owned(), field_syntax.span())
            {
                self.diagnostics
                    .push(type_diagnostics::duplicate_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                        original,
                    ));
                continue;
            }
            let Some(field) = definition
                .fields()
                .iter()
                .find(|field| field.name() == field_syntax.name())
            else {
                self.diagnostics
                    .push(type_diagnostics::unknown_record_field(
                        field_syntax.span(),
                        field_syntax.name(),
                    ));
                continue;
            };
            let value = self.check_expression_expected(
                field_syntax.value(),
                Some(ExpectedExpressionType::plain(field.field_type())),
            )?;
            self.require_same_type(
                field.field_type(),
                value.type_id(),
                value.span(),
                field.span(),
            );
            typed.push(TypedFieldValue {
                field: field.field(),
                value,
                span: field_syntax.span(),
            });
        }
        if require_complete {
            for field in definition.fields() {
                if seen.contains_key(field.name()) {
                    continue;
                }
                if let Some(default) = field.default() {
                    typed.push(TypedFieldValue {
                        field: field.field(),
                        value: typed_field_default(default, field.field_type(), field.span()),
                        span: field.span(),
                    });
                } else {
                    self.diagnostics
                        .push(type_diagnostics::missing_record_field(
                            aggregate_span,
                            field.name(),
                        ));
                }
            }
        }
        self.diagnostics.is_empty().then_some(typed)
    }
}
