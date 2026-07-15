//! Front-end orchestration implementation.
#![allow(clippy::match_same_arms, clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet};

use pop_diagnostics::compile_time as compile_time_diagnostics;
use pop_diagnostics::documentation as documentation_diagnostics;
use pop_diagnostics::syntax as syntax_diagnostics;
use pop_diagnostics::types as type_diagnostics;
use pop_documentation::{
    DocumentationAnalysis, PublicErrorDocumentationContract, TypedErrorDocumentationContract,
    TypedReturnsDocumentationContract,
};
use pop_foundation::{
    BubbleId, Diagnostic, DiagnosticSeverity, MethodId, ModuleId, SourceSpan, SymbolId,
};
use pop_hir::{
    HirBubble, HirDataSpecialization, HirDeclaration, HirDeclarationKind, HirForeignFunction,
    HirFunction, HirFunctionContext, HirKnownCallables, HirMethod, build_hir_foreign_function,
    build_hir_function_with_known_callables_and_attributes, build_hir_method,
    specialize_hir_method,
};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_syntax::{
    AttributeUseSyntax, NodeKind, parse_attribute_declaration, parse_attribute_use,
    parse_class_declaration, parse_class_method_body, parse_const_declaration,
    parse_enum_declaration, parse_error_declaration, parse_file, parse_function_body,
    parse_function_signature, parse_interface_declaration, parse_record_declaration,
    parse_type_alias_declaration, parse_union_declaration,
};
use pop_types::{
    AttributeTarget, BodyChecker, BootstrapSchema, ResolvedFunctionSignature, SemanticType,
    SignatureResolver, embedded_bootstrap_schema,
};

use crate::api::*;
use crate::attributes::{
    classify_function_attributes, resolve_ffi_attributes, resolve_source_attributes,
};
use crate::compile_time::{
    build_compile_time_context, check_compile_time_function_bodies,
    compile_time_attribute_constant, evaluate_declaration_defaults, evaluate_source_constants,
};
use crate::reference::{
    emit_reference_metadata, hir_function_references, invalid_reference_capsule,
    reference_signatures,
};
use crate::work::*;

/// Runs the architecture-ordered front end through verified backend-neutral HIR.
///
/// # Panics
///
/// Panics only if repository-validated bootstrap metadata is invalid or if a
/// resolver symbol cannot be published once in the immutable attribute-query
/// snapshot. Both are toolchain incidents guarded by bootstrap/query tests.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn analyze_bubble(input: FrontEndBubbleInput) -> FrontEndResult {
    let invalid_capsule = invalid_reference_capsule(&input.reference_metadata);
    let parsed = parse_modules(input.modules);
    let module_inputs: Vec<_> = parsed
        .iter()
        .map(|module| {
            let module_input =
                ModuleInput::new(module.module, input.bubble, &module.source, &module.syntax);
            if input.implicit_main_module == Some(module.module) {
                module_input.with_implicit_main_entry()
            } else {
                module_input
            }
        })
        .collect();
    let indexed = build_declaration_index(&module_inputs);
    let mut diagnostics = indexed.diagnostics().to_vec();
    validate_source_attribute_targets(&parsed, &mut diagnostics);
    let mut namespace_attribute_work = define_namespace_attributes(&parsed, &mut diagnostics);
    let referenced_declarations = input
        .reference_metadata
        .iter()
        .flat_map(ReferenceMetadata::functions)
        .map(|function| {
            pop_resolve::ReferencedDeclaration::function(
                function.identity(),
                function.module(),
                function.namespace(),
                function.name(),
                function.span(),
            )
        })
        .collect::<Vec<_>>();
    for metadata in &input.reference_metadata {
        assert!(
            input.dependencies.contains(&metadata.bubble()),
            "reference metadata must belong to a direct Bubble dependency"
        );
    }
    let index = indexed
        .into_index()
        .with_referenced_declarations(referenced_declarations)
        .expect("reference metadata identities are verified before analysis");
    let bootstrap = embedded_bootstrap_schema().expect("repository-validated bootstrap schema");
    let standard_baseline = pop_standard::standard_api_baseline()
        .expect("repository-validated Pop.Standard API baseline");
    let database = standard_baseline
        .entries()
        .iter()
        .filter(|entry| entry.prelude() && entry.kind() == pop_standard::ApiKind::Namespace)
        .fold(ResolutionDatabase::new(index), |database, entry| {
            database
                .with_prelude_namespace_root(
                    entry.name(),
                    entry.signature().trim_start_matches("namespace "),
                )
                .expect("repository-validated prelude namespace root")
        });
    validate_standard_native_exports(&bootstrap, pop_standard::NATIVE_EXPORTS)
        .expect("repository-validated native Standard adapters");
    let mut resolver = SignatureResolver::new(&database, bootstrap.clone());
    if input.ffi_dependency.is_some() {
        resolver = resolver.with_ffi_dependency();
    }
    define_type_aliases(&parsed, &database, &mut resolver, &mut diagnostics);
    let (mut declarations, methods, mut declaration_attributes) = define_declarations(
        &parsed,
        input.bubble,
        &database,
        &mut resolver,
        &mut diagnostics,
    );
    let (constant_work, mut constant_attributes) =
        define_constants(&parsed, &database, &mut diagnostics);
    declaration_attributes.append(&mut constant_attributes);
    declaration_attributes.sort_by_key(|work| work.symbol);
    let mut functions = resolve_functions(
        &parsed,
        &database,
        &bootstrap,
        &mut resolver,
        &mut diagnostics,
    );
    resolve_ffi_attributes(
        &mut namespace_attribute_work,
        &mut functions,
        &bootstrap,
        input.ffi_dependency.is_some(),
        resolver.arena(),
        &mut diagnostics,
    );
    let mut signatures = reference_signatures(&input.reference_metadata, &database, &mut resolver);
    signatures.extend(
        functions
            .iter()
            .map(|function| (function.signature.symbol(), function.signature.clone())),
    );
    let checked_documentation = validate_documentation(
        input.bubble,
        &parsed,
        &functions,
        &database,
        &resolver,
        &mut diagnostics,
    );
    let preliminary_bodies = check_compile_time_function_bodies(
        &functions,
        &signatures,
        &mut resolver,
        &mut diagnostics,
    );
    let compile_time = build_compile_time_context(
        &functions,
        &preliminary_bodies,
        resolver.arena(),
        &mut diagnostics,
    );
    let mut compile_time_evaluations = Vec::new();
    evaluate_declaration_defaults(
        &declarations,
        &signatures,
        &compile_time,
        &mut resolver,
        &mut compile_time_evaluations,
        &mut diagnostics,
    );
    let attribute_queries = resolve_source_attributes(
        &mut namespace_attribute_work,
        &mut declaration_attributes,
        &mut functions,
        AttributeResolutionContext {
            database: &database,
            bootstrap: &bootstrap,
            signatures: &signatures,
            compile_time: &compile_time,
        },
        &mut resolver,
        &mut compile_time_evaluations,
        &mut diagnostics,
    );
    let constants = evaluate_source_constants(
        &constant_work,
        &signatures,
        &compile_time,
        &attribute_queries,
        &mut resolver,
        &mut compile_time_evaluations,
        &mut diagnostics,
    );
    let runtime_constants: BTreeMap<_, _> = constants
        .iter()
        .filter_map(|constant| {
            compile_time_attribute_constant(constant.value.clone()).map(|value| {
                (
                    constant.symbol,
                    pop_types::RuntimeConstant::new(constant.type_id, value),
                )
            })
        })
        .collect();
    let (hir_functions, hir_foreign_functions, hir_methods, hir_build_errors) = build_runtime_hir(
        input.bubble,
        &mut functions,
        &methods,
        &signatures,
        &runtime_constants,
        &mut resolver,
        &mut diagnostics,
    );
    declarations = refresh_declarations(declarations, &resolver);
    let referenced_call_instances = hir_functions
        .iter()
        .flat_map(pop_hir::hir_referenced_call_instances)
        .collect::<Vec<_>>();
    sort_diagnostics(&mut diagnostics);
    let hir_result = if let Some(identity) = invalid_capsule {
        Err(pop_hir::HirBubbleError::InvalidSpecializationCapsule(
            identity,
        ))
    } else if diagnostics
        .iter()
        .all(|diagnostic| diagnostic.severity() != DiagnosticSeverity::Error)
        && hir_build_errors.is_empty()
    {
        HirBubble::new_with_declarations_and_methods(
            input.bubble,
            input.namespace,
            input.dependencies,
            declarations,
            hir_functions,
            hir_methods,
        )
        .and_then(|bubble| bubble.with_foreign_functions(hir_foreign_functions))
        .and_then(|bubble| {
            bubble.with_function_references(hir_function_references(
                &input.reference_metadata,
                &database,
                &signatures,
                input.bubble,
                &mut resolver,
                &referenced_call_instances,
            ))
        })
        .map(Some)
    } else {
        Ok(None)
    };
    let (hir, hir_bubble_error) = match hir_result {
        Ok(hir) => (hir, None),
        Err(error) => (None, Some(error)),
    };
    let reference_metadata = hir
        .as_ref()
        .map_or(Err(ReferenceMetadataError::AnalysisUnavailable), |hir| {
            emit_reference_metadata(hir, database.index(), resolver.arena())
        });
    FrontEndResult {
        hir,
        hir_bubble_error,
        hir_build_errors,
        types: resolver.into_arena(),
        attribute_queries,
        namespace_attributes: namespace_attribute_work
            .into_iter()
            .map(|work| NamespaceAttributes {
                module: work.module,
                attributes: work.attributes,
            })
            .collect(),
        foreign_declarations: functions
            .iter()
            .filter_map(|function| function.foreign.clone())
            .collect(),
        compile_time_evaluations,
        constants,
        diagnostics,
        reference_metadata,
        checked_documentation,
    }
}

