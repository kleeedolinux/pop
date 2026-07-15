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
    AttributeUsage, AttributeValidator, BootstrapSchema, CompilerAttributeRole, ForeignAbi,
    ForeignFunctionDeclaration, PrimitiveType, ResolvedAttribute, ResolvedFunctionSignature,
    SemanticType, SignatureResolver, TypeArena,
};

use crate::api::FrontEndCompileTimeEvaluation;
use crate::compile_time::{
    compile_time_attribute_constant, compile_time_function_name, evaluate_compile_time_function,
    evaluate_required_expression,
};
use crate::work::{
    AttributeResolutionContext, CompileTimeContext, DeclarationAttributeWork, FunctionWork,
    NamespaceAttributeWork,
};

pub(crate) fn resolve_ffi_attributes(
    namespaces: &mut [NamespaceAttributeWork],
    functions: &mut [FunctionWork],
    bootstrap: &BootstrapSchema,
    has_ffi_dependency: bool,
    types: &TypeArena,
    diagnostics: &mut Vec<Diagnostic>,
) {
    if !has_ffi_dependency {
        return;
    }
    for namespace in namespaces.iter_mut() {
        let mut ordinary = Vec::new();
        let mut aliases = BTreeSet::new();
        for attribute in std::mem::take(&mut namespace.attribute_uses) {
            match ffi_attribute_role(bootstrap, &attribute) {
                Some(CompilerAttributeRole::FfiLink) => {
                    if let Some(alias) = parse_link_alias(&attribute) {
                        if !aliases.insert(alias.clone()) {
                            diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
                                attribute.span(),
                                "duplicate Ffi.Link alias",
                            ));
                        }
                    } else {
                        diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
                            attribute.span(),
                            "Ffi.Link requires one PascalCase alias string",
                        ));
                    }
                }
                Some(_) => diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
                    attribute.span(),
                    "FFI attribute has the wrong attachment target",
                )),
                None => ordinary.push(attribute),
            }
        }
        namespace.attribute_uses = ordinary;
        namespace.ffi_link_aliases = aliases.into_iter().collect();
    }
    let aliases_by_module: BTreeMap<_, _> = namespaces
        .iter()
        .map(|namespace| (namespace.module, namespace.ffi_link_aliases.clone()))
        .collect();
    for function in functions {
        resolve_foreign_function(
            function,
            aliases_by_module
                .get(&function.module)
                .cloned()
                .unwrap_or_default(),
            bootstrap,
            types,
            diagnostics,
        );
    }
}

