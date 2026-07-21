//! Closed public reference-metadata projection for dependent Bubbles.
//!
//! Only verified public declarations enter this representation. Unsupported
//! types fail closed, and the original `(BubbleId, SymbolId)` identity is
//! preserved through HIR and MIR.

use std::collections::{BTreeMap, BTreeSet};

use pop_foundation::{SymbolId, SymbolIdentity, TypeId};
use pop_hir::{
    HirBubble, HirDeclarationKind, HirFfiLayout as ImportedHirFfiLayout,
    HirFfiLayoutCatalog as ImportedHirFfiLayoutCatalog,
    HirFfiLayoutField as ImportedHirFfiLayoutField, HirFfiValueClass as ImportedHirFfiValueClass,
    HirFunction, hir_direct_call_instances, hir_direct_data_references,
};
use pop_resolve::ResolutionDatabase;
use pop_types::{
    PrimitiveType, ResolvedFunctionSignature, SemanticType, SignatureResolver, TypeArena,
};

use crate::api::{
    ReferenceClass, ReferenceFfiLayout, ReferenceFfiLayoutCatalog, ReferenceFfiLayoutField,
    ReferenceFfiValueClass, ReferenceFunction, ReferenceFunctionParameter, ReferenceInterface,
    ReferenceMetadata, ReferenceMetadataError, ReferenceNominalType, ReferenceRecord,
    ReferenceRecordField, ReferenceSpecializationCapsule, ReferenceType, ReferenceTypeParameter,
};
use crate::artifact::{artifact_sha256_hex, capsule_sha256};
use crate::retained_metadata::RetainedMetadataArtifacts;

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
                || !declaration.has_valid_callback_pairs()
                || !declaration.callback_pairs().is_empty()
                || declaration.effects() != function.effects())
            .then_some(function.identity())
        })
}

pub(crate) fn validate_reference_nominals(metadata: &ReferenceMetadata) -> Result<(), ()> {
    if !metadata
        .interfaces()
        .windows(2)
        .all(|pair| pair[0].identity() < pair[1].identity())
        || !metadata
            .classes()
            .windows(2)
            .all(|pair| pair[0].identity() < pair[1].identity())
        || metadata.interfaces().iter().any(|interface| {
            interface.identity().bubble() != metadata.bubble()
                || interface.name().is_empty()
                || interface.namespace().is_empty()
        })
        || metadata.classes().iter().any(|class| {
            class.identity().bubble() != metadata.bubble()
                || class.name().is_empty()
                || class.namespace().is_empty()
        })
    {
        return Err(());
    }
    let interfaces = metadata
        .interfaces()
        .iter()
        .map(|interface| (interface.identity(), interface))
        .collect::<BTreeMap<_, _>>();
    let classes = metadata
        .classes()
        .iter()
        .map(|class| (class.identity(), class))
        .collect::<BTreeMap<_, _>>();
    if interfaces
        .keys()
        .any(|identity| classes.contains_key(identity))
    {
        return Err(());
    }
    for class in metadata.classes() {
        let mut witnesses = BTreeSet::new();
        for witness in class.interface_witnesses() {
            if !witnesses.insert(witness)
                || !reference_nominal_exists(
                    witness,
                    class.type_parameter_count(),
                    metadata.bubble(),
                    &interfaces,
                )
            {
                return Err(());
            }
        }
        if !class
            .interface_witnesses()
            .windows(2)
            .all(|pair| pair[0] < pair[1])
        {
            return Err(());
        }
        if let Some(base) = class.direct_base()
            && (!reference_nominal_exists(
                base,
                class.type_parameter_count(),
                metadata.bubble(),
                &classes,
            ) || base.definition() == class.identity()
                || classes
                    .get(&base.definition())
                    .is_some_and(|base| !base.is_open()))
        {
            return Err(());
        }
    }
    for class in metadata.classes() {
        let mut visited = BTreeSet::new();
        let mut cursor = Some(class.identity());
        while let Some(identity) = cursor {
            if !visited.insert(identity) {
                return Err(());
            }
            cursor = classes
                .get(&identity)
                .and_then(|class| class.direct_base())
                .filter(|base| base.arguments().is_empty())
                .map(ReferenceNominalType::definition)
                .filter(|identity| identity.bubble() == metadata.bubble());
        }
    }
    Ok(())
}

/// Verifies the complete ADR 0093 callable proof carried by public reference
/// metadata. Views are admitted only in direct callable positions with exact,
/// type-consistent retention and result-lender facts.
pub(crate) fn validate_reference_lifetime_summaries(
    metadata: &ReferenceMetadata,
) -> Result<(), ()> {
    for function in metadata.functions() {
        let parameters = function
            .parameters()
            .iter()
            .map(|parameter| parameter.parameter_type().clone())
            .collect::<Vec<_>>();
        validate_callable_lifetime_summary(
            &parameters,
            function.results(),
            function.lifetime_summary(),
        )?;
        if function.foreign_declaration().is_some()
            && parameters
                .iter()
                .chain(function.results())
                .any(reference_type_contains_view)
        {
            return Err(());
        }
    }
    Ok(())
}

/// Rejects unknown effect bits at the source-free artifact boundary. Nested
/// callable types are checked recursively so no operational unknown effect can
/// enter HIR through a container type.
pub(crate) fn validate_reference_effect_summaries(metadata: &ReferenceMetadata) -> Result<(), ()> {
    for function in metadata.functions() {
        if pop_types::EffectSummary::from_bits(function.effects().bits()).is_none()
            || function
                .parameters()
                .iter()
                .any(|parameter| !reference_type_effects_are_valid(parameter.parameter_type()))
            || function
                .results()
                .iter()
                .any(|result| !reference_type_effects_are_valid(result))
        {
            return Err(());
        }
    }
    Ok(())
}

fn reference_type_effects_are_valid(reference: &ReferenceType) -> bool {
    match reference {
        ReferenceType::Function {
            parameters,
            results,
            effects,
            ..
        } => {
            pop_types::EffectSummary::from_bits(effects.bits()).is_some()
                && parameters.iter().all(reference_type_effects_are_valid)
                && results.iter().all(reference_type_effects_are_valid)
        }
        ReferenceType::Class(nominal) | ReferenceType::Interface(nominal) => nominal
            .arguments()
            .iter()
            .all(reference_type_effects_are_valid),
        ReferenceType::Tuple(elements) | ReferenceType::Union(elements) => {
            elements.iter().all(reference_type_effects_are_valid)
        }
        ReferenceType::Array(element) | ReferenceType::Optional(element) => {
            reference_type_effects_are_valid(element)
        }
        ReferenceType::Table { key, value } => {
            reference_type_effects_are_valid(key) && reference_type_effects_are_valid(value)
        }
        ReferenceType::Builtin { arguments, .. } => {
            arguments.iter().all(reference_type_effects_are_valid)
        }
        ReferenceType::Primitive(_)
        | ReferenceType::TypeParameter(_)
        | ReferenceType::Record(_) => true,
    }
}

fn validate_callable_lifetime_summary(
    parameters: &[ReferenceType],
    results: &[ReferenceType],
    summary: Option<&pop_types::CallableLifetimeSummary>,
) -> Result<(), ()> {
    let missing_is_borrowed = parameters
        .iter()
        .chain(results)
        .any(reference_type_contains_view);
    if summary.is_none() && missing_is_borrowed {
        return Err(());
    }
    let conservative =
        pop_types::CallableLifetimeSummary::conservative(parameters.len(), results.len());
    let summary = summary.unwrap_or(&conservative);
    if !summary.is_canonical_for(parameters.len(), results.len()) {
        return Err(());
    }
    for (index, parameter) in parameters.iter().enumerate() {
        validate_callable_position_type(parameter)?;
        if reference_view_kind(parameter).is_some()
            && summary.parameter_retention()[index] != pop_types::ParameterRetention::DoesNotRetain
        {
            return Err(());
        }
    }
    for (index, result) in results.iter().enumerate() {
        validate_callable_position_type(result)?;
        match (
            reference_view_kind(result),
            &summary.result_provenance()[index],
        ) {
            (Some(kind), pop_types::ResultProvenance::ReturnsAlias(source)) => {
                let lender = parameters.get(usize::from(*source)).ok_or(())?;
                if !reference_type_is_lender_for(lender, kind) {
                    return Err(());
                }
            }
            (Some(_), _) | (None, pop_types::ResultProvenance::ReturnsAlias(_)) => {
                return Err(());
            }
            (None, _) => {}
        }
    }
    Ok(())
}