fn validate_documentation(
    bubble: BubbleId,
    modules: &[ParsedModule],
    functions: &[FunctionWork],
    database: &ResolutionDatabase,
    resolver: &SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<CheckedDocumentation> {
    let mut analyses: BTreeMap<_, _> = modules
        .iter()
        .map(|module| {
            (
                module.module,
                DocumentationAnalysis::analyze(&module.source, &module.syntax),
            )
        })
        .collect();
    let functions_by_symbol: BTreeMap<_, _> = functions
        .iter()
        .map(|function| (function.signature.symbol(), function))
        .collect();
    let mut inherited_error_tags = BTreeMap::new();
    let mut inheritance_stack = BTreeSet::new();
    for function in functions {
        effective_error_documentation(
            function,
            database,
            &functions_by_symbol,
            &analyses,
            &mut inherited_error_tags,
            &mut inheritance_stack,
            diagnostics,
        );
    }

    for module in modules {
        let analysis = analyses
            .get_mut(&module.module)
            .expect("every parsed Module has documentation analysis");
        let public_errors: Vec<_> = module
            .syntax
            .root()
            .children()
            .iter()
            .filter(|node| node.kind() == NodeKind::ErrorDeclaration)
            .filter_map(|node| {
                let syntax = parse_error_declaration(&module.source, &module.syntax, node).ok()?;
                let declaration = database.index().declarations().find(|declaration| {
                    declaration.module() == module.module
                        && declaration.kind() == pop_resolve::DeclarationKind::Error
                        && declaration.name() == syntax.name()
                })?;
                (declaration.visibility() == pop_resolve::Visibility::Public).then(|| {
                    PublicErrorDocumentationContract::new(
                        node.range(),
                        syntax.name(),
                        syntax
                            .cases()
                            .iter()
                            .map(|case| (case.name(), case.span().range())),
                    )
                })
            })
            .collect();
        analysis.validate_public_error_summaries(&public_errors);
        let contracts: Vec<_> = functions
            .iter()
            .filter(|function| function.module == module.module)
            .map(|function| {
                let error = function
                    .signature
                    .results()
                    .first()
                    .filter(|_| function.signature.results().len() == 1)
                    .and_then(pop_types::ResolvedType::type_id)
                    .and_then(|result| resolver.result_parts(result))
                    .and_then(|(_, error)| {
                        resolver
                            .error_definition_for_type(error)
                            .map(|definition| (error, definition))
                    });
                let Some((error_type, error)) = error else {
                    return TypedErrorDocumentationContract::without_result(
                        function.documentation_target,
                    );
                };
                let source = match resolver.arena().get(error_type) {
                    Some(pop_types::SemanticType::ErrorUnion { source, .. }) => *source,
                    _ => error.symbol(),
                };
                let declaration = database
                    .index()
                    .declaration(source)
                    .expect("resolved error definition retains its declaration");
                let mut names = vec![declaration.qualified_name(), declaration.name().to_owned()];
                names.sort();
                names.dedup();
                TypedErrorDocumentationContract::result_with_names(
                    function.documentation_target,
                    names,
                    error.cases().iter().map(|case| case.name()),
                    function.visibility == pop_resolve::Visibility::Public,
                )
                .with_inherited_error_tags(
                    inherited_error_tags
                        .get(&function.signature.symbol())
                        .into_iter()
                        .flatten()
                        .cloned(),
                )
            })
            .collect();
        analysis.validate_typed_errors(&contracts);
        let returns: Vec<_> = functions
            .iter()
            .filter(|function| function.module == module.module)
            .map(|function| {
                let results = function.signature.results();
                if results.is_empty() {
                    TypedReturnsDocumentationContract::without_result(function.documentation_target)
                } else if results.len() == 1
                    && results[0]
                        .type_id()
                        .is_some_and(|result| resolver.result_parts(result).is_some())
                {
                    TypedReturnsDocumentationContract::result_ok(function.documentation_target)
                } else {
                    TypedReturnsDocumentationContract::values(function.documentation_target)
                }
            })
            .collect();
        analysis.validate_typed_returns(&returns);
        diagnostics.extend(analysis.diagnostics().iter().cloned());
    }

    let mut checked = functions
        .iter()
        .filter(|function| function.visibility == pop_resolve::Visibility::Public)
        .filter_map(|function| {
            analyses
                .get(&function.module)?
                .xml_for_target(function.documentation_target)
                .cloned()
                .map(|fragment| CheckedDocumentation {
                    identity: pop_foundation::SymbolIdentity::new(
                        bubble,
                        function.signature.symbol(),
                    ),
                    fragment,
                })
        })
        .collect::<Vec<_>>();
    checked.sort_by_key(CheckedDocumentation::identity);
    checked
}

fn effective_error_documentation(
    function: &FunctionWork,
    database: &ResolutionDatabase,
    functions: &BTreeMap<SymbolId, &FunctionWork>,
    analyses: &BTreeMap<ModuleId, DocumentationAnalysis>,
    cache: &mut BTreeMap<SymbolId, Vec<String>>,
    stack: &mut BTreeSet<SymbolId>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<String> {
    let symbol = function.signature.symbol();
    if let Some(tags) = cache.get(&symbol) {
        return tags.clone();
    }
    if !stack.insert(symbol) {
        diagnostics.push(documentation_diagnostics::inheritance_cycle(
            function.span,
            function.signature.name(),
        ));
        return Vec::new();
    }
    let analysis = analyses
        .get(&function.module)
        .expect("function Module has documentation analysis");
    let mut tags: BTreeSet<_> = analysis
        .error_tags_for_target(function.documentation_target)
        .into_iter()
        .collect();
    for reference in analysis.inheritance_references_for_target(function.documentation_target) {
        let resolution = database.resolve(
            function.module,
            &reference,
            SymbolSpace::Value,
            function.span,
        );
        let Some(source) = resolution
            .symbol()
            .and_then(|source| functions.get(&source).copied())
        else {
            diagnostics.push(documentation_diagnostics::invalid_inheritance(
                function.span,
                reference,
            ));
            continue;
        };
        if !documentation_signatures_are_compatible(&function.signature, &source.signature) {
            diagnostics.push(documentation_diagnostics::invalid_inheritance(
                function.span,
                reference,
            ));
            continue;
        }
        tags.extend(effective_error_documentation(
            source,
            database,
            functions,
            analyses,
            cache,
            stack,
            diagnostics,
        ));
    }
    stack.remove(&symbol);
    let tags: Vec<_> = tags.into_iter().collect();
    cache.insert(symbol, tags.clone());
    tags
}

fn documentation_signatures_are_compatible(
    target: &ResolvedFunctionSignature,
    source: &ResolvedFunctionSignature,
) -> bool {
    target.type_parameters().len() == source.type_parameters().len()
        && target.parameters().len() == source.parameters().len()
        && target.results().len() == source.results().len()
        && target.effects() == source.effects()
        && target
            .parameters()
            .iter()
            .zip(source.parameters())
            .all(|(target, source)| {
                target.name() == source.name()
                    && target.parameter_type().type_id() == source.parameter_type().type_id()
            })
        && target
            .results()
            .iter()
            .zip(source.results())
            .all(|(target, source)| target.type_id() == source.type_id())
}

fn define_type_aliases(
    modules: &[ParsedModule],
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for module in modules {
        for node in module.syntax.root().children() {
            if node.kind() != NodeKind::TypeAliasDeclaration {
                continue;
            }
            match parse_type_alias_declaration(&module.source, &module.syntax, node) {
                Ok(syntax) => {
                    if let Some(symbol) = resolve_symbol(
                        database,
                        module.module,
                        syntax.name(),
                        SymbolSpace::Type,
                        syntax.span(),
                        diagnostics,
                    ) {
                        resolver.register_type_alias(module.module, symbol, &syntax);
                    }
                }
                Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
            }
        }
    }
}

fn parse_modules(modules: Vec<FrontEndModule>) -> Vec<ParsedModule> {
    modules
        .into_iter()
        .map(|module| ParsedModule {
            module: module.module,
            syntax: parse_file(&module.source),
            source: module.source,
        })
        .collect()
}

fn validate_source_attribute_targets(modules: &[ParsedModule], diagnostics: &mut Vec<Diagnostic>) {
    for module in modules {
        let children = module.syntax.root().children();
        let mut index = 0_usize;
        while index < children.len() {
            if children[index].kind() != NodeKind::AttributeUse {
                index += 1;
                continue;
            }
            let first = index;
            while index < children.len() && children[index].kind() == NodeKind::AttributeUse {
                index += 1;
            }
            let supported = children.get(index).is_some_and(|node| {
                matches!(
                    node.kind(),
                    NodeKind::NamespaceDeclaration
                        | NodeKind::AttributeDeclaration
                        | NodeKind::ConstDeclaration
                        | NodeKind::RecordDeclaration
                        | NodeKind::UnionDeclaration
                        | NodeKind::ErrorDeclaration
                        | NodeKind::ClassDeclaration
                        | NodeKind::InterfaceDeclaration
                        | NodeKind::FunctionDeclaration
                )
            });
            if supported {
                continue;
            }
            for attribute in &children[first..index] {
                diagnostics.push(compile_time_diagnostics::ineligible_constant_expression(
                    SourceSpan::new(module.source.id(), attribute.range()),
                    "attribute attachment target",
                ));
            }
        }
    }
}

fn define_namespace_attributes(
    modules: &[ParsedModule],
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<NamespaceAttributeWork> {
    let mut work = Vec::new();
    for module in modules {
        let mut attribute_uses = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => attribute_uses.push(syntax),
                    Err(error) => {
                        diagnostics.push(syntax_error(error.span(), error.expectation()));
                    }
                }
                continue;
            }
            if node.kind() == NodeKind::NamespaceDeclaration && !attribute_uses.is_empty() {
                work.push(NamespaceAttributeWork {
                    module: module.module,
                    attribute_uses,
                    attributes: Vec::new(),
                    ffi_link_aliases: Vec::new(),
                });
            }
            break;
        }
    }
    work.sort_by_key(|work| work.module);
    work
}