fn resolve_foreign_function(
    function: &mut FunctionWork,
    link_aliases: Vec<String>,
    bootstrap: &BootstrapSchema,
    types: &TypeArena,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut ordinary = Vec::new();
    let mut foreign = None;
    let mut nonblocking = None;
    let mut malformed = false;
    for attribute in std::mem::take(&mut function.attribute_uses) {
        match ffi_attribute_role(bootstrap, &attribute) {
            Some(CompilerAttributeRole::FfiForeign) if foreign.is_none() => {
                foreign = Some(attribute);
            }
            Some(CompilerAttributeRole::FfiNonblocking) if nonblocking.is_none() => {
                nonblocking = Some(attribute);
            }
            Some(CompilerAttributeRole::FfiForeign | CompilerAttributeRole::FfiNonblocking) => {
                malformed = true;
                diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
                    attribute.span(),
                    "duplicate foreign function attribute",
                ));
            }
            Some(_) => {
                malformed = true;
                diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
                    attribute.span(),
                    "FFI attribute has the wrong attachment target",
                ));
            }
            None => ordinary.push(attribute),
        }
    }
    function.attribute_uses = ordinary;
    let Some(foreign_syntax) = foreign else {
        if let Some(nonblocking) = nonblocking {
            diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
                nonblocking.span(),
                "Ffi.Nonblocking requires Ffi.Foreign",
            ));
        }
        return;
    };
    let initial_error_count = diagnostics.len();
    let parsed = parse_foreign_contract(&foreign_syntax).or_else(|| {
        diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
            foreign_syntax.span(),
            "Ffi.Foreign requires a symbol and a closed ABI",
        ));
        None
    });
    if nonblocking
        .as_ref()
        .is_some_and(|attribute| !attribute.arguments().is_empty())
    {
        diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
            nonblocking
                .as_ref()
                .map_or(foreign_syntax.span(), |value| value.span()),
            "Ffi.Nonblocking takes no arguments",
        ));
    }
    if !function.body.statements().is_empty() {
        diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
            function.span,
            "foreign functions cannot have a Pop body",
        ));
    }
    if function.signature.is_async() {
        diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
            function.span,
            "foreign functions cannot be async",
        ));
    }
    if !function.signature.type_parameters().is_empty() {
        diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
            function.span,
            "foreign functions cannot be generic",
        ));
    }
    let abi_types_valid = function.signature.parameters().iter().all(|parameter| {
        parameter
            .parameter_type()
            .type_id()
            .is_some_and(|type_id| valid_foreign_abi_type(type_id, types, bootstrap))
    }) && function.signature.results().iter().all(|result| {
        result
            .type_id()
            .is_some_and(|type_id| valid_foreign_abi_type(type_id, types, bootstrap))
    });
    if !abi_types_valid {
        diagnostics.push(pop_diagnostics::ffi::invalid_foreign_contract(
            function.span,
            "foreign signature contains a type without a direct ABI representation",
        ));
    }
    if malformed || diagnostics.len() != initial_error_count {
        return;
    }
    let Some((external_symbol, abi)) = parsed else {
        return;
    };
    let declaration = ForeignFunctionDeclaration::new(
        function.signature.symbol(),
        external_symbol,
        abi,
        link_aliases,
        nonblocking.is_some(),
        foreign_syntax.span(),
    );
    function.signature = function
        .signature
        .clone()
        .with_effects(declaration.effects());
    function.foreign = Some(declaration);
}

fn ffi_attribute_role(
    bootstrap: &BootstrapSchema,
    attribute: &AttributeUseSyntax,
) -> Option<CompilerAttributeRole> {
    let name = attribute.path().join(".");
    let entry = bootstrap.compiler_attribute_by_source_name(&name)?;
    (entry.owner_bubble() == "Pop.Ffi").then(|| entry.role())
}

fn parse_link_alias(attribute: &AttributeUseSyntax) -> Option<String> {
    let [argument] = attribute.arguments() else {
        return None;
    };
    if argument.name().is_some() {
        return None;
    }
    let ExpressionSyntaxKind::String(alias) = argument.value().kind() else {
        return None;
    };
    valid_pascal_identifier(alias).then(|| alias.clone())
}

fn valid_pascal_identifier(value: &str) -> bool {
    let mut characters = value.chars();
    characters
        .next()
        .is_some_and(|first| first.is_ascii_uppercase())
        && characters.all(|character| character.is_ascii_alphanumeric())
}

fn parse_foreign_contract(attribute: &AttributeUseSyntax) -> Option<(String, ForeignAbi)> {
    let arguments = attribute.arguments();
    if !(1..=2).contains(&arguments.len()) || arguments[0].name().is_some() {
        return None;
    }
    let ExpressionSyntaxKind::String(symbol) = arguments[0].value().kind() else {
        return None;
    };
    if symbol.is_empty()
        || !symbol.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '_' | '.' | '$' | '@' | '?')
        })
    {
        return None;
    }
    let abi = if let Some(argument) = arguments.get(1) {
        if argument.name() != Some("abi") {
            return None;
        }
        let ExpressionSyntaxKind::String(abi) = argument.value().kind() else {
            return None;
        };
        match abi.as_str() {
            "C" => ForeignAbi::C,
            "System" => ForeignAbi::System,
            "CUnwind" => ForeignAbi::CUnwind,
            _ => return None,
        }
    } else {
        ForeignAbi::C
    };
    Some((symbol.clone(), abi))
}