fn validate_callable_position_type(reference: &ReferenceType) -> Result<(), ()> {
    if reference_view_kind(reference).is_some() {
        return Ok(());
    }
    match reference {
        ReferenceType::Function {
            parameters,
            results,
            lifetime_summary,
            ..
        } => validate_callable_lifetime_summary(parameters, results, lifetime_summary.as_ref()),
        _ if reference_type_contains_view(reference) => Err(()),
        _ => Ok(()),
    }
}

fn reference_view_kind(reference: &ReferenceType) -> Option<pop_types::ViewKind> {
    match reference {
        ReferenceType::Builtin {
            definition,
            arguments,
        } if arguments.is_empty() && *definition == pop_types::BYTES_VIEW_TYPE_ID => {
            Some(pop_types::ViewKind::Bytes)
        }
        ReferenceType::Builtin {
            definition,
            arguments,
        } if arguments.is_empty() && *definition == pop_types::TEXT_VIEW_TYPE_ID => {
            Some(pop_types::ViewKind::Text)
        }
        _ => None,
    }
}

fn reference_type_is_lender_for(reference: &ReferenceType, kind: pop_types::ViewKind) -> bool {
    reference_view_kind(reference) == Some(kind)
        || match (kind, reference) {
            (pop_types::ViewKind::Text, ReferenceType::Primitive(PrimitiveType::String)) => true,
            (
                pop_types::ViewKind::Bytes,
                ReferenceType::Builtin {
                    definition,
                    arguments,
                },
            ) => definition.raw() == 0 && arguments.is_empty(),
            _ => false,
        }
}

fn reference_type_contains_view(reference: &ReferenceType) -> bool {
    if reference_view_kind(reference).is_some() {
        return true;
    }
    match reference {
        ReferenceType::Class(nominal) | ReferenceType::Interface(nominal) => {
            nominal.arguments().iter().any(reference_type_contains_view)
        }
        ReferenceType::Tuple(elements) | ReferenceType::Union(elements) => {
            elements.iter().any(reference_type_contains_view)
        }
        ReferenceType::Function {
            parameters,
            results,
            ..
        } => parameters
            .iter()
            .chain(results)
            .any(reference_type_contains_view),
        ReferenceType::Array(element) | ReferenceType::Optional(element) => {
            reference_type_contains_view(element)
        }
        ReferenceType::Table { key, value } => {
            reference_type_contains_view(key) || reference_type_contains_view(value)
        }
        ReferenceType::Builtin { arguments, .. } => {
            arguments.iter().any(reference_type_contains_view)
        }
        ReferenceType::Primitive(_)
        | ReferenceType::TypeParameter(_)
        | ReferenceType::Record(_) => false,
    }
}

fn reference_nominal_exists<'reference, T>(
    nominal: &ReferenceNominalType,
    type_parameter_count: u16,
    owner: pop_foundation::BubbleId,
    declarations: &BTreeMap<SymbolIdentity, &'reference T>,
) -> bool {
    (nominal.definition().bubble() != owner || declarations.contains_key(&nominal.definition()))
        && nominal.arguments().iter().all(|argument| {
            reference_type_parameter_indices_are_valid(argument, type_parameter_count)
        })
}

fn reference_type_parameter_indices_are_valid(
    reference: &ReferenceType,
    type_parameter_count: u16,
) -> bool {
    match reference {
        ReferenceType::TypeParameter(index) => *index < type_parameter_count,
        ReferenceType::Class(nominal) | ReferenceType::Interface(nominal) => {
            nominal.arguments().iter().all(|argument| {
                reference_type_parameter_indices_are_valid(argument, type_parameter_count)
            })
        }
        ReferenceType::Tuple(elements) | ReferenceType::Union(elements) => {
            elements.iter().all(|element| {
                reference_type_parameter_indices_are_valid(element, type_parameter_count)
            })
        }
        ReferenceType::Function {
            parameters,
            results,
            ..
        } => parameters.iter().chain(results).all(|element| {
            reference_type_parameter_indices_are_valid(element, type_parameter_count)
        }),
        ReferenceType::Array(element) | ReferenceType::Optional(element) => {
            reference_type_parameter_indices_are_valid(element, type_parameter_count)
        }
        ReferenceType::Table { key, value } => {
            reference_type_parameter_indices_are_valid(key, type_parameter_count)
                && reference_type_parameter_indices_are_valid(value, type_parameter_count)
        }
        ReferenceType::Builtin { arguments, .. } => arguments.iter().all(|argument| {
            reference_type_parameter_indices_are_valid(argument, type_parameter_count)
        }),
        ReferenceType::Primitive(_) | ReferenceType::Record(_) => true,
    }
}

pub(crate) fn validate_reference_ffi_layouts(metadata: &ReferenceMetadata) -> Result<(), ()> {
    if !metadata
        .records()
        .windows(2)
        .all(|pair| pair[0].identity() < pair[1].identity())
        || metadata
            .records()
            .iter()
            .any(|record| record.identity().bubble() != metadata.bubble())
    {
        return Err(());
    }
    let records = metadata
        .records()
        .iter()
        .map(|record| (record.identity(), record))
        .collect::<BTreeMap<_, _>>();
    let retained_records = metadata
        .retained_adapters()
        .iter()
        .map(|adapter| adapter.identity().target())
        .collect::<BTreeSet<_>>();
    for record in metadata.records() {
        let mut names = BTreeSet::new();
        if record.name().is_empty()
            || record.namespace().is_empty()
            || record
                .fields()
                .iter()
                .any(|field| field.name().is_empty() || !names.insert(field.name()))
            || record
                .fields()
                .iter()
                .any(|field| !reference_type_records_exist(field.field_type(), &records))
        {
            return Err(());
        }
    }
    if metadata.functions().iter().any(|function| {
        function
            .parameters()
            .iter()
            .any(|parameter| !reference_type_records_exist(parameter.parameter_type(), &records))
            || function
                .results()
                .iter()
                .any(|result| !reference_type_records_exist(result, &records))
    }) {
        return Err(());
    }
    let Some(catalog) = metadata.ffi_layout_catalog() else {
        return if records
            .keys()
            .all(|identity| retained_records.contains(identity))
        {
            Ok(())
        } else {
            Err(())
        };
    };
    if records.is_empty()
        || pop_target::TargetSpec::for_triple(catalog.target()).is_err()
        || !catalog
            .entries()
            .windows(2)
            .all(|pair| pair[0].id() < pair[1].id())
    {
        return Err(());
    }
    let entries = catalog
        .entries()
        .iter()
        .map(|entry| (entry.id(), entry))
        .collect::<BTreeMap<_, _>>();
    let mut represented_records = BTreeSet::new();
    for entry in catalog.entries() {
        let expected_fingerprint = artifact_sha256_hex(entry.descriptor().as_bytes());
        let compact =
            u64::from_str_radix(entry.fingerprint().get(..16).ok_or(())?, 16).map_err(|_| ())?;
        if entry.id() == 0
            || entry.fingerprint() != expected_fingerprint
            || entry.id() != compact
            || entry.size() == 0
            || entry.alignment() == 0
            || !entry.alignment().is_power_of_two()
            || !entry
                .descriptor()
                .contains(&format!("\"target\":\"{}\"", catalog.target()))
            || !entry
                .descriptor()
                .contains(&format!("\"abi\":\"{}\"", reference_abi_name(entry.abi())))
        {
            return Err(());
        }
        match (entry.element(), entry.value_class()) {
            (ReferenceType::Record(identity), ReferenceFfiValueClass::Record(fields)) => {
                let record = records.get(identity).copied().ok_or(())?;
                represented_records.insert(*identity);
                if fields.len() != record.fields().len() {
                    return Err(());
                }
                let mut indices = BTreeSet::new();
                let mut ranges = Vec::new();
                for field in fields {
                    let index = usize::try_from(field.source_index()).map_err(|_| ())?;
                    let declared = record.fields().get(index).ok_or(())?;
                    let child = entries.get(&field.layout()).copied().ok_or(())?;
                    if field.name() != declared.name()
                        || !indices.insert(index)
                        || child.abi() != entry.abi()
                        || child.alignment() > entry.alignment()
                        || field.offset() % child.alignment() != 0
                    {
                        return Err(());
                    }
                    let end = field.offset().checked_add(child.size()).ok_or(())?;
                    if end > entry.size() {
                        return Err(());
                    }
                    ranges.push((field.offset(), end));
                }
                ranges.sort_unstable();
                if ranges.windows(2).any(|pair| pair[0].1 > pair[1].0) {
                    return Err(());
                }
            }
            (ReferenceType::Record(_), _) | (_, ReferenceFfiValueClass::Record(_)) => {
                return Err(());
            }
            _ => {}
        }
    }
    if records.keys().any(|identity| {
        !represented_records.contains(identity) && !retained_records.contains(identity)
    }) {
        return Err(());
    }
    Ok(())
}