fn define_constants(
    modules: &[ParsedModule],
    database: &ResolutionDatabase,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<ConstantWork>, Vec<DeclarationAttributeWork>) {
    let mut constants = Vec::new();
    let mut attribute_work = Vec::new();
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
                }
                continue;
            }
            if node.kind() != NodeKind::ConstDeclaration {
                pending_attributes.clear();
                continue;
            }
            let attribute_uses = std::mem::take(&mut pending_attributes);
            match parse_const_declaration(&module.source, &module.syntax, node) {
                Ok(syntax) => {
                    if let Some(symbol) = resolve_symbol(
                        database,
                        module.module,
                        syntax.name(),
                        SymbolSpace::Value,
                        syntax.span(),
                        diagnostics,
                    ) {
                        constants.push(ConstantWork {
                            module: module.module,
                            symbol,
                            syntax,
                        });
                        attribute_work.push(DeclarationAttributeWork {
                            module: module.module,
                            symbol,
                            target: AttributeTarget::Constant,
                            attribute_uses,
                            attributes: Vec::new(),
                        });
                    }
                }
                Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
            }
        }
    }
    constants.sort_by_key(|work| work.symbol);
    attribute_work.sort_by_key(|work| work.symbol);
    (constants, attribute_work)
}

fn build_runtime_hir(
    bubble: BubbleId,
    functions: &mut [FunctionWork],
    methods: &[MethodWork],
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    constants: &BTreeMap<SymbolId, pop_types::RuntimeConstant>,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (
    Vec<HirFunction>,
    Vec<HirForeignFunction>,
    Vec<HirMethod>,
    Vec<pop_hir::HirBuildError>,
) {
    let known_functions: BTreeSet<_> = functions
        .iter()
        .filter(|function| !function.is_compile_time)
        .map(|function| function.signature.symbol())
        .collect();
    let interfaces: Vec<_> = resolver.interface_definitions().cloned().collect();
    let mut hir_build_errors = Vec::new();
    let mut hir_foreign_functions = Vec::new();
    for function in functions
        .iter()
        .filter(|function| function.foreign.is_some())
    {
        let foreign = function
            .foreign
            .as_ref()
            .expect("filtered foreign function");
        match build_hir_foreign_function(
            HirFunctionContext::new(function.module, bubble, function.visibility),
            &function.signature,
            foreign,
            &function.attributes,
        ) {
            Ok(function) => hir_foreign_functions.push(function),
            Err(errors) => hir_build_errors.extend(errors),
        }
    }
    let mut typed_functions = Vec::new();
    for (index, function) in functions.iter().enumerate() {
        if function.is_compile_time || function.foreign.is_some() {
            continue;
        }
        let typed = BodyChecker::new(function.module, resolver, signatures)
            .with_runtime_constants(constants)
            .check(&function.signature, &function.body);
        diagnostics.extend(typed.diagnostics().iter().cloned());
        let Some(body) = typed.body() else {
            continue;
        };
        typed_functions.push((index, body.clone()));
    }
    let known_methods: BTreeSet<MethodId> = resolver
        .class_definitions()
        .flat_map(|definition| definition.methods().iter())
        .map(pop_types::ClassMethodDefinition::method)
        .collect();
    let mut hir_functions = Vec::new();
    for (index, body) in typed_functions {
        let function = &functions[index];
        match build_hir_function_with_known_callables_and_attributes(
            HirFunctionContext::new(function.module, bubble, function.visibility),
            &function.signature,
            &body,
            resolver.arena(),
            HirKnownCallables::new(&known_functions, &known_methods).with_interfaces(&interfaces),
            &function.attributes,
        ) {
            Ok(function) => hir_functions.push(function),
            Err(errors) => hir_build_errors.extend(errors),
        }
    }
    let mut hir_methods = Vec::new();
    for method in methods {
        let typed = BodyChecker::new(method.module, resolver, signatures)
            .with_runtime_constants(constants)
            .check(&method.signature, &method.body);
        diagnostics.extend(typed.diagnostics().iter().cloned());
        let Some(body) = typed.body() else {
            continue;
        };
        let definition = resolver
            .class_definition(method.definition.symbol())
            .unwrap_or(&method.definition);
        match build_hir_method(
            HirFunctionContext::new(method.module, bubble, method.method.visibility()),
            definition,
            &method.method,
            &method.signature,
            body,
            resolver.arena(),
            HirKnownCallables::new(&known_functions, &known_methods).with_interfaces(&interfaces),
        ) {
            Ok(lowered) => hir_methods.push(lowered),
            Err(errors) => hir_build_errors.extend(errors),
        }
    }
    let template_methods = hir_methods.clone();
    for method in methods {
        if method.definition.type_parameters().is_empty() {
            continue;
        }
        let Some(template) = template_methods
            .iter()
            .find(|candidate| candidate.method() == method.method.method())
        else {
            continue;
        };
        let instances = resolver
            .class_instances(method.definition.symbol())
            .filter(|definition| {
                !resolver
                    .arena()
                    .contains_type_parameter(definition.type_id())
            })
            .cloned()
            .collect::<Vec<_>>();
        for instance in instances {
            let Some(SemanticType::Class { arguments, .. }) =
                resolver.arena().get(instance.type_id())
            else {
                continue;
            };
            let Some(concrete_method) = instance.methods().iter().find(|candidate| {
                candidate.name() == method.method.name()
                    && candidate.dispatch() == method.method.dispatch()
            }) else {
                continue;
            };
            let fields = method
                .definition
                .fields()
                .iter()
                .filter_map(|template_field| {
                    instance
                        .fields()
                        .iter()
                        .find(|field| field.name() == template_field.name())
                        .map(|field| ((instance.type_id(), template_field.field()), field.field()))
                })
                .collect();
            let methods = method
                .definition
                .methods()
                .iter()
                .filter_map(|template_method| {
                    instance
                        .methods()
                        .iter()
                        .find(|candidate| {
                            candidate.name() == template_method.name()
                                && candidate.dispatch() == template_method.dispatch()
                        })
                        .map(|candidate| {
                            (
                                (instance.type_id(), template_method.method()),
                                candidate.method(),
                            )
                        })
                })
                .collect();
            let data = HirDataSpecialization::new(BTreeMap::new(), fields).with_classes(
                BTreeMap::from([(instance.type_id(), (instance.symbol(), instance.class()))]),
                methods,
            );
            match specialize_hir_method(
                template,
                &instance,
                concrete_method,
                arguments,
                &data,
                resolver.arena(),
            ) {
                Some(specialized) => hir_methods.push(specialized),
                None => hir_build_errors.push(pop_hir::HirVerificationError::InvalidType {
                    type_id: instance.type_id(),
                    span: instance.span(),
                }),
            }
        }
    }
    hir_methods.sort_by_key(HirMethod::method);
    (
        hir_functions,
        hir_foreign_functions,
        hir_methods,
        hir_build_errors,
    )
}

fn define_declarations(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (
    Vec<HirDeclaration>,
    Vec<MethodWork>,
    Vec<DeclarationAttributeWork>,
) {
    let (mut declarations, mut attribute_work) =
        define_attributes(modules, bubble, database, resolver, diagnostics);
    let (methods, mut data_declarations, mut data_attribute_work) =
        define_data(modules, bubble, database, resolver, diagnostics);
    declarations.append(&mut data_declarations);
    attribute_work.append(&mut data_attribute_work);
    declarations.sort_by_key(HirDeclaration::symbol);
    attribute_work.sort_by_key(|work| work.symbol);
    (declarations, methods, attribute_work)
}

fn define_attributes(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<HirDeclaration>, Vec<DeclarationAttributeWork>) {
    let mut declarations = Vec::new();
    let mut attribute_work = Vec::new();
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
                }
                continue;
            }
            if node.kind() != NodeKind::AttributeDeclaration {
                pending_attributes.clear();
                continue;
            }
            let attribute_uses = std::mem::take(&mut pending_attributes);
            match parse_attribute_declaration(&module.source, &module.syntax, node) {
                Ok(syntax) => {
                    if let Some(symbol) = resolve_symbol(
                        database,
                        module.module,
                        syntax.name(),
                        SymbolSpace::Type,
                        syntax.span(),
                        diagnostics,
                    ) {
                        let result =
                            resolver.define_attribute_schema(module.module, symbol, &syntax);
                        diagnostics.extend(result.diagnostics().iter().cloned());
                        if let (Some(definition), Some(declaration)) =
                            (result.definition(), database.index().declaration(symbol))
                        {
                            attribute_work.push(DeclarationAttributeWork {
                                module: module.module,
                                symbol,
                                target: AttributeTarget::Attribute,
                                attribute_uses,
                                attributes: Vec::new(),
                            });
                            declarations.push(HirDeclaration::attribute(
                                module.module,
                                bubble,
                                declaration.visibility(),
                                syntax.name(),
                                definition,
                            ));
                        }
                    }
                }
                Err(error) => {
                    diagnostics.push(syntax_error(error.span(), error.expectation()));
                }
            }
        }
    }
    (declarations, attribute_work)
}

