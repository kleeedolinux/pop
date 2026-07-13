//! Source-integrated UDA contract and attachment orchestration.
//!
//! This phase resolves trusted compiler-attribute identities, validates
//! usage/validator contracts, evaluates canonical arguments, and publishes
//! an immutable query index. It cannot create declarations or alter binding.

use std::collections::{BTreeMap, BTreeSet};

use pop_compile_time::CompileTimeValue;
use pop_diagnostics::compile_time as compile_time_diagnostics;
use pop_foundation::{Diagnostic, FunctionId, ModuleId, SymbolId, TypeId};
use pop_resolve::{ResolutionDatabase, SymbolSpace};
use pop_syntax::{AttributeUseSyntax, ExpressionSyntax, ExpressionSyntaxKind};
use pop_types::{
    AttributeAttachmentError, AttributeConstant, AttributeQueryIndex, AttributeTarget,
    AttributeUsage, AttributeValidator, BootstrapSchema, CompilerAttributeRole, ResolvedAttribute,
    ResolvedFunctionSignature, SignatureResolver, TypeArena,
};

use crate::api::FrontEndCompileTimeEvaluation;
use crate::compile_time::{
    compile_time_attribute_constant, compile_time_function_name, evaluate_compile_time_function,
    evaluate_required_expression,
};
use crate::work::{
    AttributeResolutionContext, CompileTimeContext, DeclarationAttributeWork, FunctionWork,
};

pub(crate) fn classify_function_attributes(
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    module: ModuleId,
    attributes: Vec<AttributeUseSyntax>,
) -> (bool, Vec<AttributeUseSyntax>) {
    let mut marked = false;
    let mut ordinary = Vec::new();
    for attribute in attributes {
        if trusted_compiler_attribute_role(database, bootstrap, module, &attribute)
            == Some(CompilerAttributeRole::CompileTime)
            && attribute.arguments().is_empty()
        {
            marked = true;
        } else {
            ordinary.push(attribute);
        }
    }
    (marked, ordinary)
}

fn trusted_compiler_attribute_role(
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    module: ModuleId,
    attribute: &AttributeUseSyntax,
) -> Option<CompilerAttributeRole> {
    let [name] = attribute.path() else {
        return None;
    };
    let entry = bootstrap.compiler_attribute_by_source_name(name)?;
    let shadowed = database
        .resolve(module, name, SymbolSpace::Type, attribute.span())
        .symbol()
        .is_some();
    (!shadowed).then(|| entry.role())
}

pub(crate) fn resolve_attribute_contracts(
    work: &mut [DeclarationAttributeWork],
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    compile_time: &CompileTimeContext,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for declaration in work {
        if declaration.target != AttributeTarget::Attribute {
            continue;
        }
        let mut ordinary = Vec::new();
        for syntax in std::mem::take(&mut declaration.attribute_uses) {
            match trusted_compiler_attribute_role(database, bootstrap, declaration.module, &syntax)
            {
                Some(CompilerAttributeRole::AttributeUsage) => {
                    match parse_attribute_usage_contract(database, declaration.module, &syntax) {
                        Some(usage) => {
                            if resolver
                                .install_attribute_usage(declaration.symbol, usage)
                                .is_err()
                            {
                                diagnostics.push(
                                    compile_time_diagnostics::ineligible_constant_expression(
                                        syntax.span(),
                                        "duplicate AttributeUsage contract",
                                    ),
                                );
                            }
                        }
                        None => diagnostics.push(
                            compile_time_diagnostics::ineligible_constant_expression(
                                syntax.span(),
                                "AttributeUsage contract",
                            ),
                        ),
                    }
                }
                Some(CompilerAttributeRole::AttributeValidator) => {
                    let definition = resolver.attribute_definition(declaration.symbol).cloned();
                    let validator_symbol = syntax
                        .arguments()
                        .first()
                        .and_then(|argument| match argument.value().kind() {
                            ExpressionSyntaxKind::Name(path) => Some(path.join(".")),
                            _ => None,
                        })
                        .and_then(|name| {
                            database
                                .resolve(
                                    declaration.module,
                                    &name,
                                    SymbolSpace::Value,
                                    syntax.span(),
                                )
                                .symbol()
                        });
                    match resolve_attribute_validator(
                        database,
                        declaration.module,
                        &syntax,
                        compile_time,
                        definition.as_ref(),
                        validator_symbol.and_then(|symbol| signatures.get(&symbol)),
                        resolver.arena().source_type("Boolean"),
                        diagnostics,
                    ) {
                        Some(validator) => {
                            if resolver
                                .install_attribute_validator(declaration.symbol, validator)
                                .is_err()
                            {
                                diagnostics.push(
                                    compile_time_diagnostics::ineligible_constant_expression(
                                        syntax.span(),
                                        "duplicate AttributeValidator contract",
                                    ),
                                );
                            }
                        }
                        None => diagnostics.push(
                            compile_time_diagnostics::ineligible_constant_expression(
                                syntax.span(),
                                "AttributeValidator function",
                            ),
                        ),
                    }
                }
                _ => ordinary.push(syntax),
            }
        }
        declaration.attribute_uses = ordinary;
    }
}