fn reference_type_records_exist(
    reference: &ReferenceType,
    records: &BTreeMap<SymbolIdentity, &ReferenceRecord>,
) -> bool {
    match reference {
        ReferenceType::Record(identity) => records.contains_key(identity),
        ReferenceType::Class(nominal) | ReferenceType::Interface(nominal) => nominal
            .arguments()
            .iter()
            .all(|argument| reference_type_records_exist(argument, records)),
        ReferenceType::Tuple(elements) | ReferenceType::Union(elements) => elements
            .iter()
            .all(|element| reference_type_records_exist(element, records)),
        ReferenceType::Function {
            parameters,
            results,
            ..
        } => parameters
            .iter()
            .chain(results)
            .all(|element| reference_type_records_exist(element, records)),
        ReferenceType::Array(element) | ReferenceType::Optional(element) => {
            reference_type_records_exist(element, records)
        }
        ReferenceType::Table { key, value } => {
            reference_type_records_exist(key, records)
                && reference_type_records_exist(value, records)
        }
        ReferenceType::Builtin { arguments, .. } => arguments
            .iter()
            .all(|argument| reference_type_records_exist(argument, records)),
        ReferenceType::Primitive(_) | ReferenceType::TypeParameter(_) => true,
    }
}

const fn reference_abi_name(abi: pop_types::ForeignAbi) -> &'static str {
    match abi {
        pop_types::ForeignAbi::C => "C",
        pop_types::ForeignAbi::System => "System",
        pop_types::ForeignAbi::CUnwind => "CUnwind",
    }
}

#[derive(Default)]
struct NominalIdentityMaps {
    classes: BTreeMap<TypeId, SymbolIdentity>,
    interfaces: BTreeMap<TypeId, SymbolIdentity>,
}

fn nominal_identity_maps(
    hir: &HirBubble,
    index: &pop_resolve::DeclarationIndex,
) -> NominalIdentityMaps {
    let mut maps = NominalIdentityMaps::default();
    for declaration in hir.declarations() {
        let kind = match declaration.kind() {
            HirDeclarationKind::Class(_) => pop_resolve::DeclarationKind::Class,
            HirDeclarationKind::Interface(_) => pop_resolve::DeclarationKind::Interface,
            _ => continue,
        };
        let source = index.declarations().find(|source| {
            source.module() == declaration.module()
                && source.name() == declaration.name()
                && source.kind() == kind
        });
        let Some(source) = source else {
            continue;
        };
        let identity = SymbolIdentity::new(hir.bubble(), source.symbol());
        match declaration.kind() {
            HirDeclarationKind::Class(class) => {
                maps.classes.insert(class.type_id(), identity);
            }
            HirDeclarationKind::Interface(interface) => {
                maps.interfaces.insert(interface.type_id(), identity);
            }
            _ => unreachable!("filtered nominal declaration"),
        }
    }
    maps
}