fn define_data(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (
    Vec<MethodWork>,
    Vec<HirDeclaration>,
    Vec<DeclarationAttributeWork>,
) {
    let mut methods = Vec::new();
    let (mut declarations, mut attribute_work) =
        define_records_and_unions(modules, bubble, database, resolver, diagnostics);
    // Interface identities and exact member slots are indexed before classes
    // so an explicit `implements` clause never depends on Module/source order.
    let (mut interface_declarations, mut interface_attribute_work) =
        define_interfaces(modules, bubble, database, resolver, diagnostics);
    declarations.append(&mut interface_declarations);
    attribute_work.append(&mut interface_attribute_work);
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
                }
                continue;
            }
            match node.kind() {
                NodeKind::ClassDeclaration => {
                    let (class_methods, declaration, work) = define_class(
                        module,
                        node,
                        bubble,
                        database,
                        resolver,
                        diagnostics,
                        std::mem::take(&mut pending_attributes),
                    );
                    methods.extend(class_methods);
                    declarations.extend(declaration);
                    attribute_work.extend(work);
                }
                _ => pending_attributes.clear(),
            }
        }
    }
    methods.sort_by_key(|method| method.method.method());
    declarations.sort_by_key(HirDeclaration::symbol);
    attribute_work.sort_by_key(|work| work.symbol);
    (methods, declarations, attribute_work)
}