fn parse_attribute_usage_contract(
    database: &ResolutionDatabase,
    module: ModuleId,
    syntax: &AttributeUseSyntax,
) -> Option<AttributeUsage> {
    if syntax.arguments().len() != 2 {
        return None;
    }
    let mut targets = None;
    let mut repeatable = None;
    let mut next_positional = 0_usize;
    for argument in syntax.arguments() {
        let index = match argument.name() {
            Some("targets") => 0,
            Some("repeatable") => 1,
            Some(_) => return None,
            None => {
                let index = next_positional;
                next_positional = next_positional.saturating_add(1);
                index
            }
        };
        match index {
            0 if targets.is_none() => {
                targets = parse_attribute_targets(database, module, argument.value());
                targets.as_ref()?;
            }
            1 if repeatable.is_none() => {
                let ExpressionSyntaxKind::Boolean(value) = argument.value().kind() else {
                    return None;
                };
                repeatable = Some(*value);
            }
            _ => return None,
        }
    }
    Some(AttributeUsage::new(targets?, repeatable?))
}

fn parse_attribute_targets(
    database: &ResolutionDatabase,
    module: ModuleId,
    expression: &ExpressionSyntax,
) -> Option<Vec<AttributeTarget>> {
    if database
        .resolve(
            module,
            "AttributeTarget",
            SymbolSpace::Type,
            expression.span(),
        )
        .symbol()
        .is_some()
    {
        return None;
    }
    let ExpressionSyntaxKind::Array(elements) = expression.kind() else {
        return None;
    };
    elements
        .iter()
        .map(|element| {
            let ExpressionSyntaxKind::Name(path) = element.kind() else {
                return None;
            };
            let [owner, case] = path.as_slice() else {
                return None;
            };
            if owner != "AttributeTarget" {
                return None;
            }
            match case.as_str() {
                "Namespace" => Some(AttributeTarget::Namespace),
                "Function" => Some(AttributeTarget::Function),
                "Constant" => Some(AttributeTarget::Constant),
                "TypeAlias" => Some(AttributeTarget::TypeAlias),
                "Attribute" => Some(AttributeTarget::Attribute),
                "Record" => Some(AttributeTarget::Record),
                "Union" => Some(AttributeTarget::Union),
                "Class" => Some(AttributeTarget::Class),
                "Interface" => Some(AttributeTarget::Interface),
                "Enum" => Some(AttributeTarget::Enum),
                "Field" => Some(AttributeTarget::Field),
                "Case" => Some(AttributeTarget::Case),
                "Method" => Some(AttributeTarget::Method),
                _ => None,
            }
        })
        .collect()
}

