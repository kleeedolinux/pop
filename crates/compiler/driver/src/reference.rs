//! Closed public reference-metadata projection for dependent Bubbles.
//!
//! Only verified public declarations enter this representation. Unsupported
//! types fail closed, and the original `(BubbleId, SymbolId)` identity is
//! preserved through HIR and MIR.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{SymbolId, SymbolIdentity, TypeId};
use pop_hir::{HirBubble, HirFunction, hir_direct_call_instances, hir_direct_data_references};
use pop_resolve::ResolutionDatabase;
use pop_types::{
    PrimitiveType, ResolvedFunctionSignature, SemanticType, SignatureResolver, TypeArena,
};

use crate::api::{
    ReferenceFunction, ReferenceFunctionParameter, ReferenceMetadata, ReferenceMetadataError,
    ReferenceSpecializationCapsule, ReferenceType, ReferenceTypeParameter,
};
use crate::artifact::capsule_sha256;

pub(crate) fn invalid_reference_capsule(metadata: &[ReferenceMetadata]) -> Option<SymbolIdentity> {
    metadata
        .iter()
        .flat_map(ReferenceMetadata::functions)
        .find_map(|function| {
            if function.type_parameters().is_empty() {
                return function
                    .specialization_capsule()
                    .is_some()
                    .then_some(function.identity());
            }
            let Some(capsule) = function.specialization_capsule() else {
                return Some(function.identity());
            };
            (capsule.schema_version() != 1
                || capsule.root() != function.identity()
                || capsule.functions().is_empty()
                || !capsule
                    .functions()
                    .iter()
                    .any(|candidate| candidate.symbol() == capsule.root().symbol())
                || capsule
                    .functions()
                    .iter()
                    .any(|candidate| candidate.bubble() != function.identity().bubble())
                || capsule_sha256(
                    capsule.root(),
                    capsule.declarations(),
                    capsule.functions(),
                    capsule.methods(),
                    capsule.source_types(),
                )
                .as_deref()
                    != Some(capsule.content_sha256()))
            .then_some(function.identity())
        })
}

pub(crate) fn invalid_reference_foreign_contract(
    metadata: &[ReferenceMetadata],
) -> Option<SymbolIdentity> {
    metadata
        .iter()
        .flat_map(ReferenceMetadata::functions)
        .find_map(|function| {
            let declaration = function.foreign_declaration()?;
            let aliases_are_canonical = declaration
                .link_aliases()
                .windows(2)
                .all(|aliases| aliases[0] < aliases[1])
                && declaration
                    .link_aliases()
                    .iter()
                    .all(|alias| !alias.is_empty() && !alias.chars().any(char::is_control));
            (function.is_async()
                || !function.type_parameters().is_empty()
                || declaration.symbol() != function.identity().symbol()
                || declaration.external_symbol().is_empty()
                || declaration.external_symbol().chars().any(char::is_control)
                || !aliases_are_canonical
                || !declaration.has_valid_effects()
                || declaration.effects() != function.effects())
            .then_some(function.identity())
        })
}

