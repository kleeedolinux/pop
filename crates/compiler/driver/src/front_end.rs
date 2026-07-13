//! Front-end orchestration implementation.
#![allow(clippy::match_same_arms, clippy::similar_names)]

use std::collections::{BTreeMap, BTreeSet};

use pop_diagnostics::compile_time as compile_time_diagnostics;
use pop_diagnostics::syntax as syntax_diagnostics;
use pop_foundation::{BubbleId, Diagnostic, MethodId, ModuleId, SourceSpan, SymbolId};
use pop_hir::{
    HirBubble, HirDeclaration, HirDeclarationKind, HirFunction, HirFunctionContext,
    HirKnownCallables, HirMethod, build_hir_function_with_known_callables_and_attributes,
    build_hir_method,
};
use pop_resolve::{ModuleInput, ResolutionDatabase, SymbolSpace, build_declaration_index};
use pop_syntax::{
    AttributeUseSyntax, NodeKind, parse_attribute_declaration, parse_attribute_use,
    parse_class_declaration, parse_class_method_body, parse_const_declaration, parse_file,
    parse_function_body, parse_function_signature, parse_interface_declaration,
    parse_record_declaration, parse_union_declaration,
};
use pop_types::{
    AttributeTarget, BodyChecker, BootstrapSchema, ResolvedFunctionSignature, SignatureResolver,
    embedded_bootstrap_schema,
};

use crate::api::*;
use crate::attributes::{classify_function_attributes, resolve_source_attributes};
use crate::compile_time::{
    build_compile_time_context, check_compile_time_function_bodies, evaluate_declaration_defaults,
    evaluate_source_constants,
};
use crate::reference::{emit_reference_metadata, hir_function_references, reference_signatures};
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
    let database = ResolutionDatabase::new(index);
    let bootstrap = embedded_bootstrap_schema().expect("repository-validated bootstrap schema");
    validate_standard_native_exports(&bootstrap, pop_standard::NATIVE_EXPORTS)
        .expect("repository-validated native Standard adapters");
    let mut resolver = SignatureResolver::new(&database, bootstrap.clone());
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
    let mut signatures =
        reference_signatures(&input.reference_metadata, &database, resolver.arena());
    signatures.extend(
        functions
            .iter()
            .map(|function| (function.signature.symbol(), function.signature.clone())),
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
    declarations = refresh_declarations(declarations, &resolver);
    let (hir_functions, hir_methods, hir_build_failed) = build_runtime_hir(
        input.bubble,
        &mut functions,
        &methods,
        &signatures,
        &mut resolver,
        &mut diagnostics,
    );
    sort_diagnostics(&mut diagnostics);
    let hir = if diagnostics.is_empty() && !hir_build_failed {
        HirBubble::new_with_declarations_and_methods(
            input.bubble,
            input.namespace,
            input.dependencies,
            declarations,
            hir_functions,
            hir_methods,
        )
        .and_then(|bubble| {
            bubble.with_function_references(hir_function_references(
                &input.reference_metadata,
                resolver.arena(),
            ))
        })
        .ok()
    } else {
        None
    };
    let reference_metadata = hir
        .as_ref()
        .map_or(Err(ReferenceMetadataError::AnalysisUnavailable), |hir| {
            emit_reference_metadata(hir, database.index(), resolver.arena())
        });
    FrontEndResult {
        hir,
        types: resolver.into_arena(),
        attribute_queries,
        compile_time_evaluations,
        constants,
        diagnostics,
        reference_metadata,
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
                    NodeKind::AttributeDeclaration
                        | NodeKind::ConstDeclaration
                        | NodeKind::RecordDeclaration
                        | NodeKind::UnionDeclaration
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
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
) -> (Vec<HirFunction>, Vec<HirMethod>, bool) {
    let known_functions: BTreeSet<_> = functions
        .iter()
        .filter(|function| !function.is_compile_time)
        .map(|function| function.signature.symbol())
        .collect();
    let known_methods: BTreeSet<MethodId> =
        methods.iter().map(|work| work.method.method()).collect();
    let interfaces: Vec<_> = resolver.interface_definitions().cloned().collect();
    let mut hir_functions = Vec::new();
    let mut hir_build_failed = false;
    for function in functions {
        if function.is_compile_time {
            continue;
        }
        let typed = BodyChecker::new(function.module, resolver, signatures)
            .check(&function.signature, &function.body);
        diagnostics.extend(typed.diagnostics().iter().cloned());
        let Some(body) = typed.body() else {
            continue;
        };
        match build_hir_function_with_known_callables_and_attributes(
            HirFunctionContext::new(function.module, bubble, function.visibility),
            &function.signature,
            body,
            resolver.arena(),
            HirKnownCallables::new(&known_functions, &known_methods).with_interfaces(&interfaces),
            &function.attributes,
        ) {
            Ok(function) => hir_functions.push(function),
            Err(_) => hir_build_failed = true,
        }
    }
    let mut hir_methods = Vec::new();
    for method in methods {
        let typed = BodyChecker::new(method.module, resolver, signatures)
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
            Err(_) => hir_build_failed = true,
        }
    }
    (hir_functions, hir_methods, hir_build_failed)
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
            if let Some(function) = resolve_function(
                module,
                node,
                database,
                bootstrap,
                resolver,
                diagnostics,
                std::mem::take(&mut pending_attributes),
            ) {
                functions.push(function);
            }
        }
    }
    functions.sort_by_key(|function| function.signature.symbol());
    functions
}