fn public_nominal_references(
    hir: &HirBubble,
    index: &pop_resolve::DeclarationIndex,
    arena: &TypeArena,
    record_identities: &BTreeMap<TypeId, SymbolIdentity>,
    nominal_identities: &NominalIdentityMaps,
) -> Result<(Vec<ReferenceInterface>, Vec<ReferenceClass>), ReferenceMetadataError> {
    let mut interfaces = hir
        .declarations()
        .iter()
        .filter_map(|declaration| {
            let interface = declaration.as_interface()?;
            let source = index.declaration(declaration.symbol())?;
            (declaration.visibility() == pop_resolve::Visibility::Public
                && source.kind() == pop_resolve::DeclarationKind::Interface)
                .then_some((declaration, interface, source))
        })
        .map(|(declaration, interface, source)| {
            let type_parameter_count = match arena.get(interface.type_id()) {
                Some(SemanticType::Interface { arguments, .. }) => {
                    u16::try_from(arguments.len()).unwrap_or(u16::MAX)
                }
                _ => u16::MAX,
            };
            ReferenceInterface {
                identity: SymbolIdentity::new(hir.bubble(), declaration.symbol()),
                module: declaration.module(),
                namespace: source.namespace().to_owned(),
                name: declaration.name().to_owned(),
                type_parameter_count,
                span: declaration.span(),
            }
        })
        .collect::<Vec<_>>();
    interfaces.sort_by_key(ReferenceInterface::identity);
    let public_interfaces = interfaces
        .iter()
        .map(ReferenceInterface::identity)
        .collect::<BTreeSet<_>>();

    let mut classes = hir
        .declarations()
        .iter()
        .filter_map(|declaration| {
            let class = declaration.as_class()?;
            let source = index.declaration(declaration.symbol())?;
            (declaration.visibility() == pop_resolve::Visibility::Public
                && source.kind() == pop_resolve::DeclarationKind::Class)
                .then_some((declaration, class, source))
        })
        .map(|(declaration, class, source)| {
            let type_parameters = match arena.get(class.type_id()) {
                Some(SemanticType::Class { arguments, .. }) => arguments
                    .iter()
                    .enumerate()
                    .map(|(index, type_id)| (*type_id, u16::try_from(index).unwrap_or(u16::MAX)))
                    .collect::<BTreeMap<_, _>>(),
                _ => BTreeMap::new(),
            };
            let identity = SymbolIdentity::new(hir.bubble(), declaration.symbol());
            let mut interface_witnesses = class
                .interfaces()
                .iter()
                .map(|implementation| {
                    reference_type_with_parameters(
                        identity,
                        implementation.interface_type(),
                        arena,
                        &type_parameters,
                        record_identities,
                        nominal_identities,
                    )
                })
                .filter_map(|reference| match reference {
                    Ok(ReferenceType::Interface(nominal))
                        if nominal.definition().bubble() != hir.bubble()
                            || public_interfaces.contains(&nominal.definition()) =>
                    {
                        Some(Ok(nominal))
                    }
                    Ok(ReferenceType::Interface(_)) => None,
                    Ok(_) => Some(Err(ReferenceMetadataError::InvalidNominalMetadata)),
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            interface_witnesses.sort();
            interface_witnesses.dedup();
            Ok(ReferenceClass {
                identity,
                module: declaration.module(),
                namespace: source.namespace().to_owned(),
                name: declaration.name().to_owned(),
                type_parameter_count: u16::try_from(type_parameters.len()).unwrap_or(u16::MAX),
                is_open: class.is_open(),
                direct_base: None,
                interface_witnesses,
                span: declaration.span(),
            })
        })
        .collect::<Result<Vec<_>, ReferenceMetadataError>>()?;
    classes.sort_by_key(ReferenceClass::identity);
    Ok((interfaces, classes))
}

pub(crate) fn emit_reference_metadata(
    hir: &HirBubble,
    index: &pop_resolve::DeclarationIndex,
    arena: &TypeArena,
    retained_metadata: Option<&RetainedMetadataArtifacts>,
) -> Result<ReferenceMetadata, ReferenceMetadataError> {
    let nominal_identities = nominal_identity_maps(hir, index);
    let public_layouts = hir
        .declarations()
        .iter()
        .filter_map(|declaration| match declaration.kind() {
            HirDeclarationKind::Record(record)
                if declaration.visibility() == pop_resolve::Visibility::Public
                    && record.has_ffi_c_layout() =>
            {
                Some((record.type_id(), (declaration, record)))
            }
            _ => None,
        })
        .collect::<BTreeMap<_, _>>();
    let mut reachable_layouts = BTreeSet::new();
    for function in hir
        .foreign_functions()
        .iter()
        .filter(|function| function.visibility() == pop_resolve::Visibility::Public)
    {
        let owner = SymbolIdentity::new(hir.bubble(), function.symbol());
        for type_id in function
            .parameters()
            .iter()
            .map(pop_hir::HirParameter::type_id)
            .chain(function.results().iter().copied())
        {
            collect_public_layout_types(
                owner,
                type_id,
                arena,
                &public_layouts,
                &mut reachable_layouts,
            )?;
        }
    }
    let record_identities = reachable_layouts
        .iter()
        .map(|type_id| {
            let (declaration, _) = public_layouts[type_id];
            (
                *type_id,
                SymbolIdentity::new(hir.bubble(), declaration.symbol()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut records = reachable_layouts
        .iter()
        .map(|type_id| {
            let (hir_declaration, record) = public_layouts[type_id];
            let identity = SymbolIdentity::new(hir.bubble(), hir_declaration.symbol());
            let declaration = index
                .declaration(hir_declaration.symbol())
                .ok_or(ReferenceMetadataError::MissingDeclaration(identity))?;
            let fields = record
                .fields()
                .iter()
                .map(|field| {
                    reference_type_with_parameters(
                        identity,
                        field.field_type(),
                        arena,
                        &BTreeMap::new(),
                        &record_identities,
                        &nominal_identities,
                    )
                    .map(|field_type| ReferenceRecordField {
                        name: field.name().to_owned(),
                        field_type,
                    })
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ReferenceRecord {
                identity,
                module: hir_declaration.module(),
                namespace: declaration.namespace().to_owned(),
                name: hir_declaration.name().to_owned(),
                fields,
                span: hir_declaration.span(),
            })
        })
        .collect::<Result<Vec<_>, ReferenceMetadataError>>()?;
    records.sort_by_key(ReferenceRecord::identity);
    let (interfaces, classes) =
        public_nominal_references(hir, index, arena, &record_identities, &nominal_identities)?;

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
                            &record_identities,
                            &nominal_identities,
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
                    &record_identities,
                    &nominal_identities,
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
                reference_type_with_parameters(
                    identity,
                    *type_id,
                    arena,
                    &type_parameter_indices,
                    &record_identities,
                    &nominal_identities,
                )
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
            lifetime_summary: Some(function.lifetime_summary().clone()),
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
                    &record_identities,
                    &nominal_identities,
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
                reference_type_with_parameters(
                    identity,
                    *type_id,
                    arena,
                    &BTreeMap::new(),
                    &record_identities,
                    &nominal_identities,
                )
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
            lifetime_summary: Some(pop_types::CallableLifetimeSummary::conservative(
                function.parameters().len(),
                function.results().len(),
            )),
            foreign_declaration: Some(function.declaration().clone()),
            span: function.declaration().span(),
            specialization_capsule: None,
        });
    }
    functions.sort_by_key(ReferenceFunction::identity);
    let ffi_record_identities = record_identities
        .iter()
        .filter(|(type_id, _)| {
            public_layouts
                .get(type_id)
                .is_some_and(|(_, record)| record.has_ffi_c_layout())
        })
        .map(|(type_id, identity)| (*type_id, *identity))
        .collect();
    let ffi_layout_catalog = reference_ffi_layout_catalog(
        hir,
        arena,
        &ffi_record_identities,
        &nominal_identities,
        &hir.foreign_functions()
            .iter()
            .filter(|function| function.visibility() == pop_resolve::Visibility::Public)
            .map(pop_hir::HirForeignFunction::symbol)
            .collect(),
    )?;
    let retained_adapters = retained_metadata
        .map_or_else(
            || Ok(Vec::new()),
            |artifacts| artifacts.public_references(hir.bubble(), index),
        )
        .map_err(|_| ReferenceMetadataError::InvalidRetainedMetadata)?;
    Ok(ReferenceMetadata {
        bubble: hir.bubble(),
        records,
        interfaces,
        classes,
        functions,
        retained_adapters,
        ffi_layout_catalog,
    })
}

fn collect_public_layout_types<'layout>(
    owner: SymbolIdentity,
    type_id: TypeId,
    arena: &TypeArena,
    public_layouts: &BTreeMap<
        TypeId,
        (
            &'layout pop_hir::HirDeclaration,
            &'layout pop_hir::HirRecordDeclaration,
        ),
    >,
    reachable: &mut BTreeSet<TypeId>,
) -> Result<(), ReferenceMetadataError> {
    let Some(SemanticType::Record(fields)) = arena.get(type_id) else {
        return Ok(());
    };
    if !reachable.insert(type_id) {
        return Ok(());
    }
    if !public_layouts.contains_key(&type_id) {
        return Err(ReferenceMetadataError::UnsupportedPublicType {
            function: owner,
            type_id,
        });
    }
    for (_, field_type) in fields {
        collect_public_layout_types(owner, *field_type, arena, public_layouts, reachable)?;
    }
    Ok(())
}

fn reference_ffi_layout_catalog(
    hir: &HirBubble,
    arena: &TypeArena,
    record_identities: &BTreeMap<TypeId, SymbolIdentity>,
    nominal_identities: &NominalIdentityMaps,
    public_foreign_symbols: &BTreeSet<SymbolId>,
) -> Result<Option<ReferenceFfiLayoutCatalog>, ReferenceMetadataError> {
    if record_identities.is_empty() {
        return Ok(None);
    }
    let mir = pop_mir::lower_hir_bubble_with_fingerprint(hir, arena, artifact_sha256_hex)
        .map_err(|_| ReferenceMetadataError::InvalidFfiLayout)?;
    let mut included = BTreeSet::new();
    let mut pending = mir
        .foreign_functions()
        .iter()
        .filter(|function| public_foreign_symbols.contains(&function.symbol()))
        .flat_map(|function| {
            function
                .parameter_layouts()
                .iter()
                .chain(function.result_layouts())
                .flatten()
                .copied()
        })
        .collect::<Vec<_>>();
    while let Some(id) = pending.pop() {
        if !included.insert(id) {
            continue;
        }
        let entry = mir
            .ffi_layouts()
            .get(id)
            .ok_or(ReferenceMetadataError::InvalidFfiLayout)?;
        if let pop_mir::MirFfiValueClass::Record(fields) = entry.value_class() {
            pending.extend(fields.iter().map(pop_mir::MirFfiLayoutField::layout));
        }
    }
    let owner = public_foreign_symbols
        .first()
        .copied()
        .map(|symbol| SymbolIdentity::new(hir.bubble(), symbol))
        .ok_or(ReferenceMetadataError::InvalidFfiLayout)?;
    let entries = mir
        .ffi_layouts()
        .entries()
        .iter()
        .filter(|entry| included.contains(&entry.id()))
        .map(|entry| {
            let element = reference_type_with_parameters(
                owner,
                entry.element(),
                arena,
                &BTreeMap::new(),
                record_identities,
                nominal_identities,
            )?;
            let value_class = match entry.value_class() {
                pop_mir::MirFfiValueClass::Integer => ReferenceFfiValueClass::Integer,
                pop_mir::MirFfiValueClass::Float => ReferenceFfiValueClass::Float,
                pop_mir::MirFfiValueClass::Pointer => ReferenceFfiValueClass::Pointer,
                pop_mir::MirFfiValueClass::FunctionPointer => {
                    ReferenceFfiValueClass::FunctionPointer
                }
                pop_mir::MirFfiValueClass::Handle => ReferenceFfiValueClass::Handle,
                pop_mir::MirFfiValueClass::Record(fields) => {
                    let Some(SemanticType::Record(semantic_fields)) = arena.get(entry.element())
                    else {
                        return Err(ReferenceMetadataError::InvalidFfiLayout);
                    };
                    ReferenceFfiValueClass::Record(
                        fields
                            .iter()
                            .map(|field| {
                                let name = field.name().or_else(|| {
                                    semantic_fields
                                        .get(field.source_index() as usize)
                                        .map(|(name, _)| name.as_str())
                                })?;
                                Some(ReferenceFfiLayoutField {
                                    name: name.to_owned(),
                                    source_index: field.source_index(),
                                    layout: field.layout().raw(),
                                    offset: field.offset(),
                                })
                            })
                            .collect::<Option<Vec<_>>>()
                            .ok_or(ReferenceMetadataError::InvalidFfiLayout)?,
                    )
                }
            };
            Ok(ReferenceFfiLayout {
                id: entry.id().raw(),
                element,
                size: entry.size(),
                alignment: entry.alignment(),
                value_class,
                abi: entry.abi(),
                descriptor: entry.descriptor().to_owned(),
                fingerprint: entry.fingerprint().to_owned(),
            })
        })
        .collect::<Result<Vec<_>, ReferenceMetadataError>>()?;
    Ok(Some(ReferenceFfiLayoutCatalog {
        target: mir.ffi_layouts().target().to_owned(),
        entries,
    }))
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
    record_identities: &BTreeMap<TypeId, SymbolIdentity>,
    nominal_identities: &NominalIdentityMaps,
) -> Result<ReferenceType, ReferenceMetadataError> {
    match arena.get(type_id) {
        Some(SemanticType::Primitive(primitive)) => Ok(ReferenceType::Primitive(*primitive)),
        Some(SemanticType::Record(_)) => record_identities
            .get(&type_id)
            .copied()
            .map(ReferenceType::Record)
            .ok_or(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
        Some(SemanticType::Class { arguments, .. }) => nominal_identities
            .classes
            .get(&type_id)
            .copied()
            .map(|definition| {
                arguments
                    .iter()
                    .map(|argument| {
                        reference_type_with_parameters(
                            function,
                            *argument,
                            arena,
                            type_parameters,
                            record_identities,
                            nominal_identities,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|arguments| {
                        ReferenceType::Class(ReferenceNominalType {
                            definition,
                            arguments,
                        })
                    })
            })
            .transpose()?
            .ok_or(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
        Some(SemanticType::Interface { arguments, .. }) => nominal_identities
            .interfaces
            .get(&type_id)
            .copied()
            .map(|definition| {
                arguments
                    .iter()
                    .map(|argument| {
                        reference_type_with_parameters(
                            function,
                            *argument,
                            arena,
                            type_parameters,
                            record_identities,
                            nominal_identities,
                        )
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(|arguments| {
                        ReferenceType::Interface(ReferenceNominalType {
                            definition,
                            arguments,
                        })
                    })
            })
            .transpose()?
            .ok_or(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
        Some(SemanticType::TypeParameter(_)) => type_parameters
            .get(&type_id)
            .copied()
            .map(ReferenceType::TypeParameter)
            .ok_or(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
        Some(SemanticType::Tuple(elements)) => Ok(ReferenceType::Tuple(
            elements
                .iter()
                .map(|element| {
                    reference_type_with_parameters(
                        function,
                        *element,
                        arena,
                        type_parameters,
                        record_identities,
                        nominal_identities,
                    )
                })
                .collect::<Result<_, _>>()?,
        )),
        Some(SemanticType::Function {
            is_async,
            parameters,
            results,
            effects,
            lifetime_summary,
        }) => Ok(ReferenceType::Function {
            is_async: *is_async,
            parameters: parameters
                .iter()
                .map(|parameter| {
                    reference_type_with_parameters(
                        function,
                        *parameter,
                        arena,
                        type_parameters,
                        record_identities,
                        nominal_identities,
                    )
                })
                .collect::<Result<_, _>>()?,
            results: results
                .iter()
                .map(|result| {
                    reference_type_with_parameters(
                        function,
                        *result,
                        arena,
                        type_parameters,
                        record_identities,
                        nominal_identities,
                    )
                })
                .collect::<Result<_, _>>()?,
            effects: *effects,
            lifetime_summary: Some(lifetime_summary.clone()),
        }),
        Some(SemanticType::Array(element)) => Ok(ReferenceType::Array(Box::new(
            reference_type_with_parameters(
                function,
                *element,
                arena,
                type_parameters,
                record_identities,
                nominal_identities,
            )?,
        ))),
        Some(SemanticType::Table { key, value }) => Ok(ReferenceType::Table {
            key: Box::new(reference_type_with_parameters(
                function,
                *key,
                arena,
                type_parameters,
                record_identities,
                nominal_identities,
            )?),
            value: Box::new(reference_type_with_parameters(
                function,
                *value,
                arena,
                type_parameters,
                record_identities,
                nominal_identities,
            )?),
        }),
        Some(SemanticType::Optional(element)) => Ok(ReferenceType::Optional(Box::new(
            reference_type_with_parameters(
                function,
                *element,
                arena,
                type_parameters,
                record_identities,
                nominal_identities,
            )?,
        ))),
        Some(SemanticType::Builtin {
            definition,
            arguments,
        }) => Ok(ReferenceType::Builtin {
            definition: *definition,
            arguments: arguments
                .iter()
                .map(|argument| {
                    reference_type_with_parameters(
                        function,
                        *argument,
                        arena,
                        type_parameters,
                        record_identities,
                        nominal_identities,
                    )
                })
                .collect::<Result<_, _>>()?,
        }),
        Some(SemanticType::Union(elements)) => Ok(ReferenceType::Union(
            elements
                .iter()
                .map(|element| {
                    reference_type_with_parameters(
                        function,
                        *element,
                        arena,
                        type_parameters,
                        record_identities,
                        nominal_identities,
                    )
                })
                .collect::<Result<_, _>>()?,
        )),
        _ => Err(ReferenceMetadataError::UnsupportedPublicType { function, type_id }),
    }
}

pub(crate) fn define_reference_records(
    metadata: &[ReferenceMetadata],
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
) -> BTreeMap<SymbolIdentity, TypeId> {
    let mut pending = metadata
        .iter()
        .flat_map(ReferenceMetadata::records)
        .collect::<Vec<_>>();
    let mut record_types = BTreeMap::new();
    while !pending.is_empty() {
        let mut remaining = Vec::new();
        let mut progressed = false;
        for record in pending {
            let Some(fields) = record
                .fields()
                .iter()
                .map(|field| {
                    try_reference_type_id(
                        field.field_type(),
                        resolver.arena_mut(),
                        &[],
                        &record_types,
                    )
                    .map(|field_type| (field.name().to_owned(), field_type))
                })
                .collect::<Option<Vec<_>>>()
            else {
                remaining.push(record);
                continue;
            };
            let declaration = database
                .index()
                .declaration_by_reference_identity(record.identity())
                .expect("verified public record identity is indexed");
            let definition = resolver
                .define_referenced_record(declaration.symbol(), fields, true, record.span())
                .expect("verified public record schema reconstructs once");
            record_types.insert(record.identity(), definition.type_id());
            progressed = true;
        }
        assert!(progressed, "verified public record metadata is acyclic");
        pending = remaining;
    }
    record_types
}

#[derive(Default)]
pub(crate) struct ReferenceNominalTypes {
    classes: BTreeMap<SymbolIdentity, SymbolId>,
    interfaces: BTreeMap<SymbolIdentity, SymbolId>,
    direct_bases: BTreeMap<SymbolIdentity, ReferenceNominalType>,
}

pub(crate) fn define_reference_nominals(
    metadata: &[ReferenceMetadata],
    database: &ResolutionDatabase,
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
    resolver: &mut SignatureResolver<'_>,
) -> ReferenceNominalTypes {
    let mut definitions = ReferenceNominalTypes::default();
    for interface in metadata.iter().flat_map(ReferenceMetadata::interfaces) {
        let Some(declaration) = database
            .index()
            .declaration_by_reference_identity(interface.identity())
        else {
            continue;
        };
        let parameters = (0..interface.type_parameter_count())
            .map(|index| {
                resolver.referenced_type_parameter(format!("T{index}"), None, interface.span())
            })
            .collect();
        if resolver
            .define_referenced_interface(
                interface.module(),
                declaration.symbol(),
                parameters,
                interface.span(),
            )
            .is_some()
        {
            definitions
                .interfaces
                .insert(interface.identity(), declaration.symbol());
        }
    }
    for class in metadata.iter().flat_map(ReferenceMetadata::classes) {
        let Some(declaration) = database
            .index()
            .declaration_by_reference_identity(class.identity())
        else {
            continue;
        };
        let parameters = (0..class.type_parameter_count())
            .map(|index| {
                resolver.referenced_type_parameter(format!("T{index}"), None, class.span())
            })
            .collect::<Vec<_>>();
        let parameter_types = parameters
            .iter()
            .map(pop_types::ResolvedTypeParameter::type_id)
            .collect::<Vec<_>>();
        let Some(interface_types) = class
            .interface_witnesses()
            .iter()
            .map(|witness| {
                reference_nominal_type_id(
                    witness,
                    resolver,
                    &parameter_types,
                    record_types,
                    &definitions,
                    true,
                )
            })
            .collect::<Option<Vec<_>>>()
        else {
            continue;
        };
        if resolver
            .define_referenced_class(
                class.module(),
                declaration.symbol(),
                parameters,
                class.is_open(),
                interface_types,
                class.span(),
            )
            .is_some()
        {
            definitions
                .classes
                .insert(class.identity(), declaration.symbol());
            if let Some(base) = class.direct_base() {
                definitions
                    .direct_bases
                    .insert(class.identity(), base.clone());
            }
        }
    }
    definitions
}

pub(crate) fn hir_reference_nominal_catalog(
    resolver: &mut SignatureResolver<'_>,
    definitions: &ReferenceNominalTypes,
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
) -> pop_hir::HirNominalReferenceCatalog {
    let mut nominal_identities = NominalIdentityMaps::default();
    for (identity, symbol) in &definitions.interfaces {
        for definition in resolver
            .interface_definition(*symbol)
            .into_iter()
            .chain(resolver.interface_instances(*symbol))
        {
            nominal_identities
                .interfaces
                .insert(definition.type_id(), *identity);
        }
    }
    for (identity, symbol) in &definitions.classes {
        for definition in resolver
            .class_definition(*symbol)
            .into_iter()
            .chain(resolver.class_instances(*symbol))
        {
            nominal_identities
                .classes
                .insert(definition.type_id(), *identity);
        }
    }
    let record_identities = record_types
        .iter()
        .map(|(identity, type_id)| (*type_id, *identity))
        .collect::<BTreeMap<_, _>>();
    let mut interface_identity_by_type = BTreeMap::new();
    let mut interfaces = Vec::new();
    for (identity, symbol) in &definitions.interfaces {
        let class_definitions = resolver
            .interface_definition(*symbol)
            .into_iter()
            .chain(resolver.interface_instances(*symbol));
        for definition in class_definitions {
            if resolver
                .arena()
                .contains_type_parameter(definition.type_id())
            {
                continue;
            }
            let arguments = match resolver.arena().get(definition.type_id()) {
                Some(SemanticType::Interface { arguments, .. }) => arguments.clone(),
                _ => continue,
            };
            let Some(canonical_arguments) = canonical_runtime_arguments(
                *identity,
                &arguments,
                resolver.arena(),
                &record_identities,
                &nominal_identities,
            ) else {
                continue;
            };
            let nominal =
                pop_hir::HirNominalIdentity::new(*identity, arguments, canonical_arguments);
            interface_identity_by_type.insert(definition.type_id(), nominal.clone());
            interfaces.push(pop_hir::HirInterfaceReference::new(
                nominal,
                definition.interface(),
                definition.type_id(),
            ));
        }
    }
    let mut classes = Vec::new();
    for (identity, symbol) in &definitions.classes {
        let class_definitions = resolver
            .class_definition(*symbol)
            .into_iter()
            .chain(resolver.class_instances(*symbol))
            .cloned()
            .collect::<Vec<_>>();
        for definition in class_definitions {
            if resolver
                .arena()
                .contains_type_parameter(definition.type_id())
            {
                continue;
            }
            let arguments = match resolver.arena().get(definition.type_id()) {
                Some(SemanticType::Class { arguments, .. }) => arguments.clone(),
                _ => continue,
            };
            let Some(canonical_arguments) = canonical_runtime_arguments(
                *identity,
                &arguments,
                resolver.arena(),
                &record_identities,
                &nominal_identities,
            ) else {
                continue;
            };
            let interfaces = definition
                .interfaces()
                .iter()
                .filter_map(|implementation| {
                    let nominal = interface_identity_by_type
                        .get(&implementation.interface_type())?
                        .clone();
                    Some(pop_hir::HirInterfaceReference::new(
                        nominal,
                        implementation.interface(),
                        implementation.interface_type(),
                    ))
                })
                .collect();
            let base = definitions
                .direct_bases
                .get(identity)
                .and_then(|base| {
                    reference_nominal_type_id(
                        base,
                        resolver,
                        &arguments,
                        record_types,
                        definitions,
                        false,
                    )
                })
                .and_then(|base_type| match resolver.arena().get(base_type) {
                    Some(SemanticType::Class { class, .. }) => Some((*class, base_type)),
                    _ => None,
                });
            classes.push(pop_hir::HirClassReference::new(
                pop_hir::HirNominalIdentity::new(*identity, arguments, canonical_arguments),
                definition.class(),
                definition.type_id(),
                definition.is_open(),
                base,
                interfaces,
            ));
        }
    }
    pop_hir::HirNominalReferenceCatalog::new(interfaces, classes)
}

fn canonical_runtime_arguments(
    owner: SymbolIdentity,
    arguments: &[TypeId],
    arena: &TypeArena,
    record_identities: &BTreeMap<TypeId, SymbolIdentity>,
    nominal_identities: &NominalIdentityMaps,
) -> Option<Vec<pop_types::CanonicalTypeIdentity>> {
    arguments
        .iter()
        .map(|argument| {
            reference_type_with_parameters(
                owner,
                *argument,
                arena,
                &BTreeMap::new(),
                record_identities,
                nominal_identities,
            )
            .ok()
            .and_then(|reference| canonical_reference_type(&reference))
        })
        .collect()
}

fn canonical_reference_type(reference: &ReferenceType) -> Option<pop_types::CanonicalTypeIdentity> {
    use pop_types::{CanonicalNominalIdentity, CanonicalTypeIdentity};
    Some(match reference {
        ReferenceType::Primitive(primitive) => CanonicalTypeIdentity::Primitive(*primitive),
        ReferenceType::TypeParameter(_) => return None,
        ReferenceType::Record(identity) => CanonicalTypeIdentity::Record(*identity),
        ReferenceType::Class(nominal) | ReferenceType::Interface(nominal) => {
            let arguments = nominal
                .arguments()
                .iter()
                .map(canonical_reference_type)
                .collect::<Option<Vec<_>>>()?;
            let identity = CanonicalNominalIdentity::new(nominal.definition(), arguments);
            if matches!(reference, ReferenceType::Class(_)) {
                CanonicalTypeIdentity::Class(identity)
            } else {
                CanonicalTypeIdentity::Interface(identity)
            }
        }
        ReferenceType::Tuple(elements) => CanonicalTypeIdentity::Tuple(
            elements
                .iter()
                .map(canonical_reference_type)
                .collect::<Option<Vec<_>>>()?,
        ),
        ReferenceType::Function {
            is_async,
            parameters,
            results,
            effects,
            lifetime_summary,
        } => CanonicalTypeIdentity::Function {
            is_async: *is_async,
            parameters: parameters
                .iter()
                .map(canonical_reference_type)
                .collect::<Option<Vec<_>>>()?,
            results: results
                .iter()
                .map(canonical_reference_type)
                .collect::<Option<Vec<_>>>()?,
            effects: *effects,
            lifetime_summary: lifetime_summary.clone().unwrap_or_else(|| {
                pop_types::CallableLifetimeSummary::conservative(parameters.len(), results.len())
            }),
        },
        ReferenceType::Array(element) => {
            CanonicalTypeIdentity::Array(Box::new(canonical_reference_type(element)?))
        }
        ReferenceType::Table { key, value } => CanonicalTypeIdentity::Table {
            key: Box::new(canonical_reference_type(key)?),
            value: Box::new(canonical_reference_type(value)?),
        },
        ReferenceType::Optional(element) => {
            CanonicalTypeIdentity::Optional(Box::new(canonical_reference_type(element)?))
        }
        ReferenceType::Builtin {
            definition,
            arguments,
        } => CanonicalTypeIdentity::Builtin {
            definition: *definition,
            arguments: arguments
                .iter()
                .map(canonical_reference_type)
                .collect::<Option<Vec<_>>>()?,
        },
        ReferenceType::Union(elements) => CanonicalTypeIdentity::Union(
            elements
                .iter()
                .map(canonical_reference_type)
                .collect::<Option<Vec<_>>>()?,
        ),
    })
}

fn reference_nominal_type_id(
    nominal: &ReferenceNominalType,
    resolver: &mut SignatureResolver<'_>,
    type_parameters: &[TypeId],
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
    nominal_types: &ReferenceNominalTypes,
    interface: bool,
) -> Option<TypeId> {
    let arguments = nominal
        .arguments()
        .iter()
        .map(|argument| {
            try_reference_type_id_with_nominals(
                argument,
                resolver,
                type_parameters,
                record_types,
                nominal_types,
            )
        })
        .collect::<Option<Vec<_>>>()?;
    let symbol = if interface {
        nominal_types.interfaces.get(&nominal.definition())
    } else {
        nominal_types.classes.get(&nominal.definition())
    }
    .copied()?;
    if arguments.is_empty() {
        return resolver.declaration_type(symbol);
    }
    if interface {
        resolver
            .instantiate_interface(symbol, &arguments)
            .map(|definition| definition.type_id())
    } else {
        resolver
            .instantiate_class(symbol, &arguments)
            .map(|definition| definition.type_id())
    }
}

pub(crate) fn hir_reference_ffi_layout_catalog(
    metadata: &[ReferenceMetadata],
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
) -> Result<Option<ImportedHirFfiLayoutCatalog>, pop_hir::HirBubbleError> {
    let catalogs = metadata
        .iter()
        .filter_map(ReferenceMetadata::ffi_layout_catalog)
        .collect::<Vec<_>>();
    let Some(first) = catalogs.first() else {
        return Ok(None);
    };
    if catalogs
        .iter()
        .any(|catalog| catalog.target() != first.target())
    {
        return Err(pop_hir::HirBubbleError::InvalidReferenceFfiLayout);
    }
    let records = metadata
        .iter()
        .flat_map(ReferenceMetadata::records)
        .map(|record| (record.identity(), record))
        .collect::<BTreeMap<_, _>>();
    let mut imported = BTreeMap::new();
    for layout in catalogs.iter().flat_map(|catalog| catalog.entries()) {
        let element =
            try_reference_type_id(layout.element(), resolver.arena_mut(), &[], record_types)
                .ok_or(pop_hir::HirBubbleError::InvalidReferenceFfiLayout)?;
        let value_class = match layout.value_class() {
            ReferenceFfiValueClass::Integer => ImportedHirFfiValueClass::Integer,
            ReferenceFfiValueClass::Float => ImportedHirFfiValueClass::Float,
            ReferenceFfiValueClass::Pointer => ImportedHirFfiValueClass::Pointer,
            ReferenceFfiValueClass::FunctionPointer => ImportedHirFfiValueClass::FunctionPointer,
            ReferenceFfiValueClass::Handle => ImportedHirFfiValueClass::Handle,
            ReferenceFfiValueClass::Record(fields) => {
                let ReferenceType::Record(identity) = layout.element() else {
                    return Err(pop_hir::HirBubbleError::InvalidReferenceFfiLayout);
                };
                let record = records
                    .get(identity)
                    .copied()
                    .ok_or(pop_hir::HirBubbleError::InvalidReferenceFfiLayout)?;
                let declaration = database
                    .index()
                    .declaration_by_reference_identity(*identity)
                    .ok_or(pop_hir::HirBubbleError::InvalidReferenceFfiLayout)?;
                let definition = resolver
                    .record_definition(declaration.symbol())
                    .ok_or(pop_hir::HirBubbleError::InvalidReferenceFfiLayout)?;
                if fields.len() != record.fields().len()
                    || definition.fields().len() != record.fields().len()
                {
                    return Err(pop_hir::HirBubbleError::InvalidReferenceFfiLayout);
                }
                let mut indices = BTreeSet::new();
                ImportedHirFfiValueClass::Record(
                    fields
                        .iter()
                        .map(|field| {
                            let index = usize::try_from(field.source_index()).ok()?;
                            let declared = record.fields().get(index)?;
                            let local = definition.fields().get(index)?;
                            if field.name() != declared.name()
                                || field.name() != local.name()
                                || !indices.insert(index)
                            {
                                return None;
                            }
                            Some(ImportedHirFfiLayoutField::new(
                                local.field(),
                                field.name(),
                                field.source_index(),
                                field.layout(),
                                field.offset(),
                            ))
                        })
                        .collect::<Option<Vec<_>>>()
                        .ok_or(pop_hir::HirBubbleError::InvalidReferenceFfiLayout)?,
                )
            }
        };
        let entry = ImportedHirFfiLayout::new(
            layout.id(),
            element,
            layout.size(),
            layout.alignment(),
            value_class,
            layout.abi(),
            layout.descriptor(),
            layout.fingerprint(),
        );
        match imported.entry(layout.id()) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(entry);
            }
            std::collections::btree_map::Entry::Occupied(slot) if slot.get() == &entry => {}
            std::collections::btree_map::Entry::Occupied(_) => {
                return Err(pop_hir::HirBubbleError::InvalidReferenceFfiLayout);
            }
        }
    }
    Ok(Some(ImportedHirFfiLayoutCatalog::new(
        first.target(),
        imported.into_values().collect(),
    )))
}

pub(crate) fn reference_signatures(
    metadata: &[ReferenceMetadata],
    database: &ResolutionDatabase,
    resolver: &mut SignatureResolver<'_>,
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
    nominal_types: &ReferenceNominalTypes,
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
                let bound = parameter.bound().map(|bound| {
                    reference_type_id_with_nominals(
                        bound,
                        resolver,
                        &parameter_types,
                        record_types,
                        nominal_types,
                    )
                });
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
                        reference_type_id_with_nominals(
                            parameter.parameter_type(),
                            resolver,
                            &parameter_types,
                            record_types,
                            nominal_types,
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
                        reference_type_id_with_nominals(
                            result,
                            resolver,
                            &parameter_types,
                            record_types,
                            nominal_types,
                        ),
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
                )
                .with_lifetime_summary(
                    function.lifetime_summary().cloned().unwrap_or_else(|| {
                        pop_types::CallableLifetimeSummary::conservative(
                            function.parameters().len(),
                            function.results().len(),
                        )
                    }),
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
                signature.lifetime_summary().clone(),
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
            lifetime_summary,
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
            lifetime_summary,
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

fn reference_type_id_with_nominals(
    reference: &ReferenceType,
    resolver: &mut SignatureResolver<'_>,
    type_parameters: &[TypeId],
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
    nominal_types: &ReferenceNominalTypes,
) -> TypeId {
    try_reference_type_id_with_nominals(
        reference,
        resolver,
        type_parameters,
        record_types,
        nominal_types,
    )
    .expect("verified nominal reference metadata type")
}

fn try_reference_type_id_with_nominals(
    reference: &ReferenceType,
    resolver: &mut SignatureResolver<'_>,
    type_parameters: &[TypeId],
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
    nominal_types: &ReferenceNominalTypes,
) -> Option<TypeId> {
    match reference {
        ReferenceType::Primitive(primitive) => {
            let source_name = PrimitiveType::source_schema()
                .iter()
                .copied()
                .find(|entry| entry.primitive() == *primitive && !entry.is_alias())?
                .source_name();
            resolver.arena().source_type(source_name)
        }
        ReferenceType::TypeParameter(index) => type_parameters.get(usize::from(*index)).copied(),
        ReferenceType::Record(identity) => record_types.get(identity).copied(),
        ReferenceType::Class(nominal) => reference_nominal_type_id(
            nominal,
            resolver,
            type_parameters,
            record_types,
            nominal_types,
            false,
        ),
        ReferenceType::Interface(nominal) => reference_nominal_type_id(
            nominal,
            resolver,
            type_parameters,
            record_types,
            nominal_types,
            true,
        ),
        ReferenceType::Tuple(elements) | ReferenceType::Union(elements) => {
            let elements = elements
                .iter()
                .map(|element| {
                    try_reference_type_id_with_nominals(
                        element,
                        resolver,
                        type_parameters,
                        record_types,
                        nominal_types,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            let semantic = if matches!(reference, ReferenceType::Tuple(_)) {
                SemanticType::Tuple(elements)
            } else {
                SemanticType::Union(elements)
            };
            resolver.arena_mut().intern(semantic).ok()
        }
        ReferenceType::Function {
            is_async,
            parameters,
            results,
            effects,
            lifetime_summary,
        } => {
            let parameters = parameters
                .iter()
                .map(|parameter| {
                    try_reference_type_id_with_nominals(
                        parameter,
                        resolver,
                        type_parameters,
                        record_types,
                        nominal_types,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            let results = results
                .iter()
                .map(|result| {
                    try_reference_type_id_with_nominals(
                        result,
                        resolver,
                        type_parameters,
                        record_types,
                        nominal_types,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            let lifetime_summary = lifetime_summary.clone().unwrap_or_else(|| {
                pop_types::CallableLifetimeSummary::conservative(parameters.len(), results.len())
            });
            resolver
                .arena_mut()
                .intern(SemanticType::Function {
                    is_async: *is_async,
                    parameters,
                    results,
                    effects: *effects,
                    lifetime_summary,
                })
                .ok()
        }
        ReferenceType::Array(element) | ReferenceType::Optional(element) => {
            let element = try_reference_type_id_with_nominals(
                element,
                resolver,
                type_parameters,
                record_types,
                nominal_types,
            )?;
            if matches!(reference, ReferenceType::Array(_)) {
                resolver
                    .arena_mut()
                    .intern(SemanticType::Array(element))
                    .ok()
            } else {
                resolver.arena_mut().optional(element).ok()
            }
        }
        ReferenceType::Table { key, value } => {
            let key = try_reference_type_id_with_nominals(
                key,
                resolver,
                type_parameters,
                record_types,
                nominal_types,
            )?;
            let value = try_reference_type_id_with_nominals(
                value,
                resolver,
                type_parameters,
                record_types,
                nominal_types,
            )?;
            resolver
                .arena_mut()
                .intern(SemanticType::Table { key, value })
                .ok()
        }
        ReferenceType::Builtin {
            definition,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| {
                    try_reference_type_id_with_nominals(
                        argument,
                        resolver,
                        type_parameters,
                        record_types,
                        nominal_types,
                    )
                })
                .collect::<Option<Vec<_>>>()?;
            resolver
                .arena_mut()
                .intern(SemanticType::Builtin {
                    definition: *definition,
                    arguments,
                })
                .ok()
        }
    }
}

fn try_reference_type_id(
    reference: &ReferenceType,
    arena: &mut TypeArena,
    type_parameters: &[TypeId],
    record_types: &BTreeMap<SymbolIdentity, TypeId>,
) -> Option<TypeId> {
    match reference {
        ReferenceType::Primitive(primitive) => {
            let source_name = PrimitiveType::source_schema()
                .iter()
                .copied()
                .find(|entry| entry.primitive() == *primitive && !entry.is_alias())?
                .source_name();
            arena.source_type(source_name)
        }
        ReferenceType::TypeParameter(index) => type_parameters.get(usize::from(*index)).copied(),
        ReferenceType::Record(identity) => record_types.get(identity).copied(),
        ReferenceType::Class(_) | ReferenceType::Interface(_) => None,
        ReferenceType::Tuple(elements) => {
            let elements = elements
                .iter()
                .map(|element| try_reference_type_id(element, arena, type_parameters, record_types))
                .collect::<Option<Vec<_>>>()?;
            arena.intern(SemanticType::Tuple(elements)).ok()
        }
        ReferenceType::Function {
            is_async,
            parameters,
            results,
            effects,
            lifetime_summary,
        } => {
            let parameters = parameters
                .iter()
                .map(|parameter| {
                    try_reference_type_id(parameter, arena, type_parameters, record_types)
                })
                .collect::<Option<Vec<_>>>()?;
            let results = results
                .iter()
                .map(|result| try_reference_type_id(result, arena, type_parameters, record_types))
                .collect::<Option<Vec<_>>>()?;
            let lifetime_summary = lifetime_summary.clone().unwrap_or_else(|| {
                pop_types::CallableLifetimeSummary::conservative(parameters.len(), results.len())
            });
            arena
                .intern(SemanticType::Function {
                    is_async: *is_async,
                    parameters,
                    results,
                    effects: *effects,
                    lifetime_summary,
                })
                .ok()
        }
        ReferenceType::Array(element) => {
            let element = try_reference_type_id(element, arena, type_parameters, record_types)?;
            arena.intern(SemanticType::Array(element)).ok()
        }
        ReferenceType::Table { key, value } => {
            let key = try_reference_type_id(key, arena, type_parameters, record_types)?;
            let value = try_reference_type_id(value, arena, type_parameters, record_types)?;
            arena.intern(SemanticType::Table { key, value }).ok()
        }
        ReferenceType::Optional(element) => {
            let element = try_reference_type_id(element, arena, type_parameters, record_types)?;
            arena.optional(element).ok()
        }
        ReferenceType::Builtin {
            definition,
            arguments,
        } => {
            let arguments = arguments
                .iter()
                .map(|argument| {
                    try_reference_type_id(argument, arena, type_parameters, record_types)
                })
                .collect::<Option<Vec<_>>>()?;
            arena
                .intern(SemanticType::Builtin {
                    definition: *definition,
                    arguments,
                })
                .ok()
        }
        ReferenceType::Union(elements) => {
            let elements = elements
                .iter()
                .map(|element| try_reference_type_id(element, arena, type_parameters, record_types))
                .collect::<Option<Vec<_>>>()?;
            arena.intern(SemanticType::Union(elements)).ok()
        }
    }
}
