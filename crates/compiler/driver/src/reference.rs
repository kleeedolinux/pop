//! Closed public reference-metadata projection for dependent Bubbles.
//!
//! Only verified public declarations enter this representation. Unsupported
//! types fail closed, and the original `(BubbleId, SymbolId)` identity is
//! preserved through HIR and MIR.

use std::collections::BTreeMap;

use pop_foundation::{SymbolId, SymbolIdentity, TypeId};
use pop_hir::HirBubble;
use pop_resolve::ResolutionDatabase;
use pop_types::{PrimitiveType, ResolvedFunctionSignature, SemanticType, TypeArena};

use crate::api::{
    ReferenceFunction, ReferenceFunctionParameter, ReferenceMetadata, ReferenceMetadataError,
    ReferenceType,
};

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
        let parameters = function
            .parameters()
            .iter()
            .map(|parameter| {
                reference_type(identity, parameter.type_id(), arena).map(|parameter_type| {
                    ReferenceFunctionParameter {
                        name: parameter.name().to_owned(),
                        parameter_type,
                    }
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let results = function
            .results()
            .iter()
            .map(|type_id| reference_type(identity, *type_id, arena))
            .collect::<Result<Vec<_>, _>>()?;
        functions.push(ReferenceFunction {
            identity,
            module: function.module(),
            namespace: declaration.namespace().to_owned(),
            name: function.name().to_owned(),
            parameters,
            results,
            effects: function.effects(),
            span: function
                .parameters()
                .first()
                .map_or(declaration.span(), pop_hir::HirParameter::span),
        });
    }
    functions.sort_by_key(ReferenceFunction::identity);
    Ok(ReferenceMetadata {
        bubble: hir.bubble(),
        functions,
    })
}

pub(crate) fn reference_type(
    function: SymbolIdentity,
    type_id: TypeId,
    arena: &TypeArena,
) -> Result<ReferenceType, ReferenceMetadataError> {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(primitive)) => Ok(ReferenceType::Primitive(*primitive)),
        _ => Err(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
    }
}

pub(crate) fn reference_signatures(
    metadata: &[ReferenceMetadata],
    database: &ResolutionDatabase,
    arena: &TypeArena,
) -> BTreeMap<SymbolId, ResolvedFunctionSignature> {
    metadata
        .iter()
        .flat_map(ReferenceMetadata::functions)
        .map(|function| {
            let declaration = database
                .index()
                .declaration_by_reference_identity(function.identity())
                .expect("indexed reference identity");
            let parameters = function
                .parameters()
                .iter()
                .map(|parameter| {
                    (
                        parameter.name().to_owned(),
                        reference_type_id(parameter.parameter_type(), arena),
                        function.span(),
                    )
                })
                .collect();
            let results = function
                .results()
                .iter()
                .map(|result| (reference_type_id(*result, arena), function.span()))
                .collect();
            (
                declaration.symbol(),
                ResolvedFunctionSignature::referenced(
                    declaration.symbol(),
                    function.name(),
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
    arena: &TypeArena,
) -> Vec<pop_hir::HirFunctionReference> {
    metadata
        .iter()
        .flat_map(ReferenceMetadata::functions)
        .map(|function| {
            pop_hir::HirFunctionReference::new(
                function.identity(),
                function
                    .parameters()
                    .iter()
                    .map(|parameter| reference_type_id(parameter.parameter_type(), arena))
                    .collect(),
                function
                    .results()
                    .iter()
                    .map(|result| reference_type_id(*result, arena))
                    .collect(),
                function.effects(),
            )
        })
        .collect()
}

pub(crate) fn reference_type_id(reference: ReferenceType, arena: &TypeArena) -> TypeId {
    let ReferenceType::Primitive(primitive) = reference;
    let source_name = PrimitiveType::source_schema()
        .iter()
        .copied()
        .find(|entry| entry.primitive() == primitive && !entry.is_alias())
        .map(pop_types::PrimitiveSchemaEntry::source_name)
        .expect("every primitive metadata type has one canonical source name");
    arena
        .source_type(source_name)
        .expect("consumer primitive arena matches metadata schema")
}