fn resolve_function(
    module: &ParsedModule,
    node: &pop_syntax::SyntaxNode,
    database: &ResolutionDatabase,
    bootstrap: &BootstrapSchema,
    resolver: &mut SignatureResolver<'_>,
    diagnostics: &mut Vec<Diagnostic>,
    attribute_uses: Vec<AttributeUseSyntax>,
) -> Option<FunctionWork> {
    let syntax_signature = parse_function_signature(&module.source, &module.syntax, node)
        .map_err(|error| syntax_error(error.span(), error.expectation()))
        .map_err(|diagnostic| diagnostics.push(diagnostic))
        .ok()?;
    let body = parse_function_body(&module.source, &module.syntax, node, &syntax_signature)
        .map_err(|error| syntax_error(error.span(), error.expectation()))
        .map_err(|diagnostic| diagnostics.push(diagnostic))
        .ok()?;
    let span = SourceSpan::new(module.source.id(), syntax_signature.range());
    let symbol = resolve_symbol(
        database,
        module.module,
        syntax_signature.name(),
        SymbolSpace::Value,
        span,
        diagnostics,
    )?;
    let declaration = database.index().declaration(symbol)?;
    let result = resolver.resolve(module.module, symbol, &syntax_signature);
    diagnostics.extend(result.diagnostics().iter().cloned());
    let (is_compile_time, attribute_uses) =
        classify_function_attributes(database, bootstrap, module.module, attribute_uses);
    Some(FunctionWork {
        module: module.module,
        visibility: declaration.visibility(),
        span,
        body,
        signature: result.signature()?.clone(),
        is_compile_time,
        attribute_uses,
        attributes: Vec::new(),
    })
}

fn refresh_declarations(
    declarations: Vec<HirDeclaration>,
    resolver: &SignatureResolver<'_>,
) -> Vec<HirDeclaration> {
    declarations
        .into_iter()
        .map(|declaration| {
            if matches!(declaration.kind(), HirDeclarationKind::Attribute(_)) {
                if let Some(definition) = resolver.attribute_definition(declaration.symbol()) {
                    return HirDeclaration::attribute(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    );
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Record(_)) {
                if let Some(definition) = resolver.record_definition(declaration.symbol()) {
                    return HirDeclaration::record(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    );
                }
            }
            if matches!(declaration.kind(), HirDeclarationKind::Class(_)) {
                if let Some(definition) = resolver.class_definition(declaration.symbol()) {
                    return HirDeclaration::class(
                        declaration.module(),
                        declaration.bubble(),
                        declaration.visibility(),
                        declaration.name(),
                        definition,
                    );
                }
            }
            declaration
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