#[allow(clippy::question_mark, clippy::too_many_arguments)]
fn resolve_attribute_validator(
    database: &ResolutionDatabase,
    module: ModuleId,
    syntax: &AttributeUseSyntax,
    compile_time: &CompileTimeContext,
    attribute: Option<&pop_types::AttributeDefinition>,
    source_signature: Option<&ResolvedFunctionSignature>,
    boolean: Option<TypeId>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<AttributeValidator> {
    let [argument] = syntax.arguments() else {
        return None;
    };
    if argument.name().is_some() {
        return None;
    }
    let ExpressionSyntaxKind::Name(path) = argument.value().kind() else {
        return None;
    };
    let name = path.join(".");
    let symbol = database
        .resolve(module, &name, SymbolSpace::Value, argument.value().span())
        .symbol()?;
    let function = FunctionId::from_raw(symbol.raw());
    let Some(attribute) = attribute else {
        return None;
    };
    let expected_parameters: Vec<_> = attribute
        .parameters()
        .iter()
        .map(pop_types::AttributeParameterDefinition::parameter_type)
        .collect();
    let valid_parameters = source_signature.is_some_and(|signature| {
        signature.parameters().len() == expected_parameters.len()
            && signature.parameters().iter().zip(&expected_parameters).all(
                |(parameter, expected)| parameter.parameter_type().type_id() == Some(*expected),
            )
    });
    let valid_result = source_signature.is_some_and(|signature| {
        signature.results().len() == 1
            && boolean.is_some_and(|boolean| signature.results()[0].type_id() == Some(boolean))
    });
    if !valid_parameters || !valid_result {
        diagnostics.push(
            compile_time_diagnostics::invalid_attribute_validator_signature(syntax.span(), name),
        );
        return None;
    }
    if !compile_time.eligible.contains(&function) {
        return None;
    }
    Some(AttributeValidator::new(function))
}

pub(crate) fn resolve_source_attributes(
    declarations: &mut [DeclarationAttributeWork],
    functions: &mut [FunctionWork],
    context: AttributeResolutionContext<'_>,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) -> AttributeQueryIndex {
    resolve_attribute_contracts(
        declarations,
        context.database,
        context.bootstrap,
        context.compile_time,
        context.signatures,
        resolver,
        diagnostics,
    );
    resolve_declaration_attributes(
        declarations,
        context.database,
        context.signatures,
        context.compile_time,
        resolver,
        compile_time_evaluations,
        diagnostics,
    );
    resolve_function_attributes(
        functions,
        context.database,
        context.signatures,
        context.compile_time,
        resolver,
        compile_time_evaluations,
        diagnostics,
    );
    build_attribute_query_index(declarations, functions, resolver)
}

fn resolve_function_attributes(
    functions: &mut [FunctionWork],
    database: &ResolutionDatabase,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for function in functions {
        let resolved_attributes = resolve_attribute_uses(
            function.module,
            AttributeTarget::Function,
            &function.attribute_uses,
            database,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
        let validated =
            resolver.validate_attribute_attachments(AttributeTarget::Function, resolved_attributes);
        diagnostics.extend(
            validated
                .errors()
                .iter()
                .map(attribute_attachment_diagnostic),
        );
        if let Some(attachments) = validated.attachment_set() {
            function.attributes = attachments.attachments().to_vec();
        }
    }
}

fn resolve_declaration_attributes(
    declarations: &mut [DeclarationAttributeWork],
    database: &ResolutionDatabase,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for declaration in declarations {
        let resolved_attributes = resolve_attribute_uses(
            declaration.module,
            declaration.target,
            &declaration.attribute_uses,
            database,
            signatures,
            compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
        let validated =
            resolver.validate_attribute_attachments(declaration.target, resolved_attributes);
        diagnostics.extend(
            validated
                .errors()
                .iter()
                .map(attribute_attachment_diagnostic),
        );
        if let Some(attachments) = validated.attachment_set() {
            declaration.attributes = attachments.attachments().to_vec();
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn resolve_attribute_uses(
    module: ModuleId,
    target: AttributeTarget,
    attribute_uses: &[AttributeUseSyntax],
    database: &ResolutionDatabase,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    compile_time: &CompileTimeContext,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<ResolvedAttribute> {
    let mut attributes = Vec::new();
    for syntax in attribute_uses {
        let name = syntax.path().join(".");
        let definition = database
            .resolve(module, &name, SymbolSpace::Type, syntax.span())
            .symbol()
            .and_then(|symbol| resolver.attribute_definition(symbol))
            .cloned();
        let Some(definition) = definition else {
            let result = resolver.resolve_attribute_use(module, syntax);
            diagnostics.extend(result.diagnostics().iter().cloned());
            continue;
        };
        if !definition.usage().allows(target) {
            diagnostics.push(attribute_attachment_diagnostic(
                &AttributeAttachmentError::WrongTarget {
                    attribute: definition.attribute(),
                    target,
                    span: syntax.span(),
                },
            ));
            continue;
        }
        let mut evaluated = Vec::new();
        let mut next_positional = 0;
        for argument in syntax.arguments() {
            let index = if let Some(name) = argument.name() {
                definition
                    .parameters()
                    .iter()
                    .position(|parameter| parameter.name() == name)
            } else {
                let index = next_positional;
                next_positional += 1;
                Some(index)
            };
            let Some(expected) = index
                .and_then(|index| definition.parameters().get(index))
                .map(pop_types::AttributeParameterDefinition::parameter_type)
            else {
                continue;
            };
            let value = evaluate_required_expression(
                module,
                argument.value(),
                expected,
                signatures,
                compile_time,
                resolver,
                compile_time_evaluations,
            )
            .and_then(|value| {
                compile_time_attribute_constant(value).ok_or_else(|| {
                    vec![compile_time_diagnostics::ineligible_constant_expression(
                        argument.value().span(),
                        "attribute argument",
                    )]
                })
            });
            evaluated.push((argument.value().span(), expected, value));
        }
        let result = resolver.resolve_attribute_use_with_evaluator(
            module,
            syntax,
            |expression, expected| {
                evaluated
                    .iter()
                    .find(|(span, cached_expected, _)| {
                        *span == expression.span() && *cached_expected == expected
                    })
                    .map_or_else(
                        || {
                            Err(vec![
                                compile_time_diagnostics::ineligible_constant_expression(
                                    expression.span(),
                                    "attribute argument",
                                ),
                            ])
                        },
                        |(_, _, value)| value.clone(),
                    )
            },
        );
        diagnostics.extend(result.diagnostics().iter().cloned());
        if let Some(attribute) = result.attribute() {
            if validate_resolved_attribute(
                attribute,
                &definition,
                compile_time,
                resolver.arena(),
                compile_time_evaluations,
                diagnostics,
            ) {
                attributes.push(attribute.clone());
            }
        }
    }
    attributes
}

fn validate_resolved_attribute(
    attribute: &ResolvedAttribute,
    definition: &pop_types::AttributeDefinition,
    compile_time: &CompileTimeContext,
    types: &TypeArena,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) -> bool {
    let Some(validator) = definition.validator() else {
        return true;
    };
    let arguments: Vec<_> = attribute
        .arguments()
        .iter()
        .map(|argument| attribute_constant_compile_time_value(argument.value()))
        .collect();
    let result = evaluate_compile_time_function(
        validator.function(),
        &arguments,
        attribute.span(),
        compile_time,
        types,
        compile_time_evaluations,
    );
    match result {
        Ok(CompileTimeValue::Boolean(true)) => true,
        Ok(CompileTimeValue::Boolean(false)) => {
            diagnostics.push(compile_time_diagnostics::attribute_validator_rejected(
                attribute.span(),
                format!("attribute#{}", attribute.attribute().raw()),
                [],
            ));
            false
        }
        Ok(_) => {
            diagnostics.push(
                compile_time_diagnostics::invalid_attribute_validator_signature(
                    attribute.span(),
                    compile_time_function_name(&compile_time.names, validator.function()),
                ),
            );
            false
        }
        Err(errors) => {
            diagnostics.extend(errors);
            false
        }
    }
}

fn attribute_constant_compile_time_value(value: &AttributeConstant) -> CompileTimeValue {
    match value {
        AttributeConstant::Nil => CompileTimeValue::Nil,
        AttributeConstant::Boolean(value) => CompileTimeValue::Boolean(*value),
        AttributeConstant::Integer(value) => CompileTimeValue::Integer(*value),
        AttributeConstant::Float(value) => CompileTimeValue::Float(*value),
        AttributeConstant::String(value) => CompileTimeValue::String(value.clone()),
        AttributeConstant::Tuple(values) => CompileTimeValue::Tuple(
            values
                .iter()
                .map(attribute_constant_compile_time_value)
                .collect(),
        ),
    }
}

fn attribute_attachment_diagnostic(error: &AttributeAttachmentError) -> Diagnostic {
    let (span, context) = match error {
        AttributeAttachmentError::UnknownAttribute { attribute, span } => {
            (*span, format!("unknown attribute#{}", attribute.raw()))
        }
        AttributeAttachmentError::WrongTarget {
            attribute,
            target,
            span,
        } => (
            *span,
            format!("attribute#{} cannot target {target:?}", attribute.raw()),
        ),
        AttributeAttachmentError::NonRepeatableDuplicate {
            attribute,
            duplicate,
            ..
        } => (
            *duplicate,
            format!("attribute#{} is not repeatable", attribute.raw()),
        ),
    };
    compile_time_diagnostics::ineligible_constant_expression(span, context)
}

fn build_attribute_query_index(
    declarations: &[DeclarationAttributeWork],
    functions: &[FunctionWork],
    resolver: &SignatureResolver<'_>,
) -> AttributeQueryIndex {
    let mut index = resolver.attribute_query_index();
    let mut indexed_types = BTreeSet::new();
    for declaration in declarations {
        let validated = resolver.validate_attribute_attachments(
            declaration.target,
            declaration.attributes.iter().cloned(),
        );
        if let Some(attachments) = validated.attachment_set() {
            index
                .insert_symbol(declaration.symbol, attachments.clone())
                .expect("validated declaration has one indexed resolver symbol");
            if let Some(type_id) = resolver.declaration_type(declaration.symbol) {
                if indexed_types.insert(type_id) {
                    index
                        .insert_type(type_id, declaration.symbol, attachments.clone())
                        .expect("validated declaration type has one indexed identity");
                }
            }
        }
    }
    for function in functions {
        let validated = resolver.validate_attribute_attachments(
            AttributeTarget::Function,
            function.attributes.iter().cloned(),
        );
        if let Some(attachments) = validated.attachment_set() {
            index
                .insert_symbol(function.signature.symbol(), attachments.clone())
                .expect("validated function has one indexed resolver symbol");
        }
    }
    index
}