fn define_records_and_unions(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<HirDeclaration>, Vec<DeclarationAttributeWork>) {
    let mut declarations = Vec::new();
    let mut attribute_work = Vec::new();
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
                }
                continue;
            }
            let (declaration, work) = match node.kind() {
                NodeKind::RecordDeclaration => define_record(
                    module,
                    node,
                    bubble,
                    database,
                    resolver,
                    diagnostics,
                    std::mem::take(&mut pending_attributes),
                ),
                NodeKind::UnionDeclaration => define_union(
                    module,
                    node,
                    bubble,
                    database,
                    resolver,
                    diagnostics,
                    std::mem::take(&mut pending_attributes),
                ),
                NodeKind::ErrorDeclaration => define_error(
                    module,
                    node,
                    bubble,
                    database,
                    resolver,
                    diagnostics,
                    std::mem::take(&mut pending_attributes),
                ),
                NodeKind::EnumDeclaration => define_enum(
                    module,
                    node,
                    bubble,
                    database,
                    resolver,
                    diagnostics,
                    std::mem::take(&mut pending_attributes),
                ),
                _ => {
                    pending_attributes.clear();
                    continue;
                }
            };
            declarations.extend(declaration);
            attribute_work.extend(work);
        }
    }
    (declarations, attribute_work)
}