fn valid_foreign_abi_type(type_id: TypeId, types: &TypeArena, bootstrap: &BootstrapSchema) -> bool {
    match types.get(type_id) {
        Some(SemanticType::Primitive(
            PrimitiveType::Integer(_) | PrimitiveType::Float32 | PrimitiveType::Float64,
        )) => true,
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) => {
            let Some(entry) = bootstrap.type_by_id(*definition) else {
                return false;
            };
            match entry.source_name() {
                "Ffi.Pointer" | "Ffi.OptionalPointer" => {
                    let [pointee] = arguments.as_slice() else {
                        return false;
                    };
                    valid_foreign_pointer_target(*pointee, types, bootstrap)
                }
                "Ffi.Handle" => arguments.len() == 1,
                "Ffi.Function" | "Ffi.OptionalFunction" => {
                    let [signature] = arguments.as_slice() else {
                        return false;
                    };
                    valid_foreign_callback_signature(*signature, types, bootstrap)
                }
                name if name.starts_with("Ffi.C.") => arguments.is_empty(),
                _ => false,
            }
        }
        _ => false,
    }
}

fn valid_foreign_pointer_target(
    type_id: TypeId,
    types: &TypeArena,
    bootstrap: &BootstrapSchema,
) -> bool {
    match types.get(type_id) {
        Some(
            SemanticType::Primitive(
                PrimitiveType::Integer(_) | PrimitiveType::Float32 | PrimitiveType::Float64,
            )
            | SemanticType::Opaque(_),
        ) => true,
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) => bootstrap.type_by_id(*definition).is_some_and(|entry| {
            if entry.source_name().starts_with("Ffi.C.") {
                arguments.is_empty()
            } else if matches!(entry.source_name(), "Ffi.Pointer" | "Ffi.OptionalPointer") {
                let [pointee] = arguments.as_slice() else {
                    return false;
                };
                valid_foreign_pointer_target(*pointee, types, bootstrap)
            } else {
                false
            }
        }),
        _ => false,
    }
}

fn valid_foreign_callback_signature(
    type_id: TypeId,
    types: &TypeArena,
    bootstrap: &BootstrapSchema,
) -> bool {
    let Some(SemanticType::Function {
        is_async,
        parameters,
        results,
        ..
    }) = types.get(type_id)
    else {
        return false;
    };
    !is_async
        && parameters
            .iter()
            .chain(results)
            .all(|type_id| valid_foreign_abi_type(*type_id, types, bootstrap))
}

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
                "Error" => Some(AttributeTarget::Error),
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
    namespaces: &mut [NamespaceAttributeWork],
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
    resolve_namespace_attributes(
        namespaces,
        context,
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

fn resolve_namespace_attributes(
    namespaces: &mut [NamespaceAttributeWork],
    context: AttributeResolutionContext<'_>,
    resolver: &mut SignatureResolver<'_>,
    compile_time_evaluations: &mut Vec<FrontEndCompileTimeEvaluation>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for namespace in namespaces {
        let resolved_attributes = resolve_attribute_uses(
            namespace.module,
            AttributeTarget::Namespace,
            &namespace.attribute_uses,
            context.database,
            context.signatures,
            context.compile_time,
            resolver,
            compile_time_evaluations,
            diagnostics,
        );
        let validated = resolver
            .validate_attribute_attachments(AttributeTarget::Namespace, resolved_attributes);
        diagnostics.extend(
            validated
                .errors()
                .iter()
                .map(attribute_attachment_diagnostic),
        );
        if let Some(attachments) = validated.attachment_set() {
            namespace.attributes = attachments.attachments().to_vec();
        }
    }
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