pub(crate) fn emit_reference_metadata(
    hir: &HirBubble,
    index: &pop_resolve::DeclarationIndex,
    arena: &TypeArena,
) -> Result<ReferenceMetadata, ReferenceMetadataError> {
    let mut functions = Vec::new();
    for function in hir
        .functions()
        .iter()
        .filter(|function| function.visibility() == pop_resolve::Visibility::Public)
    {
        let identity = SymbolIdentity::new(hir.bubble(), function.symbol());
        let declaration = index
            .declaration(function.symbol())
            .ok_or(ReferenceMetadataError::MissingDeclaration(identity))?;
        let type_parameter_indices: BTreeMap<_, _> = function
            .type_parameters()
            .iter()
            .enumerate()
            .map(|(index, type_id)| (*type_id, u16::try_from(index).unwrap_or(u16::MAX)))
            .collect();
        let type_parameters = function
            .type_parameters()
            .iter()
            .zip(function.type_parameter_names())
            .zip(function.type_parameter_bounds())
            .map(|((_, name), bound)| {
                bound
                    .map(|bound| {
                        reference_type_with_parameters(
                            identity,
                            bound,
                            arena,
                            &type_parameter_indices,
                        )
                    })
                    .transpose()
                    .map(|bound| ReferenceTypeParameter {
                        name: name.clone(),
                        bound,
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let parameters = function
            .parameters()
            .iter()
            .map(|parameter| {
                reference_type_with_parameters(
                    identity,
                    parameter.type_id(),
                    arena,
                    &type_parameter_indices,
                )
                .map(|parameter_type| ReferenceFunctionParameter {
                    name: parameter.name().to_owned(),
                    parameter_type,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let results = function
            .results()
            .iter()
            .map(|type_id| {
                reference_type_with_parameters(identity, *type_id, arena, &type_parameter_indices)
            })
            .collect::<Result<Vec<_>, _>>()?;
        functions.push(ReferenceFunction {
            identity,
            module: function.module(),
            namespace: declaration.namespace().to_owned(),
            name: function.name().to_owned(),
            is_async: function.is_async(),
            type_parameters,
            parameters,
            results,
            effects: function.effects(),
            foreign_declaration: None,
            span: function
                .parameters()
                .first()
                .map_or(declaration.span(), pop_hir::HirParameter::span),
            specialization_capsule: specialization_capsule(hir, function, arena),
        });
    }
    for function in hir
        .foreign_functions()
        .iter()
        .filter(|function| function.visibility() == pop_resolve::Visibility::Public)
    {
        let identity = SymbolIdentity::new(hir.bubble(), function.symbol());
        let declaration = index
            .declaration(function.symbol())
            .ok_or(ReferenceMetadataError::MissingDeclaration(identity))?;
        let parameters = function
            .parameters()
            .iter()
            .map(|parameter| {
                reference_type_with_parameters(
                    identity,
                    parameter.type_id(),
                    arena,
                    &BTreeMap::new(),
                )
                .map(|parameter_type| ReferenceFunctionParameter {
                    name: parameter.name().to_owned(),
                    parameter_type,
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let results = function
            .results()
            .iter()
            .map(|type_id| {
                reference_type_with_parameters(identity, *type_id, arena, &BTreeMap::new())
            })
            .collect::<Result<Vec<_>, _>>()?;
        functions.push(ReferenceFunction {
            identity,
            module: function.module(),
            namespace: declaration.namespace().to_owned(),
            name: function.name().to_owned(),
            is_async: false,
            type_parameters: Vec::new(),
            parameters,
            results,
            effects: function.effects(),
            foreign_declaration: Some(function.declaration().clone()),
            span: function.declaration().span(),
            specialization_capsule: None,
        });
    }
    functions.sort_by_key(ReferenceFunction::identity);
    Ok(ReferenceMetadata {
        bubble: hir.bubble(),
        functions,
    })
}

fn specialization_capsule(
    hir: &HirBubble,
    root: &HirFunction,
    arena: &TypeArena,
) -> Option<ReferenceSpecializationCapsule> {
    if root.type_parameters().is_empty() {
        return None;
    }
    let functions_by_symbol: BTreeMap<_, _> = hir
        .functions()
        .iter()
        .map(|function| (function.symbol(), function))
        .collect();
    let mut pending = BTreeSet::from([root.symbol()]);
    let mut included = BTreeSet::new();
    while let Some(symbol) = pending.pop_first() {
        if !included.insert(symbol) {
            continue;
        }
        let function = functions_by_symbol.get(&symbol)?;
        pending.extend(
            hir_direct_call_instances(function)
                .into_iter()
                .map(|(callee, _)| callee),
        );
    }
    let functions = included
        .into_iter()
        .filter_map(|symbol| functions_by_symbol.get(&symbol).copied().cloned())
        .collect::<Vec<_>>();
    let mut pending_classes = BTreeSet::new();
    let mut pending_methods = BTreeSet::new();
    for function in &functions {
        let (classes, methods) = hir_direct_data_references(function);
        pending_classes.extend(classes);
        pending_methods.extend(methods);
    }
    let mut included_classes = BTreeSet::new();
    let mut included_methods = BTreeSet::new();
    while !pending_classes.is_empty() || !pending_methods.is_empty() {
        if let Some(class) = pending_classes.pop_first() {
            if included_classes.insert(class) {
                pending_methods.extend(
                    hir.methods()
                        .iter()
                        .filter(|method| method.class() == class)
                        .map(pop_hir::HirMethod::method),
                );
            }
            continue;
        }
        let Some(method) = pending_methods.pop_first() else {
            continue;
        };
        if !included_methods.insert(method) {
            continue;
        }
        let implementation = hir
            .methods()
            .iter()
            .find(|candidate| candidate.method() == method)?;
        pending_classes.insert(implementation.class());
        let (classes, methods) = hir_direct_data_references(implementation.function());
        pending_classes.extend(classes);
        pending_methods.extend(methods);
    }
    let declarations = hir
        .declarations()
        .iter()
        .filter(|declaration| {
            matches!(declaration.kind(), pop_hir::HirDeclarationKind::Class(class)
                if included_classes.contains(&class.class()))
        })
        .cloned()
        .collect::<Vec<_>>();
    let methods = hir
        .methods()
        .iter()
        .filter(|method| included_methods.contains(&method.method()))
        .cloned()
        .collect::<Vec<_>>();
    let identity = SymbolIdentity::new(hir.bubble(), root.symbol());
    let content_sha256 = capsule_sha256(identity, &declarations, &functions, &methods, arena)?;
    Some(ReferenceSpecializationCapsule {
        schema_version: 1,
        content_sha256,
        root: identity,
        declarations,
        functions,
        methods,
        source_types: arena.clone(),
    })
}

fn reference_type_with_parameters(
    function: SymbolIdentity,
    type_id: TypeId,
    arena: &TypeArena,
    type_parameters: &BTreeMap<TypeId, u16>,
) -> Result<ReferenceType, ReferenceMetadataError> {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(primitive)) => Ok(ReferenceType::Primitive(*primitive)),
        Some(SemanticType::TypeParameter(_)) => type_parameters
            .get(&type_id)
            .copied()
            .map(ReferenceType::TypeParameter)
            .ok_or(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
        Some(SemanticType::Tuple(elements)) => Ok(ReferenceType::Tuple(
            elements
                .iter()
                .map(|element| {
                    reference_type_with_parameters(function, *element, arena, type_parameters)
                })
                .collect::<Result<_, _>>()?,
        )),
        Some(SemanticType::Function {
            is_async,
            parameters,
            results,
            effects,
        }) => Ok(ReferenceType::Function {
            is_async: *is_async,
            parameters: parameters
                .iter()
                .map(|parameter| {
                    reference_type_with_parameters(function, *parameter, arena, type_parameters)
                })
                .collect::<Result<_, _>>()?,
            results: results
                .iter()
                .map(|result| {
                    reference_type_with_parameters(function, *result, arena, type_parameters)
                })
                .collect::<Result<_, _>>()?,
            effects: *effects,
        }),
        Some(SemanticType::Array(element)) => Ok(ReferenceType::Array(Box::new(
            reference_type_with_parameters(function, *element, arena, type_parameters)?,
        ))),
        Some(SemanticType::Table { key, value }) => Ok(ReferenceType::Table {
            key: Box::new(reference_type_with_parameters(
                function,
                *key,
                arena,
                type_parameters,
            )?),
            value: Box::new(reference_type_with_parameters(
                function,
                *value,
                arena,
                type_parameters,
            )?),
        }),
        Some(SemanticType::Optional(element)) => Ok(ReferenceType::Optional(Box::new(
            reference_type_with_parameters(function, *element, arena, type_parameters)?,
        ))),
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) => Ok(ReferenceType::Builtin {
            definition: *definition,
            arguments: arguments
                .iter()
                .map(|argument| {
                    reference_type_with_parameters(function, *argument, arena, type_parameters)
                })
                .collect::<Result<_, _>>()?,
        }),
        Some(SemanticType::Union(elements)) => Ok(ReferenceType::Union(
            elements
                .iter()
                .map(|element| {
                    reference_type_with_parameters(function, *element, arena, type_parameters)
                })
                .collect::<Result<_, _>>()?,
        )),
        _ => Err(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
    }
}

pub(crate) fn reference_signatures(
    metadata: &[ReferenceMetadata],
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
) -> BTreeMap<SymbolId, ResolvedFunctionSignature> {
    metadata
        .iter()
        .flat_map(ReferenceMetadata::functions)
        .map(|function| {
            let declaration = database
                .index()
                .declaration_by_reference_identity(function.identity())
                .expect("indexed reference identity");
            let mut type_parameters = Vec::new();
            let mut parameter_types = Vec::new();
            for parameter in function.type_parameters() {
                let bound = parameter
                    .bound()
                    .map(|bound| reference_type_id(bound, resolver.arena_mut(), &parameter_types));
                let resolved =
                    resolver.referenced_type_parameter(parameter.name(), bound, function.span());
                parameter_types.push(resolved.type_id());
                type_parameters.push(resolved);
            }
            let parameters = function
                .parameters()
                .iter()
                .map(|parameter| {
                    (
                        parameter.name().to_owned(),
                        reference_type_id(
                            parameter.parameter_type(),
                            resolver.arena_mut(),
                            &parameter_types,
                        ),
                        function.span(),
                    )
                })
                .collect();
            let results = function
                .results()
                .iter()
                .map(|result| {
                    (
                        reference_type_id(result, resolver.arena_mut(), &parameter_types),
                        function.span(),
                    )
                })
                .collect();
            (
                declaration.symbol(),
                ResolvedFunctionSignature::referenced_generic(
                    declaration.symbol(),
                    function.name(),
                    function.is_async(),
                    type_parameters,
                    parameters,
                    results,
                    function.effects(),
                ),
            )
        })
        .collect()
}

pub(crate) fn hir_function_references(
    metadata: &[ReferenceMetadata],
    database: &ResolutionDatabase,
    signatures: &BTreeMap<SymbolId, ResolvedFunctionSignature>,
    consumer_bubble: pop_foundation::BubbleId,
    resolver: &mut SignatureResolver<'_>,
    referenced_call_instances: &[(SymbolIdentity, Vec<TypeId>)],
) -> Vec<pop_hir::HirFunctionReference> {
    let mut next_symbol = database
        .index()
        .declarations()
        .map(|declaration| declaration.symbol().raw())
        .max()
        .unwrap_or(0)
        .saturating_add(1);
    let mut capsule_symbols = BTreeMap::new();
    metadata
        .iter()
        .flat_map(ReferenceMetadata::functions)
        .map(|function| {
            let symbol = database
                .index()
                .declaration_by_reference_identity(function.identity())
                .expect("indexed reference identity")
                .symbol();
            let signature = signatures.get(&symbol).expect("referenced signature");
            let reference = pop_hir::HirFunctionReference::new(
                function.identity(),
                function.is_async(),
                signature
                    .type_parameters()
                    .iter()
                    .map(pop_types::ResolvedTypeParameter::type_id)
                    .collect(),
                signature
                    .type_parameters()
                    .iter()
                    .map(pop_types::ResolvedTypeParameter::bound)
                    .collect(),
                signature
                    .parameters()
                    .iter()
                    .filter_map(|parameter| parameter.parameter_type().type_id())
                    .collect(),
                signature
                    .results()
                    .iter()
                    .filter_map(pop_types::ResolvedType::type_id)
                    .collect(),
                function.effects(),
            )
            .with_foreign_declaration(function.foreign_declaration().cloned());
            let Some(capsule) = function.specialization_capsule() else {
                return reference;
            };
            if capsule.schema_version() != 1
                || capsule.root() != function.identity()
                || capsule_sha256(
                    capsule.root(),
                    capsule.declarations(),
                    capsule.functions(),
                    capsule.methods(),
                    capsule.source_types(),
                )
                .as_deref()
                    != Some(capsule.content_sha256())
            {
                return reference;
            }
            let next_class = capsule
                .declarations()
                .iter()
                .filter_map(|declaration| match declaration.kind() {
                    pop_hir::HirDeclarationKind::Class(class) => Some(class.class().raw()),
                    _ => None,
                })
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let next_field = capsule
                .declarations()
                .iter()
                .filter_map(|declaration| match declaration.kind() {
                    pop_hir::HirDeclarationKind::Class(class) => Some(class.fields()),
                    _ => None,
                })
                .flatten()
                .map(|field| field.field().raw())
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            let next_method = capsule
                .methods()
                .iter()
                .map(|method| method.method().raw())
                .max()
                .unwrap_or(0)
                .saturating_add(1);
            resolver.reserve_capsule_identifiers(next_class, next_field, next_method);
            let root_source = capsule.root().symbol();
            capsule_symbols.insert(capsule.root(), symbol);
            let mut symbol_map = BTreeMap::new();
            for source in capsule
                .declarations()
                .iter()
                .map(pop_hir::HirDeclaration::symbol)
                .chain(capsule.functions().iter().map(HirFunction::symbol))
            {
                let identity = SymbolIdentity::new(function.identity().bubble(), source);
                let target = *capsule_symbols.entry(identity).or_insert_with(|| {
                    let allocated = SymbolId::from_raw(next_symbol);
                    next_symbol = next_symbol.saturating_add(1);
                    allocated
                });
                symbol_map.insert(source, target);
            }
            symbol_map.insert(root_source, symbol);
            let root_type_parameters = signature
                .type_parameters()
                .iter()
                .map(pop_types::ResolvedTypeParameter::type_id)
                .collect::<Vec<_>>();
            let mut function_type_parameters = BTreeMap::new();
            function_type_parameters.insert(root_source, root_type_parameters);
            for source in capsule
                .functions()
                .iter()
                .filter(|source| source.symbol() != root_source)
            {
                let type_parameters = source
                    .type_parameter_names()
                    .iter()
                    .map(|name| {
                        resolver
                            .referenced_type_parameter(name, None, function.span())
                            .type_id()
                    })
                    .collect::<Vec<_>>();
                function_type_parameters.insert(source.symbol(), type_parameters);
            }
            let mut type_map = BTreeMap::new();
            for source in capsule.functions() {
                for (source_type, target_type) in source
                    .type_parameters()
                    .iter()
                    .zip(&function_type_parameters[&source.symbol()])
                {
                    type_map.insert(*source_type, *target_type);
                }
            }
            for raw in 0..capsule.source_types().len() {
                let source_type = TypeId::from_raw(u32::try_from(raw).unwrap_or(u32::MAX));
                if type_map.contains_key(&source_type)
                    || !matches!(
                        capsule.source_types().get(source_type),
                        Some(SemanticType::TypeParameter(_))
                    )
                {
                    continue;
                }
                let target = resolver
                    .referenced_type_parameter("CapsuleType", None, function.span())
                    .type_id();
                type_map.insert(source_type, target);
            }
            for raw in 0..capsule.source_types().len() {
                let source_type = TypeId::from_raw(u32::try_from(raw).unwrap_or(u32::MAX));
                let _ = import_capsule_type(
                    source_type,
                    capsule.source_types(),
                    resolver.arena_mut(),
                    &mut type_map,
                );
            }
            let capsule_classes = capsule
                .declarations()
                .iter()
                .filter_map(|declaration| match declaration.kind() {
                    pop_hir::HirDeclarationKind::Class(class) => Some(class.class()),
                    _ => None,
                })
                .collect::<BTreeSet<_>>();
            for (class, arguments, concrete) in capsule.source_types().class_specializations() {
                if !capsule_classes.contains(&class) {
                    continue;
                }
                let Some(arguments) = arguments
                    .iter()
                    .map(|argument| type_map.get(argument).copied())
                    .collect::<Option<Vec<_>>>()
                else {
                    continue;
                };
                let Some(concrete) = type_map.get(&concrete).copied() else {
                    continue;
                };
                let _ = resolver
                    .arena_mut()
                    .register_class_specialization(class, arguments, concrete);
            }
            let class_map = capsule
                .declarations()
                .iter()
                .filter_map(|declaration| {
                    let pop_hir::HirDeclarationKind::Class(class) = declaration.kind() else {
                        return None;
                    };
                    Some((
                        *type_map.get(&class.type_id())?,
                        (symbol_map[&declaration.symbol()], class.class()),
                    ))
                })
                .collect::<BTreeMap<_, _>>();
            let mut specialized_declarations = Vec::new();
            let mut specialized_methods = Vec::new();
            for (_, arguments) in referenced_call_instances
                .iter()
                .filter(|(identity, _)| *identity == function.identity())
            {
                let Some(root) = capsule
                    .functions()
                    .iter()
                    .find(|candidate| candidate.symbol() == root_source)
                else {
                    continue;
                };
                if arguments.len() != root.type_parameters().len() {
                    continue;
                }
                let mut concrete_types = root
                    .type_parameters()
                    .iter()
                    .copied()
                    .zip(arguments.iter().copied())
                    .collect::<BTreeMap<_, _>>();
                let root_parameters = root
                    .type_parameters()
                    .iter()
                    .copied()
                    .collect::<BTreeSet<_>>();
                let class_specializations = capsule
                    .source_types()
                    .class_specializations()
                    .map(|(class, arguments, concrete)| (class, arguments.to_vec(), concrete))
                    .collect::<Vec<_>>();
                for (source_class, symbolic_arguments, symbolic_type) in class_specializations {
                    if !symbolic_arguments
                        .iter()
                        .any(|argument| root_parameters.contains(argument))
                    {
                        continue;
                    }
                    let Some(template) = capsule.declarations().iter().find(|declaration| {
                        matches!(declaration.kind(), pop_hir::HirDeclarationKind::Class(class)
                            if class.class() == source_class
                                && capsule.source_types().contains_type_parameter(class.type_id()))
                    }) else {
                        continue;
                    };
                    let pop_hir::HirDeclarationKind::Class(template_class) = template.kind() else {
                        continue;
                    };
                    let Some(SemanticType::Class {
                        arguments: template_arguments,
                        ..
                    }) = capsule.source_types().get(template_class.type_id())
                    else {
                        continue;
                    };
                    let concrete_arguments = symbolic_arguments
                        .iter()
                        .map(|argument| {
                            import_capsule_type(
                                *argument,
                                capsule.source_types(),
                                resolver.arena_mut(),
                                &mut concrete_types,
                            )
                        })
                        .collect::<Option<Vec<_>>>();
                    let Some(concrete_arguments) = concrete_arguments else {
                        continue;
                    };
                    if concrete_arguments
                        .iter()
                        .any(|argument| resolver.arena().contains_type_parameter(*argument))
                    {
                        continue;
                    }
                    let concrete_class = resolver.allocate_capsule_class();
                    let concrete_type = resolver
                        .arena_mut()
                        .intern(SemanticType::Class {
                            class: concrete_class,
                            arguments: concrete_arguments.clone(),
                        })
                        .ok();
                    let Some(concrete_type) = concrete_type else {
                        continue;
                    };
                    if let Some(symbolic_target) = type_map.get(&symbolic_type).copied()
                        && let Some(SemanticType::Class { class, .. }) =
                            resolver.arena().get(symbolic_target)
                    {
                        let symbolic_class = *class;
                        let _ = resolver.arena_mut().register_class_specialization(
                            symbolic_class,
                            concrete_arguments.clone(),
                            concrete_type,
                        );
                    }
                    concrete_types.insert(symbolic_type, concrete_type);
                    let mut specialized_types = type_map
                        .iter()
                        .filter(|(source, _)| {
                            !capsule.source_types().contains_type_parameter(**source)
                        })
                        .map(|(source, target)| (*source, *target))
                        .collect::<BTreeMap<_, _>>();
                    for raw in 0..capsule.source_types().len() {
                        let source = TypeId::from_raw(u32::try_from(raw).unwrap_or(u32::MAX));
                        if !matches!(
                            capsule.source_types().get(source),
                            Some(SemanticType::TypeParameter(_))
                        ) {
                            continue;
                        }
                        if let Some(target) = concrete_types
                            .get(&source)
                            .or_else(|| type_map.get(&source))
                            .copied()
                        {
                            specialized_types.insert(source, target);
                        }
                    }
                    for (parameter, argument) in template_arguments.iter().zip(&concrete_arguments)
                    {
                        specialized_types.insert(*parameter, *argument);
                    }
                    specialized_types.insert(template_class.type_id(), concrete_type);
                    for raw in 0..capsule.source_types().len() {
                        let source_type = TypeId::from_raw(u32::try_from(raw).unwrap_or(u32::MAX));
                        let _ = import_capsule_type(
                            source_type,
                            capsule.source_types(),
                            resolver.arena_mut(),
                            &mut specialized_types,
                        );
                    }
                    let fields = template_class
                        .fields()
                        .iter()
                        .map(|field| (field.field(), resolver.allocate_capsule_field()))
                        .collect::<BTreeMap<_, _>>();
                    let methods = template_class
                        .methods()
                        .iter()
                        .map(|method| (method.method(), resolver.allocate_capsule_method()))
                        .collect::<BTreeMap<_, _>>();
                    let class_symbol = SymbolId::from_raw(next_symbol);
                    next_symbol = next_symbol.saturating_add(1);
                    if let Some((declaration, mut methods)) =
                        pop_hir::rebind_hir_class_specialization(
                            template,
                            capsule.methods(),
                            class_symbol,
                            consumer_bubble,
                            concrete_type,
                            concrete_class,
                            &specialized_types,
                            &fields,
                            &methods,
                            &symbol_map,
                        )
                    {
                        specialized_declarations.push(declaration);
                        specialized_methods.append(&mut methods);
                    }
                }
                for raw in 0..capsule.source_types().len() {
                    let source_type = TypeId::from_raw(u32::try_from(raw).unwrap_or(u32::MAX));
                    let _ = import_capsule_type(
                        source_type,
                        capsule.source_types(),
                        resolver.arena_mut(),
                        &mut concrete_types,
                    );
                }
            }
            let functions = capsule
                .functions()
                .iter()
                .map(|source| {
                    pop_hir::rebind_hir_function_template(
                        source,
                        symbol_map[&source.symbol()],
                        consumer_bubble,
                        &function_type_parameters[&source.symbol()],
                        &type_map,
                        &symbol_map,
                        &class_map,
                        &BTreeMap::new(),
                    )
                })
                .collect::<Option<Vec<_>>>();
            let declarations = capsule
                .declarations()
                .iter()
                .map(|source| {
                    pop_hir::rebind_hir_class_declaration(
                        source,
                        symbol_map[&source.symbol()],
                        consumer_bubble,
                        &type_map,
                    )
                })
                .collect::<Option<Vec<_>>>()
                .map(|mut declarations| {
                    declarations.append(&mut specialized_declarations);
                    declarations
                });
            let methods = capsule
                .methods()
                .iter()
                .map(|source| {
                    pop_hir::rebind_hir_method_template(
                        source,
                        symbol_map[&source.definition()],
                        consumer_bubble,
                        &type_map,
                        &symbol_map,
                        &class_map,
                        &BTreeMap::new(),
                    )
                })
                .collect::<Option<Vec<_>>>()
                .map(|mut methods| {
                    methods.append(&mut specialized_methods);
                    methods
                });
            functions.zip(declarations).zip(methods).map_or(
                reference.clone(),
                |((functions, declarations), methods)| {
                    reference.with_specialization_capsule(pop_hir::HirSpecializationCapsule::new(
                        function.identity(),
                        symbol,
                        declarations,
                        functions,
                        methods,
                    ))
                },
            )
        })
        .collect()
}

fn import_capsule_type(
    source: TypeId,
    source_arena: &TypeArena,
    target_arena: &mut TypeArena,
    imported: &mut BTreeMap<TypeId, TypeId>,
) -> Option<TypeId> {
    if let Some(target) = imported.get(&source) {
        return Some(*target);
    }
    let semantic = source_arena.get(source)?.clone();
    let mut import = |type_id| import_capsule_type(type_id, source_arena, target_arena, imported);
    let target_semantic = match semantic {
        SemanticType::Primitive(primitive) => SemanticType::Primitive(primitive),
        SemanticType::Tuple(elements) => SemanticType::Tuple(
            elements
                .into_iter()
                .map(&mut import)
                .collect::<Option<_>>()?,
        ),
        SemanticType::Function {
            is_async,
            parameters,
            results,
            effects,
        } => SemanticType::Function {
            is_async,
            parameters: parameters
                .into_iter()
                .map(&mut import)
                .collect::<Option<_>>()?,
            results: results
                .into_iter()
                .map(&mut import)
                .collect::<Option<_>>()?,
            effects,
        },
        SemanticType::Record(fields) => SemanticType::Record(
            fields
                .into_iter()
                .map(|(name, field_type)| import(field_type).map(|field_type| (name, field_type)))
                .collect::<Option<_>>()?,
        ),
        SemanticType::Array(element) => SemanticType::Array(import(element)?),
        SemanticType::Table { key, value } => SemanticType::Table {
            key: import(key)?,
            value: import(value)?,
        },
        SemanticType::Builtin {
            definition,
            arguments,
        } => SemanticType::Builtin {
            definition,
            arguments: arguments
                .into_iter()
                .map(&mut import)
                .collect::<Option<_>>()?,
        },
        SemanticType::Union(elements) => SemanticType::Union(
            elements
                .into_iter()
                .map(&mut import)
                .collect::<Option<_>>()?,
        ),
        SemanticType::Optional(element) => SemanticType::Optional(import(element)?),
        SemanticType::TypeParameter(_) => return imported.get(&source).copied(),
        SemanticType::Class { class, arguments } => SemanticType::Class {
            class,
            arguments: arguments
                .into_iter()
                .map(&mut import)
                .collect::<Option<_>>()?,
        },
        SemanticType::TaggedUnion { .. }
        | SemanticType::ErrorUnion { .. }
        | SemanticType::Enum { .. }
        | SemanticType::Interface { .. }
        | SemanticType::Attribute { .. }
        | SemanticType::Opaque(_)
        | SemanticType::Error => return None,
    };
    let target = target_arena
        .find(&target_semantic)
        .or_else(|| target_arena.intern(target_semantic).ok())?;
    imported.insert(source, target);
    Some(target)
}

#[cfg(test)]
mod capsule_tests {
    use pop_foundation::{BubbleId, FileId, ModuleId, NamespaceId};
    use pop_source::SourceFile;

    use super::super::{FrontEndBubbleInput, FrontEndModule, analyze_bubble};

    #[test]
    fn malformed_capsule_hash_fails_closed_before_hir_loading() {
        let library_bubble = BubbleId::from_raw(2);
        let source = SourceFile::new(
            FileId::from_raw(0),
            "src/generic.pop",
            "namespace Pop.Sequence\npublic function identity<T>(value: T): T\n    return value\nend\n",
        )
        .expect("source");
        let library = analyze_bubble(FrontEndBubbleInput::new(
            library_bubble,
            NamespaceId::from_raw(2),
            Vec::new(),
            vec![FrontEndModule::new(ModuleId::from_raw(0), source)],
        ));
        let mut metadata = library.reference_metadata().expect("metadata").clone();
        metadata.functions[0]
            .specialization_capsule
            .as_mut()
            .expect("capsule")
            .content_sha256
            .replace_range(0..1, "z");
        let consumer_source = SourceFile::new(
            FileId::from_raw(1),
            "src/main.pop",
            "namespace Application\nusing Pop.Sequence\npublic function run(): Int\n    return identity(42)\nend\n",
        )
        .expect("consumer source");
        let consumer = analyze_bubble(
            FrontEndBubbleInput::new(
                BubbleId::from_raw(7),
                NamespaceId::from_raw(7),
                vec![library_bubble],
                vec![FrontEndModule::new(ModuleId::from_raw(1), consumer_source)],
            )
            .with_reference_metadata(vec![metadata]),
        );
        assert!(consumer.hir().is_none());
        assert!(matches!(
            consumer.hir_bubble_error(),
            Some(pop_hir::HirBubbleError::InvalidSpecializationCapsule(_))
        ));
    }
}

pub(crate) fn reference_type_id(
    reference: &ReferenceType,
    arena: &mut TypeArena,
    type_parameters: &[TypeId],
) -> TypeId {
    match reference {
        ReferenceType::Primitive(primitive) => {
            let source_name = PrimitiveType::source_schema()
                .iter()
                .copied()
                .find(|entry| entry.primitive() == *primitive && !entry.is_alias())
                .map(pop_types::PrimitiveSchemaEntry::source_name)
                .expect("every primitive metadata type has one canonical source name");
            arena
                .source_type(source_name)
                .expect("consumer primitive arena matches metadata schema")
        }
        ReferenceType::TypeParameter(index) => type_parameters[usize::from(*index)],
        ReferenceType::Tuple(elements) => {
            let elements = elements
                .iter()
                .map(|element| reference_type_id(element, arena, type_parameters))
                .collect();
            arena
                .intern(SemanticType::Tuple(elements))
                .expect("verified tuple metadata")
        }
        ReferenceType::Function {
            is_async,
            parameters,
            results,
            effects,
        } => {
            let parameters = parameters
                .iter()
                .map(|parameter| reference_type_id(parameter, arena, type_parameters))
                .collect();
            let results = results
                .iter()
                .map(|result| reference_type_id(result, arena, type_parameters))
                .collect();
            arena
                .intern(SemanticType::Function {
                    is_async: *is_async,
                    parameters,
                    results,
                    effects: *effects,
                })
                .expect("verified function metadata")
        }
        ReferenceType::Array(element) => {
            let element = reference_type_id(element, arena, type_parameters);
            arena
                .intern(SemanticType::Array(element))
                .expect("verified array metadata")
        }
        ReferenceType::Table { key, value } => {
            let key = reference_type_id(key, arena, type_parameters);
            let value = reference_type_id(value, arena, type_parameters);
            arena
                .intern(SemanticType::Table { key, value })
                .expect("verified table metadata")
        }
        ReferenceType::Optional(element) => {
            let element = reference_type_id(element, arena, type_parameters);
            arena.optional(element).expect("verified optional metadata")
        }
        ReferenceType::Builtin {
            definition,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| reference_type_id(argument, arena, type_parameters))
                .collect();
            arena
                .intern(SemanticType::Builtin {
                    definition: *definition,
                    arguments,
                })
                .expect("verified built-in metadata")
        }
        ReferenceType::Union(elements) => {
            let elements = elements
                .iter()
                .map(|element| reference_type_id(element, arena, type_parameters))
                .collect();
            arena
                .intern(SemanticType::Union(elements))
                .expect("verified union metadata")
        }
    }
}