fn define_interfaces(
    modules: &[ParsedModule],
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<HirDeclaration>, Vec<DeclarationAttributeWork>) {
    let mut declarations = Vec::new();
    let mut attribute_work = Vec::new();
    for module in modules {
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => diagnostics.push(syntax_error(error.span(), error.expectation())),
                }
                continue;
            }
            if node.kind() != NodeKind::InterfaceDeclaration {
                pending_attributes.clear();
                continue;
            }
            let attribute_uses = std::mem::take(&mut pending_attributes);
            let syntax = match parse_interface_declaration(&module.source, &module.syntax, node) {
                Ok(syntax) => syntax,
                Err(error) => {
                    diagnostics.push(syntax_error(error.span(), error.expectation()));
                    continue;
                }
            };
            let Some(symbol) = resolve_symbol(
                database,
                module.module,
                syntax.name(),
                SymbolSpace::Type,
                syntax.span(),
                diagnostics,
            ) else {
                continue;
            };
            let result = resolver.define_interface(module.module, symbol, &syntax);
            diagnostics.extend(result.diagnostics().iter().cloned());
            let (Some(definition), Some(declaration)) =
                (result.definition(), database.index().declaration(symbol))
            else {
                continue;
            };
            declarations.push(HirDeclaration::interface(
                module.module,
                bubble,
                declaration.visibility(),
                syntax.name(),
                definition,
            ));
            attribute_work.push(DeclarationAttributeWork {
                module: module.module,
                symbol,
                target: AttributeTarget::Interface,
                attribute_uses,
                attributes: Vec::new(),
            });
        }
    }
    (declarations, attribute_work)
}

fn define_record(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (Option<HirDeclaration>, Option<DeclarationAttributeWork>) {
    let syntax = match parse_record_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (None, None);
    };
    let result = resolver.define_record_schema(module.module, symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (Some(definition), Some(declaration)) =
        (result.definition(), database.index().declaration(symbol))
    else {
        return (None, None);
    };
    (
        Some(HirDeclaration::record(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            definition,
        )),
        Some(DeclarationAttributeWork {
            module: module.module,
            symbol,
            target: AttributeTarget::Record,
            attribute_uses,
            attributes: Vec::new(),
        }),
    )
}

fn define_union(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (Option<HirDeclaration>, Option<DeclarationAttributeWork>) {
    let syntax = match parse_union_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (None, None);
    };
    let result = resolver.define_union(module.module, symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (Some(definition), Some(declaration)) =
        (result.definition(), database.index().declaration(symbol))
    else {
        return (None, None);
    };
    (
        Some(HirDeclaration::tagged_union(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            definition,
        )),
        Some(DeclarationAttributeWork {
            module: module.module,
            symbol,
            target: AttributeTarget::Union,
            attribute_uses,
            attributes: Vec::new(),
        }),
    )
}

fn define_error(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (Option<HirDeclaration>, Option<DeclarationAttributeWork>) {
    let syntax = match parse_error_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (None, None);
    };
    let result = resolver.define_error(module.module, symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (Some(definition), Some(declaration)) =
        (result.definition(), database.index().declaration(symbol))
    else {
        return (None, None);
    };
    (
        Some(HirDeclaration::error(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            definition,
        )),
        Some(DeclarationAttributeWork {
            module: module.module,
            symbol,
            target: AttributeTarget::Error,
            attribute_uses,
            attributes: Vec::new(),
        }),
    )
}

fn define_enum(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (Option<HirDeclaration>, Option<DeclarationAttributeWork>) {
    let syntax = match parse_enum_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (None, None);
    };
    let result = resolver.define_enum(symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (Some(definition), Some(declaration)) =
        (result.definition(), database.index().declaration(symbol))
    else {
        return (None, None);
    };
    (
        Some(HirDeclaration::enumeration(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            definition,
        )),
        Some(DeclarationAttributeWork {
            module: module.module,
            symbol,
            target: AttributeTarget::Enum,
            attribute_uses,
            attributes: Vec::new(),
        }),
    )
}

fn define_class(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    bubble: BubbleId,
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> (
    Vec<MethodWork>,
    Option<HirDeclaration>,
    Option<DeclarationAttributeWork>,
) {
    let syntax = match parse_class_declaration(&module.source, &module.syntax, node) {
        Ok(syntax) => syntax,
        Err(error) => {
            diagnostics.push(syntax_error(error.span(), error.expectation()));
            return (Vec::new(), None, None);
        }
    };
    let Some(symbol) = resolve_symbol(
        database,
        module.module,
        syntax.name(),
        SymbolSpace::Type,
        syntax.span(),
        diagnostics,
    ) else {
        return (Vec::new(), None, None);
    };
    let result = resolver.define_class_schema(module.module, symbol, &syntax);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let Some(definition) = result.definition().cloned() else {
        return (Vec::new(), None, None);
    };
    let declaration = database.index().declaration(symbol).map(|declaration| {
        HirDeclaration::class(
            module.module,
            bubble,
            declaration.visibility(),
            syntax.name(),
            &definition,
        )
    });
    let methods = syntax
        .methods()
        .iter()
        .zip(definition.methods())
        .filter_map(|(method_syntax, method)| {
            match parse_class_method_body(&module.source, &module.syntax, node, method_syntax) {
                Ok(body) => Some(MethodWork {
                    module: module.module,
                    signature: resolver.method_signature(&definition, method),
                    definition: definition.clone(),
                    method: method.clone(),
                    body,
                }),
                Err(error) => {
                    diagnostics.push(syntax_error(error.span(), error.expectation()));
                    None
                }
            }
        })
        .collect();
    let attribute_work = Some(DeclarationAttributeWork {
        module: module.module,
        symbol,
        target: AttributeTarget::Class,
        attribute_uses,
        attributes: Vec::new(),
    });
    (methods, declaration, attribute_work)
}

fn resolve_functions(
    modules: &[ParsedModule],
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Vec<FunctionWork> {
    let mut functions = Vec::new();
    for module in modules {
        let function_symbols = database
            .index()
            .module(module.module)
            .into_iter()
            .flat_map(pop_resolve::ModuleIndex::declarations)
            .filter(|symbol| {
                database
                    .index()
                    .declaration(**symbol)
                    .is_some_and(|declaration| {
                        declaration.kind() == pop_resolve::DeclarationKind::Function
                    })
            })
            .copied()
            .collect::<Vec<_>>();
        let mut function_symbols = function_symbols.into_iter();
        let mut pending_attributes = Vec::new();
        for node in module.syntax.root().children() {
            if node.kind() == NodeKind::AttributeUse {
                match parse_attribute_use(&module.source, &module.syntax, node) {
                    Ok(syntax) => pending_attributes.push(syntax),
                    Err(error) => {
                        diagnostics.push(syntax_error(error.span(), error.expectation()));
                    }
                }
                continue;
            }
            if node.kind() != NodeKind::FunctionDeclaration {
                pending_attributes.clear();
                continue;
            }
            let Some(symbol) = function_symbols.next() else {
                diagnostics.push(syntax_error(
                    SourceSpan::new(module.source.id(), node.range()),
                    "indexed function declaration",
                ));
                continue;
            };
            if let Some(function) = resolve_function(
                module,
                node,
                PendingFunctionDeclaration {
                    symbol,
                    attribute_uses: std::mem::take(&mut pending_attributes),
                },
                database,
                bootstrap,
                resolver,
                diagnostics,
            ) {
                functions.push(function);
            }
        }
    }
    validate_source_overload_sets(&functions, database, diagnostics);
    functions.sort_by_key(|function| function.signature.symbol());
    functions
}

fn validate_source_overload_sets(
    functions: &[FunctionWork],
    database: &ResolutionDatabase,
    diagnostics: &mut Vec<Diagnostic>,
) {
    let mut groups: BTreeMap<(String, String), Vec<&FunctionWork>> = BTreeMap::new();
    for function in functions {
        let Some(declaration) = database.index().declaration(function.signature.symbol()) else {
            continue;
        };
        groups
            .entry((
                declaration.namespace().to_owned(),
                function.signature.name().to_owned(),
            ))
            .or_default()
            .push(function);
    }
    for ((_, name), group) in groups.into_iter().filter(|(_, group)| group.len() > 1) {
        let first = group[0];
        if let Some(generic) = group
            .iter()
            .copied()
            .find(|function| !function.signature.type_parameters().is_empty())
        {
            diagnostics.push(type_diagnostics::invalid_overload_set(
                generic.span,
                name,
                "generic candidates are not supported",
                first.span,
            ));
            continue;
        }
        let mut parameter_packs = BTreeMap::new();
        for function in group {
            let pack = function
                .signature
                .parameters()
                .iter()
                .filter_map(|parameter| parameter.parameter_type().type_id())
                .collect::<Vec<_>>();
            if let Some(original) = parameter_packs.insert(pack, function.span) {
                diagnostics.push(type_diagnostics::invalid_overload_set(
                    function.span,
                    &name,
                    "parameter types duplicate another overload",
                    original,
                ));
            }
        }
    }
}

struct PendingFunctionDeclaration {
    symbol: SymbolId,
    attribute_uses: Vec<AttributeUseSyntax>,
}

fn resolve_function(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    pending: PendingFunctionDeclaration,
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<FunctionWork> {
    let PendingFunctionDeclaration {
        symbol,
        attribute_uses,
    } = pending;
    let syntax_signature = parse_function_signature(&module.source, &module.syntax, node)
        .map_err(|error| syntax_error(error.span(), error.expectation()))
        .map_err(|diagnostic| diagnostics.push(diagnostic))
        .ok()?;
    let body = parse_function_body(&module.source, &module.syntax, node, &syntax_signature)
        .map_err(|error| syntax_error(error.span(), error.expectation()))
        .map_err(|diagnostic| diagnostics.push(diagnostic))
        .ok()?;
    let span = SourceSpan::new(module.source.id(), syntax_signature.range());
    let declaration = database.index().declaration(symbol)?;
    let result = resolver.resolve(module.module, symbol, &syntax_signature);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (is_compile_time, attribute_uses) =
        classify_function_attributes(database, bootstrap, module.module, attribute_uses);
    Some(FunctionWork {
        module: module.module,
        visibility: declaration.visibility(),
        span,
        documentation_target: node.range(),
        body,
        signature: result.signature()?.clone(),
        is_compile_time,
        attribute_uses,
        attributes: Vec::new(),
        foreign: None,
    })
}

fn refresh_declarations(
    declarations: Vec<HirDeclaration>,
    resolver: &SignatureResolver<'_>,
) -> Vec<HirDeclaration> {
    declarations
        .into_iter()
        .flat_map(|declaration| {
            if matches!(declaration.kind(), HirDeclarationKind::Attribute(_)) {
                if let Some(definition) = resolver.attribute_definition(declaration.symbol()) {
                    return vec![HirDeclaration::attribute(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    )];
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Record(_)) {
                if resolver.record_is_generic(declaration.symbol()) {
                    let mut refreshed = resolver
                        .record_instances(declaration.symbol())
                        .map(|definition| {
                            HirDeclaration::record(
                                declaration.module(),
                                declaration.bubble(),
                                declaration.visibility(),
                                declaration.name(),
                                definition,
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(template) = resolver.record_definition(declaration.symbol()) {
                        refreshed.push(HirDeclaration::record(
                            declaration.module(),
                            declaration.bubble(),
                            declaration.visibility(),
                            declaration.name(),
                            template,
                        ));
                    }
                    return refreshed;
                }
                if let Some(definition) = resolver.record_definition(declaration.symbol()) {
                    return vec![HirDeclaration::record(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    )];
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Union(_))
                && resolver.union_is_generic(declaration.symbol())
            {
                let mut refreshed = resolver
                    .union_instances(declaration.symbol())
                    .map(|definition| {
                        HirDeclaration::tagged_union(
                            declaration.module(),
                            declaration.bubble(),
                            declaration.visibility(),
                            declaration.name(),
                            definition,
                        )
                    })
                    .collect::<Vec<_>>();
                if let Some(template) = resolver.union_definition(declaration.symbol()) {
                    refreshed.push(HirDeclaration::tagged_union(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        template,
                    ));
                }
                return refreshed;
            }
            if matches!(declaration.kind(), HirDeclarationKind::Error(_)) {
                if resolver.error_is_generic(declaration.symbol()) {
                    let mut refreshed = resolver
                        .error_instances(declaration.symbol())
                        .map(|definition| {
                            HirDeclaration::error(
                                declaration.module(),
                                declaration.bubble(),
                                declaration.visibility(),
                                declaration.name(),
                                definition,
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(template) = resolver.error_definition(declaration.symbol()) {
                        refreshed.push(HirDeclaration::error(
                            declaration.module(),
                            declaration.bubble(),
                            declaration.visibility(),
                            declaration.name(),
                            template,
                        ));
                    }
                    return refreshed;
                }
                if let Some(definition) = resolver.error_definition(declaration.symbol()) {
                    return vec![HirDeclaration::error(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    )];
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Class(_)) {
                if resolver.class_is_generic(declaration.symbol()) {
                    let mut refreshed = resolver
                        .class_instances(declaration.symbol())
                        .filter(|definition| {
                            !resolver
                                .arena()
                                .contains_type_parameter(definition.type_id())
                        })
                        .map(|definition| {
                            HirDeclaration::class(
                                declaration.module(),
                                declaration.bubble(),
                                declaration.visibility(),
                                declaration.name(),
                                definition,
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(template) = resolver.class_definition(declaration.symbol()) {
                        refreshed.push(HirDeclaration::class(
                            declaration.module(),
                            declaration.bubble(),
                            declaration.visibility(),
                            declaration.name(),
                            template,
                        ));
                    }
                    return refreshed;
                }
                if let Some(definition) = resolver.class_definition(declaration.symbol()) {
                    return vec![HirDeclaration::class(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    )];
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Interface(_)) {
                if resolver.interface_is_generic(declaration.symbol()) {
                    let mut refreshed = resolver
                        .interface_instances(declaration.symbol())
                        .map(|definition| {
                            HirDeclaration::interface(
                                declaration.module(),
                                declaration.bubble(),
                                declaration.visibility(),
                                declaration.name(),
                                definition,
                            )
                        })
                        .collect::<Vec<_>>();
                    if let Some(template) = resolver.interface_definition(declaration.symbol()) {
                        refreshed.push(HirDeclaration::interface(
                            declaration.module(),
                            declaration.bubble(),
                            declaration.visibility(),
                            declaration.name(),
                            template,
                        ));
                    }
                    return refreshed;
                }
                if let Some(definition) = resolver.interface_definition(declaration.symbol()) {
                    return vec![HirDeclaration::interface(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    )];
                }
            }
            vec![declaration]
        })
        .collect()
}

fn resolve_symbol(
    database: &ResolutionDatabase,
    module: ModuleId,
    name: &str,
    space: SymbolSpace,
    span: SourceSpan,
    diagnostics: &mut Vec<Diagnostic>,
) -> Option<SymbolId> {
    let result = database.resolve(module, name, space, span);
    diagnostics.extend(result.diagnostics().iter().cloned());
    result.symbol()
}

fn syntax_error(span: SourceSpan, expectation: &'static str) -> Diagnostic {
    syntax_diagnostics::unexpected_token(span, expectation, "malformed declaration")
}

fn sort_diagnostics(diagnostics: &mut Vec<Diagnostic>) {
    diagnostics.sort_by_key(|diagnostic| {
        let span = diagnostic.primary_span();
        (
            span.file(),
            span.range().start(),
            diagnostic.code().as_str(),
        )
    });
    diagnostics.dedup();
}

pub(crate) fn diagnostic_snapshot(diagnostics: &[Diagnostic]) -> String {
    let mut snapshot = String::new();
    for diagnostic in diagnostics {
        let range = diagnostic.primary_span().range();
        snapshot.push_str(diagnostic.code().as_str());
        snapshot.push('@');
        snapshot.push_str(&range.start().to_u32().to_string());
        snapshot.push_str("..");
        snapshot.push_str(&range.end().to_u32().to_string());
        snapshot.push('\n');
    }
    snapshot
}
